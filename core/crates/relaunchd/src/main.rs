//! relaunchd binary — the D-Bus transport around the authority engine.
//!
//! The service owns the per-uid key store under `/var/lib/omarchy-relaunch` and
//! is D-Bus activated as root. Every method derives the caller's uid and unique
//! sender from bus credentials (never from a request field), looks the account
//! name up from the password database, and runs the engine on a blocking thread
//! via a captured Tokio handle. Authorization is delegated to polkit
//! (`polkit::PolkitAuthenticator`). The name is held non-replaceably; a client
//! disconnect invalidates that uid's pending transaction; name loss exits.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code: a failed unwrap/index is a failed assertion"
    )
)]

use relaunch_sys::diag;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use zbus::fdo;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::BusName;

// Request caps are shared with the client via relaunch-core (INV-5) and enforced
// here BEFORE any parse.
use relaunch_core::{MAX_CLASS_LEN, MAX_REDUCE_OPS, MAX_REQUEST_BYTES};
use relaunchd::engine::{Engine, EngineError, Proposal, ReduceOp};
use relaunchd::polkit;
use relaunchd::store::Store;
use relaunchd::txn::Clock;

const STATE_ROOT: &str = "/var/lib/omarchy-relaunch";
const BUS_NAME: &str = "io.github.laytonf.Relaunch1";
const OBJECT_PATH: &str = "/io/github/laytonf/Relaunch1";
const IDLE_EXIT_SECS: u64 = 30;
struct SystemClock;
impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        // Monotonic (reviewer L2): a wall-clock change must not alter a lease.
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs()
    }
}

type Eng = Engine<SystemClock, polkit::PolkitAuthenticator>;

struct Activity {
    inflight: AtomicUsize,
    last_done: AtomicU64,
}

/// RAII in-flight counter: increments on creation and ALWAYS decrements on drop,
/// so a handler panic or early `?` cannot leave the count elevated (reviewer H3).
struct Busy(Arc<Activity>);
impl Busy {
    fn new(a: &Arc<Activity>) -> Self {
        a.inflight.fetch_add(1, Ordering::SeqCst);
        Busy(a.clone())
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        self.0
            .last_done
            .store(SystemClock.now_secs(), Ordering::SeqCst);
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Service {
    engine: Arc<Eng>,
    activity: Arc<Activity>,
    handle: tokio::runtime::Handle,
}

/// The reduce operations of a request. The errors are fixed text: nothing
/// from the request is echoed back.
fn parse_reduce_ops(ops: Vec<(String, String)>) -> Result<Vec<ReduceOp>, &'static str> {
    ops.into_iter()
        .map(|(kind, class)| {
            if class.len() > MAX_CLASS_LEN {
                return Err("class too long");
            }
            match kind.as_str() {
                "disable" => Ok(ReduceOp::Disable(class)),
                "drop" => Ok(ReduceOp::Drop(class)),
                _ => Err("unknown reduce op"),
            }
        })
        .collect()
}

fn eng_err(e: &EngineError) -> fdo::Error {
    fdo::Error::Failed(relaunch_core::bounded_message(&format!("{e:?}")))
}

impl Service {
    async fn caller(
        conn: &zbus::Connection,
        hdr: &zbus::message::Header<'_>,
    ) -> fdo::Result<(u32, String, String)> {
        let sender = hdr
            .sender()
            .ok_or_else(|| fdo::Error::Failed("no sender on message".into()))?
            .to_owned();
        let dbus = fdo::DBusProxy::new(conn).await?;
        let uid = dbus
            .get_connection_unix_user(BusName::from(sender.clone()))
            .await?;
        let user = relaunch_sys::account_name(uid)
            .ok_or_else(|| fdo::Error::Failed(format!("no account for uid {uid}")))?;
        Ok((uid, user, sender.as_str().to_string()))
    }

    async fn run_blocking<T, F>(&self, f: F) -> fdo::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Eng) -> Result<T, EngineError> + Send + 'static,
    {
        let engine = self.engine.clone();
        let _busy = Busy::new(&self.activity);
        let r = self
            .handle
            .spawn_blocking(move || f(&engine))
            .await
            .map_err(|e| fdo::Error::Failed(format!("task: {e}")))?;
        r.map_err(|e| eng_err(&e))
    }
}

