//! The polkit-backed `Authenticator`: the trusted-path credential check, with a
//! cancellation id and a lease-aligned deadline (reviewer M4).
//!
//! `CheckAuthorization` names a `system-bus-name` subject = the *caller's* unique
//! connection, so polkit resolves the subject to the calling process and runs
//! AUTH_SELF for that uid (fingerprint, then password — the installed polkit-1
//! PAM stack). The `00-` rule (spec §3) denies non-local/inactive callers first.
//!
//! The call is driven from the sync `Authenticator` trait via `handle.block_on`
//! on a `spawn_blocking` thread (not a runtime worker, so blocking is allowed),
//! and is bounded by a timeout equal to the transaction lease so a stuck or
//! malicious agent cannot pin the thread and the uid's one-transaction slot
//! indefinitely. Each call carries a unique cancellation id; `PolkitCanceller`
//! lets the engine fire `CancelCheckAuthorization` for a uid when its pending
//! transaction is invalidated (Purge, disconnect, denial). The polkit dialog is
//! NOT a trusted display surface (spec §1/D6); the panel code is the human
//! cross-check, this message is only a pointer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zbus::zvariant::{OwnedValue, Type, Value};
use zbus::Connection;

use crate::engine::{AuthRequest, Authenticator};

/// Bound on the remembered pre-dispatch cancellation ids. When full, the OLDEST
/// is evicted (never a bulk clear), so a still-live pre-dispatch cancellation is
/// not dropped wholesale (reviewer #3).
const MAX_CANCELLED_IDS: usize = 4096;

/// `(sa{sv})` — a polkit subject.
#[derive(Serialize, Type)]
struct Subject {
    kind: String,
    details: HashMap<String, OwnedValue>,
}

/// `(bba{ss})` — a polkit AuthorizationResult.
#[derive(Deserialize, Type)]
struct AuthzResult {
    is_authorized: bool,
    #[allow(dead_code)]
    is_challenge: bool,
    #[allow(dead_code)]
    details: HashMap<String, String>,
}

const AUTHORITY_DEST: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// CheckAuthorizationFlags::AllowUserInteraction.
const ALLOW_USER_INTERACTION: u32 = 0x1;

/// Owns the system-bus connection used for polkit and runs/cancels checks by
/// their cancellation id. Cancellation is keyed on the id, not the uid
/// (reviewer M4/#2): completing one check never disturbs another, and an id
/// cancelled before its check dispatches is remembered in `cancelled`. Shared
/// (Arc) between the `Authenticator` and the engine's invalidation hook.
struct Coord {
    /// Checks currently dispatched (id -> its abort signal). An entry is removed
    /// when the check completes (retire only COMPLETED ids, reviewer #3).
    inflight: HashMap<String, Arc<Notify>>,
    /// Cancellations that arrived before their check registered. Bounded; oldest
    /// evicted, never bulk-cleared.
    pre: HashSet<String>,
    order: VecDeque<String>,
}

pub struct PolkitCanceller {
    bus: Connection,
    handle: tokio::runtime::Handle,
    action_id: String,
    coord: Mutex<Coord>,
}

/// Removes an in-flight registration on drop, so a panic in the check cannot
/// leave a stale id (reviewer D1).
struct InflightGuard<'a> {
    canceller: &'a PolkitCanceller,
    id: String,
}
impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.canceller.lock_coord().inflight.remove(&self.id);
    }
}

impl PolkitCanceller {
    pub fn new(bus: Connection, handle: tokio::runtime::Handle, action_id: &str) -> Arc<Self> {
        Arc::new(PolkitCanceller {
            bus,
            handle,
            action_id: action_id.to_string(),
            coord: Mutex::new(Coord {
                inflight: HashMap::new(),
                pre: HashSet::new(),
                order: VecDeque::new(),
            }),
        })
    }

    fn lock_coord(&self) -> std::sync::MutexGuard<'_, Coord> {
        self.coord.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register a check as in-flight, unless it was already cancelled
    /// pre-dispatch. Returns its abort `Notify`, or `None` if cancelled.
    fn register(&self, id: &str) -> Option<Arc<Notify>> {
        let mut c = self.lock_coord();
        if c.pre.remove(id) {
            return None;
        }
        let n = Arc::new(Notify::new());
        c.inflight.insert(id.to_string(), n.clone());
        Some(n)
    }

