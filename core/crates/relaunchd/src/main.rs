//! relaunchd binary — the D-Bus transport around the authority engine.
//!
//! The service owns the per-uid key store under `/var/lib/omarchy-relaunch` and
//! is D-Bus activated as root. Every method derives the caller's uid and unique
//! sender from bus credentials (never from a request field), looks the account
//! name up from the password database, and runs the engine on a blocking thread.
//! Authorization is delegated to polkit (`polkit::PolkitAuthenticator`).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use zbus::fdo;

use relaunch_core::Entry;
use relaunchd::engine::{Engine, EngineError, Proposal, ReduceOp};
use relaunchd::store::Store;
use relaunchd::txn::Clock;
use relaunchd::{polkit, users};

const STATE_ROOT: &str = "/var/lib/omarchy-relaunch";
const BUS_NAME: &str = "io.github.laytonf.Relaunch1";
const OBJECT_PATH: &str = "/io/github/laytonf/Relaunch1";
const ACTION_ID: &str = "io.github.laytonf.relaunch.authorize";
/// Exit after this many seconds with no calls and no live transaction.
const IDLE_EXIT_SECS: u64 = 30;

struct SystemClock;
impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
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

impl Activity {
    fn enter(&self) {
        self.inflight.fetch_add(1, Ordering::SeqCst);
    }
    fn leave(&self) {
        self.last_done
            .store(SystemClock.now_secs(), Ordering::SeqCst);
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Service {
    engine: Arc<Eng>,
    activity: Arc<Activity>,
}

fn eng_err(e: EngineError) -> fdo::Error {
    fdo::Error::Failed(format!("{e:?}"))
}

impl Service {
    /// Resolve the caller's uid and account name from bus credentials.
    async fn caller(
        conn: &zbus::Connection,
        hdr: &zbus::message::Header<'_>,
    ) -> fdo::Result<(u32, String, String)> {
        let sender = hdr
            .sender()
            .ok_or_else(|| fdo::Error::Failed("no sender on message".into()))?
            .to_owned();
        let dbus = fdo::DBusProxy::new(conn).await?;
        let uid = dbus.get_connection_unix_user(sender.clone().into()).await?;
        let user = users::name_for_uid(uid)
            .ok_or_else(|| fdo::Error::Failed(format!("no account for uid {uid}")))?;
        Ok((uid, user, sender.as_str().to_string()))
    }

    fn run_blocking<T, F>(&self, f: F) -> impl std::future::Future<Output = fdo::Result<T>>
    where
        T: Send + 'static,
        F: FnOnce(&Eng) -> Result<T, EngineError> + Send + 'static,
    {
        let engine = self.engine.clone();
        let activity = self.activity.clone();
        async move {
            activity.enter();
            let r = tokio::task::spawn_blocking(move || f(&engine))
                .await
                .map_err(|e| fdo::Error::Failed(format!("task: {e}")))?;
            activity.leave();
            r.map_err(eng_err)
        }
    }
}

#[zbus::interface(name = "io.github.laytonf.Relaunch1")]
impl Service {
    /// Freeze a transaction and return (txid, code, rendered_diff). An empty
    /// diff returns an empty txid and code.
    async fn prepare(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        base: Vec<u8>,
        proposal_json: Vec<u8>,
    ) -> fdo::Result<(Vec<u8>, String, String)> {
        let (uid, user, sender) = Self::caller(conn, &hdr).await?;
        let wire: ProposalWire = serde_json::from_slice(&proposal_json)
            .map_err(|e| fdo::Error::InvalidArgs(format!("proposal: {e}")))?;
        let proposal = Proposal {
            stagger_seconds: wire.stagger_seconds,
            entries: wire.entries,
        };
        let base_opt = if base.is_empty() { None } else { Some(base) };
        self.run_blocking(move |eng| {
            let out = eng.prepare(uid, &user, &sender, base_opt.as_deref(), proposal)?;
            Ok((
                out.txid.map(|t| t.to_vec()).unwrap_or_default(),
                out.code.unwrap_or_default(),
                out.rendered_diff,
            ))
        })
        .await
    }

    /// Authorize and commit a prepared transaction; returns the signed config.json.
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

    /// Disable/drop only, on a verified base. No polkit prompt (allow_any).
    async fn reduce(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        base: Vec<u8>,
        ops: Vec<(String, String)>,
    ) -> fdo::Result<Vec<u8>> {
        let (uid, user, _sender) = Self::caller(conn, &hdr).await?;
        let mut parsed = Vec::with_capacity(ops.len());
        for (kind, class) in ops {
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

    /// Delete the caller's key and state, invalidating any pending transaction.
    async fn purge(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> fdo::Result<()> {
        let (uid, _user, _sender) = Self::caller(conn, &hdr).await?;
        self.run_blocking(move |eng| eng.purge(uid)).await
    }

    /// Whether a live transaction exists for the caller. Returns no code, diff
    /// or txid (spec §4: no pending-code leak).
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let authenticator = polkit::PolkitAuthenticator::connect(ACTION_ID)?;
    let engine = Arc::new(Engine::new(
        Store::new(STATE_ROOT),
        SystemClock,
        authenticator,
    ));
    let activity = Arc::new(Activity {
        inflight: AtomicUsize::new(0),
        last_done: AtomicU64::new(SystemClock.now_secs()),
    });

    let service = Service {
        engine: engine.clone(),
        activity: activity.clone(),
    };
    let _conn = zbus::connection::Builder::system()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, service)?
        .build()
        .await?;

    // Idle-exit: quit only when there is no in-flight call and no live
    // transaction, so a lease or a commit is never dropped.
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(IDLE_EXIT_SECS)).await;
        let idle = activity.inflight.load(Ordering::SeqCst) == 0
            && !engine.any_pending()
            && SystemClock
                .now_secs()
                .saturating_sub(activity.last_done.load(Ordering::SeqCst))
                >= IDLE_EXIT_SECS;
        if idle {
            break;
        }
    }
    Ok(())
}
