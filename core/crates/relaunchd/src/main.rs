//! relaunchd binary — the D-Bus transport around the authority engine.
//!
//! The service owns the per-uid key store under `/var/lib/omarchy-relaunch` and
//! is D-Bus activated as root. Every method derives the caller's uid and unique
//! sender from bus credentials (never from a request field), looks the account
//! name up from the password database, and runs the engine on a blocking thread
//! via a captured Tokio handle. Authorization is delegated to polkit
//! (`polkit::PolkitAuthenticator`). The name is held non-replaceably; a client
//! disconnect invalidates that uid's pending transaction; name loss exits.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use zbus::fdo;
use zbus::names::BusName;

use relaunch_core::Entry;
use relaunchd::engine::{Engine, EngineError, Proposal, ReduceOp};
use relaunchd::store::Store;
use relaunchd::txn::Clock;
use relaunchd::{polkit, users};

const STATE_ROOT: &str = "/var/lib/omarchy-relaunch";
const BUS_NAME: &str = "io.github.laytonf.Relaunch1";
const OBJECT_PATH: &str = "/io/github/laytonf/Relaunch1";
const ACTION_ID: &str = "io.github.laytonf.relaunch.authorize";
const IDLE_EXIT_SECS: u64 = 30;
/// Upper bound on a single request's byte-ish inputs, enforced BEFORE any parse
/// (reviewer I6/M2). It is the shared document limit plus envelope headroom, so
/// a just-signed document (up to MAX_DOCUMENT_BYTES) always round-trips as the
/// next Prepare/Reduce `base` (reviewer #1). D-Bus's own maximum is not an
/// application limit.
const MAX_REQUEST_BYTES: usize = relaunch_core::MAX_DOCUMENT_BYTES + 64 * 1024;
const MAX_REDUCE_OPS: usize = 512;
// Compile-time round-trip guarantee (reviewer #1): any document the service
// may sign fits a Prepare/Reduce base request.
const _: () = assert!(MAX_REQUEST_BYTES >= relaunch_core::MAX_DOCUMENT_BYTES);

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

/// Proposal as the bash client sends it (JSON). Identity (uid/user/gen) is set
/// by the service from bus credentials, never taken from the client.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalWire {
    #[serde(rename = "staggerSeconds", default)]
    stagger_seconds: u32,
    entries: Vec<Entry>,
}

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

fn eng_err(e: EngineError) -> fdo::Error {
    fdo::Error::Failed(format!("{e:?}"))
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
        let user = users::name_for_uid(uid)
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
        r.map_err(eng_err)
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
    ) -> fdo::Result<(Vec<u8>, String, String)> {
        if base.len() > MAX_REQUEST_BYTES || proposal_json.len() > MAX_REQUEST_BYTES {
            return Err(fdo::Error::InvalidArgs("request too large".into()));
        }
        let (uid, user, sender) = Self::caller(conn, &hdr).await?;
        let wire: ProposalWire = serde_json::from_slice(&proposal_json)
            .map_err(|e| fdo::Error::InvalidArgs(format!("proposal: {e}")))?;
        let proposal = Proposal {
            stagger_seconds: wire.stagger_seconds,
            entries: wire.entries,
        };
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
                    engine.invalidate_sender(&sender)
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
        let mut parsed = Vec::with_capacity(ops.len());
        for (kind, class) in ops {
            if class.len() > 256 {
                return Err(fdo::Error::InvalidArgs("class too long".into()));
            }
            parsed.push(match kind.as_str() {
                "disable" => ReduceOp::Disable(class),
                "drop" => ReduceOp::Drop(class),
                other => {
                    return Err(fdo::Error::InvalidArgs(format!(
                        "unknown reduce op {other}"
                    )))
                }
            });
        }
        self.run_blocking(move |eng| eng.reduce(uid, &user, &base, &parsed))
            .await
    }

    async fn purge(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> fdo::Result<()> {
        let (uid, _user, _sender) = Self::caller(conn, &hdr).await?;
        self.run_blocking(move |eng| eng.purge(uid)).await
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

/// Consume a NameOwnerChanged stream, invalidating a uid's pending transaction
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
                    engine.invalidate_sender(&name)
                }));
            }
        }
    }
    eprintln!("relaunchd: disconnect watcher stream ended");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handle = tokio::runtime::Handle::current();
    // A dedicated system-bus connection for polkit calls, created under this
    // runtime so the authenticator's block_on can drive it.
    let pk_conn = zbus::Connection::system().await?;
    let canceller = polkit::PolkitCanceller::new(pk_conn, handle.clone(), ACTION_ID);
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

    use zbus::fdo::{RequestNameFlags, RequestNameReply};
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
            _ = tick => {
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