    async fn check(&self, sender: &str, message: &str, id: &str) -> Result<bool, String> {
        let name: OwnedValue = Value::new(sender.to_string())
            .try_into()
            .map_err(|e| format!("{e}"))?;
        let mut subject_details = HashMap::new();
        subject_details.insert("name".to_string(), name);
        let subject = Subject {
            kind: "system-bus-name".to_string(),
            details: subject_details,
        };

        let mut details: HashMap<&str, &str> = HashMap::new();
        details.insert("polkit.message", message);

        let reply = self
            .bus
            .call_method(
                Some(AUTHORITY_DEST),
                AUTHORITY_PATH,
                Some(AUTHORITY_IFACE),
                "CheckAuthorization",
                &(
                    subject,
                    self.action_id.as_str(),
                    details,
                    ALLOW_USER_INTERACTION,
                    id,
                ),
            )
            .await
            .map_err(|e| format!("polkit CheckAuthorization: {e}"))?;
        let result: AuthzResult = reply.body().deserialize().map_err(|e| format!("{e}"))?;
        Ok(result.is_authorized)
    }

    /// Run a bounded, cancellable check for the request's cancellation id, using
    /// the request's remaining-lease deadline. Blocks the calling
    /// (spawn_blocking) thread until the result, the deadline, or a cancel. The
    /// abort `Notify` is armed under the lock BEFORE dispatch, so a cancel that
    /// arrives at any point from registration onward aborts the check rather
    /// than letting it run to the deadline (reviewer #2).
    fn authorize_blocking(&self, req: &AuthRequest) -> Result<bool, String> {
        let id = req.cancellation_id;
        let notify = match self.register(id) {
            Some(n) => n,
            None => return Err("authorization cancelled".into()),
        };
        let _guard = InflightGuard {
            canceller: self,
            id: id.to_string(),
        };
        self.handle.block_on(async {
            tokio::select! {
                biased;
                _ = notify.notified() => Err("authorization cancelled".into()),
                r = tokio::time::timeout(req.deadline, self.check(req.sender, req.message, id)) => {
                    match r {
                        Ok(inner) => inner,
                        Err(_timeout) => {
                            self.spawn_cancel(id.to_string());
                            Err("authorization timed out".into())
                        }
                    }
                }
            }
        })
        // _guard drops here, retiring the completed id from `inflight`.
    }

    /// Cancel a specific check id. If the check is in flight, fire its abort
    /// signal and tell polkit; if it has not registered yet, remember the
    /// cancellation (bounded, oldest-evicted) so it aborts on registration. A
    /// post-completion id (no registration, already retired) is remembered only
    /// briefly and evicted — callers skip this for a check already returned.
    pub fn cancel_id(&self, id: &str) {
        let in_flight = {
            let mut c = self.lock_coord();
            if let Some(n) = c.inflight.get(id).cloned() {
                n.notify_one();
                true
            } else {
                if !c.pre.contains(id) {
                    if c.order.len() >= MAX_CANCELLED_IDS {
                        if let Some(old) = c.order.pop_front() {
                            c.pre.remove(&old);
                        }
                    }
                    c.pre.insert(id.to_string());
                    c.order.push_back(id.to_string());
                }
                false
            }
        };
        // Tell polkit regardless (harmless if it never registered the id).
        let _ = in_flight;
        self.spawn_cancel(id.to_string());
    }

    fn spawn_cancel(&self, id: String) {
        let bus = self.bus.clone();
        self.handle.spawn(async move {
            let _ = bus
                .call_method(
                    Some(AUTHORITY_DEST),
                    AUTHORITY_PATH,
                    Some(AUTHORITY_IFACE),
                    "CancelCheckAuthorization",
                    &(id,),
                )
                .await;
        });
    }
}

pub struct PolkitAuthenticator {
    canceller: Arc<PolkitCanceller>,
}

impl PolkitAuthenticator {
    pub fn new(canceller: Arc<PolkitCanceller>) -> Self {
        PolkitAuthenticator { canceller }
    }
}

impl Authenticator for PolkitAuthenticator {
    fn authorize(&self, req: AuthRequest) -> Result<bool, String> {
        self.canceller.authorize_blocking(&req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    #[test]
    fn subject_and_result_match_polkit_signatures() {
        // polkit Subject is (sa{sv}); AuthorizationResult is (bba{ss}).
        assert_eq!(Subject::SIGNATURE.to_string(), "(sa{sv})");
        assert_eq!(AuthzResult::SIGNATURE.to_string(), "(bba{ss})");
    }
}