#[zbus::interface(name = "io.github.laytonf.Relaunch1")]
impl Service {
    async fn prepare(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        base: Vec<u8>,
        proposal_json: Vec<u8>,
    ) -> fdo::Result<(Vec<u8>, String, String, String)> {
        if base.len() > MAX_REQUEST_BYTES || proposal_json.len() > MAX_REQUEST_BYTES {
            return Err(fdo::Error::InvalidArgs("request too large".into()));
        }
        let (uid, user, sender) = Self::caller(conn, &hdr).await?;
        let proposal: Proposal = serde_json::from_slice(&proposal_json).map_err(|e| {
            fdo::Error::InvalidArgs(format!("proposal: {}", relaunch_core::json_error(&e)))
        })?;
        let base_opt = if base.is_empty() { None } else { Some(base) };
        let sender_for_prepare = sender.clone();
        let out = self
            .run_blocking(move |eng| {
                let out = eng.prepare(
                    uid,
                    &user,
                    &sender_for_prepare,
                    base_opt.as_deref(),
                    proposal,
                )?;
                Ok((
                    out.txid.map(|t| t.to_vec()).unwrap_or_default(),
                    out.code.unwrap_or_default(),
                    out.summary,
                    out.rendered_diff,
                ))
            })
            .await?;

        // Disconnect-before-publication (reviewer #4): the disconnect watcher
        // may have run its invalidation for this sender BEFORE this queued
        // Prepare published its pending record. Recheck ownership now that the
        // record exists; if the sender is already gone, invalidate the orphan so
        // it does not hold the one-per-uid slot until the lease expires.
        if !out.0.is_empty() {
            let still_owned = fdo::DBusProxy::new(conn)
                .await?
                .name_has_owner(BusName::from(
                    zbus::names::UniqueName::try_from(sender.as_str())
                        .map_err(|e| fdo::Error::Failed(format!("{e}")))?,
                ))
                .await?;
            if !still_owned {
                let engine = self.engine.clone();
                std::mem::drop(tokio::task::spawn_blocking(move || {
                    engine.invalidate_sender(&sender);
                }));
            }
        }
        Ok(out)
    }

    async fn authorize(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        txid: Vec<u8>,
    ) -> fdo::Result<Vec<u8>> {
        let (uid, _user, sender) = Self::caller(conn, &hdr).await?;
        let txid: [u8; 16] = txid
            .try_into()
            .map_err(|_| fdo::Error::InvalidArgs("txid must be 16 bytes".into()))?;
        self.run_blocking(move |eng| eng.authorize(uid, &sender, &txid))
            .await
    }

    async fn reduce(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        base: Vec<u8>,
        ops: Vec<(String, String)>,
    ) -> fdo::Result<Vec<u8>> {
        if base.len() > MAX_REQUEST_BYTES || ops.len() > MAX_REDUCE_OPS {
            return Err(fdo::Error::InvalidArgs("request too large".into()));
        }
        let (uid, user, _sender) = Self::caller(conn, &hdr).await?;
        let parsed = parse_reduce_ops(ops).map_err(|e| fdo::Error::InvalidArgs(e.into()))?;
        self.run_blocking(move |eng| eng.reduce(uid, &user, &base, &parsed))
            .await
    }

    async fn purge(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> fdo::Result<()> {
        let (uid, _user, sender) = Self::caller(conn, &hdr).await?;
        self.run_blocking(move |eng| eng.purge(uid, &sender)).await
    }

    async fn has_pending(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> fdo::Result<bool> {
        let (uid, _user, _sender) = Self::caller(conn, &hdr).await?;
        let engine = self.engine.clone();
        Ok(engine.has_pending(uid))
    }
}

/// Consume a `NameOwnerChanged` stream, invalidating a uid's pending transaction
/// when the Prepare connection drops (reviewer I7/#3). The synchronous,
/// lock-taking invalidation is offloaded to the blocking pool so it cannot stall
/// the tokio worker or subsequent disconnect handling.
async fn run_disconnect_watcher(mut stream: zbus::fdo::NameOwnerChangedStream, engine: Arc<Eng>) {
    use futures_util::StreamExt;
    while let Some(sig) = stream.next().await {
        if let Ok(args) = sig.args() {
            let name = args.name().as_str();
            if name.starts_with(':') && args.new_owner().is_none() {
                let engine = engine.clone();
                let name = name.to_string();
                // Fire-and-forget: the detached JoinHandle keeps the task running.
                std::mem::drop(tokio::task::spawn_blocking(move || {
                    engine.invalidate_sender(&name);
                }));
            }
        }
    }
    diag!("relaunchd: disconnect watcher stream ended");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handle = tokio::runtime::Handle::current();
    // A dedicated system-bus connection for polkit calls, created under this
    // runtime so the authenticator's block_on can drive it.
    let pk_conn = zbus::Connection::system().await?;
    let canceller = polkit::PolkitCanceller::new(pk_conn, handle.clone());
    let authenticator = polkit::PolkitAuthenticator::new(canceller.clone());
    // On invalidation (denial, disconnect, Purge) cancel exactly that check
    // (reviewer M4): the deadline passed into each check is the transaction's
    // remaining lease, so a stuck agent cannot outlive the slot.
    let hook_canceller = canceller.clone();
    let engine = Arc::new(
        Engine::new(Store::new(STATE_ROOT), SystemClock, authenticator).with_invalidate_hook(
            Arc::new(move |_uid, id: Option<String>| {
                if let Some(id) = id {
                    hook_canceller.cancel_id(&id);
                }
            }),
        ),
    );
    let activity = Arc::new(Activity {
        inflight: AtomicUsize::new(0),
        last_done: AtomicU64::new(SystemClock.now_secs()),
    });

    let service = Service {
        engine: engine.clone(),
        activity: activity.clone(),
        handle,
    };
    // Build the connection and export the object, but do NOT let the builder
    // request the name (reviewer M5): request it ourselves without replacement
    // and require primary ownership, so a second instance cannot take over.
    let conn = zbus::connection::Builder::system()?
        .serve_at(OBJECT_PATH, service)?
        .build()
        .await?;

    // Establish the disconnect subscription BEFORE the name is published
    // (reviewer #3), so an early Prepare-then-disconnect cannot be missed. The
    // NameOwnerChanged match rule is installed when the stream is created; the
    // stream buffers events until the watcher task drains it.
    let dbus = fdo::DBusProxy::new(&conn).await?;
    let owner_changed = dbus.receive_name_owner_changed().await?;
    let mut name_lost = dbus.receive_name_lost().await?;
    tokio::spawn(run_disconnect_watcher(owner_changed, engine.clone()));

    let reply = conn
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?;
    if reply != RequestNameReply::PrimaryOwner {
        return Err(format!("another relaunchd owns {BUS_NAME}").into());
    }

    // Exit on name loss, or when idle with no in-flight call and no live lease.
    loop {
        let tick = tokio::time::sleep(std::time::Duration::from_secs(IDLE_EXIT_SECS));
        tokio::select! {
            _ = {
                use futures_util::StreamExt;
                name_lost.next()
            } => break,
            () = tick => {
                let idle = activity.inflight.load(Ordering::SeqCst) == 0
                    && !engine.any_pending()
                    && SystemClock.now_secs()
                        .saturating_sub(activity.last_done.load(Ordering::SeqCst))
                        >= IDLE_EXIT_SECS;
                if idle {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_op_errors_echo_nothing_from_the_request() {
        let long = "SECRET".repeat(100_000);
        assert_eq!(
            parse_reduce_ops(vec![(long.clone(), "c".into())]).unwrap_err(),
            "unknown reduce op"
        );
        assert_eq!(
            parse_reduce_ops(vec![("drop".into(), long)]).unwrap_err(),
            "class too long"
        );
        assert_eq!(
            parse_reduce_ops(vec![
                ("disable".into(), "a".into()),
                ("drop".into(), "b".into())
            ])
            .unwrap(),
            vec![ReduceOp::Disable("a".into()), ReduceOp::Drop("b".into())]
        );
    }
}
