//! The authority engine: the single serialization point for Prepare, Authorize,
//! Reduce and Purge. Everything security-relevant lives here; the D-Bus layer
//! (chunk 3) is a thin adapter that supplies the caller uid/sender from bus
//! credentials and the polkit-backed `Authenticator`.
//!
//! Serialization is per-uid: a transaction is frozen under the per-uid lock,
//! the lock is **released across the human challenge**, and commit re-takes it
//! and re-validates liveness, base generation and enrollment id. A late auth
//! success after a Reduce/Purge/expiry therefore cannot sign (spec §5.2/§5.4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relaunch_core::{
    sign, to_config_file, SignedConfig, MAX_DOCUMENT_BYTES, SIGNED_SCHEMA_VERSION,
};

use crate::store::{Store, StoreError};
use crate::txn::{digest_of, new_txid, Clock, State, Transaction, DEFAULT_TTL_SECS};

/// One trusted-path check. `cancellation_id` identifies this exact check so it,
/// and only it, can be cancelled on invalidation; `deadline` is the
/// transaction's *remaining* lease, not a fresh timeout (reviewer M4/#2).
pub struct AuthRequest<'a> {
    /// What the human is asked to allow; each is its own polkit action.
    pub action: AuthAction,
    /// The caller's uid, from bus credentials.
    pub uid: u32,
    /// The caller's unique bus name, the polkit subject.
    pub sender: &'a str,
    /// The short `polkit.message`: the plain-language summary, bounded; never
    /// exec text.
    pub message: &'a str,
    /// Identifies this check, so exactly it can be cancelled.
    pub cancellation_id: &'a str,
    /// The transaction's remaining lease.
    pub deadline: Duration,
}

/// The privileged operations a credential check guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthAction {
    /// Commit a change that can make something new start at login.
    Authorize,
    /// Delete the caller's signing key and state.
    Purge,
}

/// The `polkit.message` of a Purge check.
pub const PURGE_MESSAGE: &str = "Relaunch: remove your login-launch signing key and saved list?";
/// How long a Purge check may wait for the human; Purge has no transaction
/// lease to inherit.
pub const PURGE_DEADLINE: Duration = Duration::from_secs(120);

/// Runs the trusted-path check (polkit `AUTH_SELF`). Returns Ok(true) only on a
/// real credential success for the caller's uid.
pub trait Authenticator {
    /// Run the check.
    ///
    /// # Errors
    ///
    /// A transport failure, timeout, or cancellation; never a credential
    /// denial, which is `Ok(false)`.
    fn authorize(&self, req: AuthRequest<'_>) -> Result<bool, String>;
}

/// Hook fired when a uid's pending transaction is invalidated, carrying the
/// in-flight check's cancellation id (if the challenge had been dispatched), so
/// the transport can cancel exactly that polkit check.
pub type InvalidateHook = Arc<dyn Fn(u32, Option<String>) + Send + Sync>;

/// Why an engine operation was refused. Nothing is signed on any of these.
#[derive(Debug)]
pub enum EngineError {
    /// A transaction for this uid is already open or authorizing.
    Busy,
    /// The uid has no committed key.
    NotEnrolled,
    /// The credential check failed.
    AuthDenied,
    /// The credential check could not run (or entropy was unavailable).
    AuthError(String),
    /// The lease expired, or the base generation or enrollment moved.
    Stale,
    /// No open transaction matches the txid.
    NoSuchTransaction,
    /// Authorize came from a different connection than Prepare.
    WrongSender,
    /// The resulting document failed structural validation.
    Validation(relaunch_core::ValidationError),
    /// The review diff exceeded a D7 cap.
    Diff(crate::diff::DiffError),
    /// The key store failed.
    Store(StoreError),
    /// The supplied base failed verification.
    Verify(String),
    /// A reduce op named a class not in the verified base.
    BadOp(String),
    /// The generation counter cannot advance.
    GenExhausted,
    /// The resulting document exceeds the size limit (its size).
    DocumentTooLarge(usize),
}

/// Lock that recovers from poisoning instead of propagating a panic into the
/// root service (reviewer I12). The guarded state is a small `Option` whose
/// consistency the post-challenge rechecks re-establish, so continuing is safe.
fn plock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl From<StoreError> for EngineError {
    fn from(e: StoreError) -> Self {
        EngineError::Store(e)
    }
}
impl From<relaunch_core::ValidationError> for EngineError {
    fn from(e: relaunch_core::ValidationError) -> Self {
        EngineError::Validation(e)
    }
}
impl From<crate::diff::DiffError> for EngineError {
    fn from(e: crate::diff::DiffError) -> Self {
        EngineError::Diff(e)
    }
}

/// A client's proposed launch set, before identity binding.
pub use relaunch_core::Proposal;

/// A reducing operation on a verified base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReduceOp {
    /// Set the class's entry to disabled.
    Disable(String),
    /// Remove the class's entry.
    Drop(String),
}

/// What Prepare returns to the preparing connection only (§4).
pub struct PrepareOk {
    /// None when the diff is empty (nothing to authorize).
    pub txid: Option<[u8; 16]>,
    /// The pairing code; `None` when the diff is empty. Carried to the
    /// preparing connection only; no review surface shows it.
    pub code: Option<String>,
    /// What the change means, in plain language (the review's headline).
    pub summary: String,
    /// The escaped review diff (the review's Details).
    pub rendered_diff: String,
    /// How many entries changed.
    pub changed: usize,
}

#[derive(Default)]
struct Cell {
    pending: Option<Transaction>,
}

/// The single serialization point for every per-uid operation (spec §5.2).
pub struct Engine<C: Clock, A: Authenticator> {
    store: Store,
    clock: C,
    auth: A,
    ttl_secs: u64,
    doc_limit: usize,
    cells: Mutex<HashMap<u32, Arc<Mutex<Cell>>>>,
    /// Called when a uid's pending transaction is invalidated (denial,
    /// disconnect, Purge), with the in-flight check's cancellation id, so the
    /// transport can cancel exactly that polkit check (reviewer M4). None in tests.
    on_invalidate: Option<InvalidateHook>,
}

impl<C: Clock, A: Authenticator> Engine<C, A> {
    /// An engine over `store` with the default lease and document limit.
    pub fn new(store: Store, clock: C, auth: A) -> Self {
        Engine {
            store,
            clock,
            auth,
            ttl_secs: DEFAULT_TTL_SECS,
            doc_limit: MAX_DOCUMENT_BYTES,
            cells: Mutex::new(HashMap::new()),
            on_invalidate: None,
        }
    }

    /// Use a different transaction lease length.
    #[must_use]
    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }

    /// Test seam: lower the resulting-document size limit so the rejection path
    /// can be exercised without building a multi-megabyte proposal.
    #[must_use]
    pub fn with_doc_limit(mut self, limit: usize) -> Self {
        self.doc_limit = limit;
        self
    }

    /// Run `hook` when a pending transaction is invalidated.
    #[must_use]
    pub fn with_invalidate_hook(mut self, hook: InvalidateHook) -> Self {
        self.on_invalidate = Some(hook);
        self
    }

    fn fire_invalidate(&self, uid: u32, check_id: Option<String>) {
        if let Some(h) = &self.on_invalidate {
            h(uid, check_id);
        }
    }

    fn cell(&self, uid: u32) -> Arc<Mutex<Cell>> {
        let mut map = plock(&self.cells);
        map.entry(uid).or_default().clone()
    }

    /// Current committed generation and enrollment id for a uid, or (0, "") if
    /// not yet enrolled. Used both to verify a base and to bind a transaction.
    fn head(&self, uid: u32) -> Result<(u64, String, Option<[u8; 32]>), EngineError> {
        match self.store.current(uid)? {
            Some(st) => {
                let mut pk = [0u8; 32];
                hex::decode_to_slice(&st.pubkey, &mut pk)
                    .map_err(|_| EngineError::Store(StoreError::Corrupt("pubkey".into())))?;
                Ok((st.gen, st.enrollment_id, Some(pk)))
            }
            None => Ok((0, String::new(), None)),
        }
    }

    /// Verify a client-supplied base document against this uid's committed key,
    /// generation and account. Any failure yields an empty base, so a tampered
    /// or planted base shows every row as an addition in the diff.
    fn verified_base(&self, uid: u32, user: &str, base_json: Option<&[u8]>) -> SignedConfig {
        let empty = SignedConfig {
            v: SIGNED_SCHEMA_VERSION,
            uid,
            user: user.to_string(),
            gen: 0,
            pre_launch_ms: 0,
            post_launch_ms: 0,
            entries: vec![],
        };
        let Some(json) = base_json else { return empty };
        let Ok((gen, _eid, Some(pk))) = self.head(uid) else {
            return empty;
        };
        let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&pk) else {
            return empty;
        };
        match relaunch_core::parse_and_verify(json, &vk, uid, user, gen) {
            Ok(cfg) => cfg,
            Err(_) => empty,
        }
    }

    fn build_proposal(
        uid: u32,
        user: &str,
        gen: u64,
        p: Proposal,
    ) -> Result<SignedConfig, EngineError> {
        Ok(SignedConfig::from_proposal(uid, user, gen, p)?)
    }

    /// Freeze a transaction for `proposal` relative to the verified base.
    ///
    /// # Errors
    ///
    /// An [`EngineError`] if a transaction is already open, the proposal or
    /// resulting document is invalid or too large, the diff exceeds a cap, or the key
    /// store fails.
    pub fn prepare(
        &self,
        uid: u32,
        user: &str,
        sender: &str,
        base_json: Option<&[u8]>,
        proposal: Proposal,
    ) -> Result<PrepareOk, EngineError> {
        let cell = self.cell(uid);
        let mut guard = plock(&cell);

        // Drop an expired lease so it cannot wedge the one-per-uid slot.
        if let Some(t) = &guard.pending {
            if t.expired_at(self.clock.now_secs()) {
                guard.pending = None;
            } else {
                return Err(EngineError::Busy);
            }
        }

        let (base_gen, enrollment_id, _pk) = self.head(uid)?;
        let next_gen = base_gen.checked_add(1).ok_or(EngineError::GenExhausted)?;
        let base = self.verified_base(uid, user, base_json);
        let cfg = Self::build_proposal(uid, user, next_gen, proposal)?;
        let digest = digest_of(&cfg);
        let digest_short = hex::encode(&digest[..4]);
        let diff = crate::diff::compute(&base, &cfg, &digest_short)?;
        if !diff.has_change {
            return Ok(PrepareOk {
                txid: None,
                code: None,
                summary: diff.summary,
                rendered_diff: diff.rendered,
                changed: 0,
            });
        }

        let txid = new_txid().map_err(EngineError::AuthError)?;
        let txn = Transaction {
            txid,
            uid,
            user: user.to_string(),
            enrollment_id,
            sender: sender.to_string(),
            base_gen,
            digest,
            proposal: cfg,
            prompt: diff.prompt,
            created_secs: self.clock.now_secs(),
            ttl_secs: self.ttl_secs,
            state: State::Prepared,
            check_id: None,
        };
        let code = txn.code();
        guard.pending = Some(txn);
        Ok(PrepareOk {
            txid: Some(txid),
            code: Some(code),
            summary: diff.summary,
            rendered_diff: diff.rendered,
            changed: diff.changed,
        })
    }

    /// Authorize and commit a prepared transaction. The lock is released across
    /// the challenge; commit re-validates everything.
    ///
    /// # Errors
    ///
    /// An [`EngineError`] if no matching live transaction exists, the sender
    /// differs, the check is denied or fails, the state moved meanwhile, or the commit
    /// fails.
    pub fn authorize(
        &self,
        uid: u32,
        sender: &str,
        txid: &[u8; 16],
    ) -> Result<Vec<u8>, EngineError> {
        let cell = self.cell(uid);

        // Phase 1: claim the transaction under the lock. The check's
        // cancellation id is recorded in the txn BEFORE the lock is released, so
        // any invalidation that arrives while the challenge runs can cancel
        // exactly this check (reviewer M4/#2).
        // Same entropy source as the txid; a failure refuses the Authorize rather
        // than minting a predictable (all-zero) id that could collide.
        let check_id = hex::encode(new_txid().map_err(EngineError::AuthError)?);
        let (message, deadline) = {
            let mut guard = plock(&cell);
            let now = self.clock.now_secs();
            let t = guard
                .pending
                .as_mut()
                .ok_or(EngineError::NoSuchTransaction)?;
            if &t.txid != txid {
                return Err(EngineError::NoSuchTransaction);
            }
            if t.expired_at(now) {
                guard.pending = None;
                return Err(EngineError::Stale);
            }
            if t.sender != sender {
                return Err(EngineError::WrongSender);
            }
            if t.state != State::Prepared {
                return Err(EngineError::Busy); // an Authorize is already in flight
            }
            t.state = State::Authorizing;
            t.check_id = Some(check_id.clone());
            // The challenge gets the transaction's REMAINING lease, not a fresh
            // full timeout, so a stuck agent cannot outlive the slot.
            let elapsed = now.saturating_sub(t.created_secs);
            let remaining = t.ttl_secs.saturating_sub(elapsed).max(1);
            (t.prompt.clone(), Duration::from_secs(remaining))
        };

        // Phase 2: the human challenge, WITHOUT holding the lock.
        let req = AuthRequest {
            action: AuthAction::Authorize,
            uid,
            sender,
            message: &message,
            cancellation_id: &check_id,
            deadline,
        };
        // The check has already returned on these paths, so only free the slot;
        // do NOT fire a cancellation (the id is retired, cancelling it would
        // only leak into the pre-dispatch set — reviewer #3).
        let ok = self.auth.authorize(req).map_err(|e| {
            Self::invalidate(&cell, txid);
            EngineError::AuthError(e)
        })?;
        if !ok {
            Self::invalidate(&cell, txid);
            return Err(EngineError::AuthDenied);
        }

        // Phase 3: re-take the lock and re-validate before signing.
        let mut guard = plock(&cell);
        let now = self.clock.now_secs();
        let txn = match guard.pending.take() {
            Some(t) if &t.txid == txid && t.state == State::Authorizing && !t.expired_at(now) => t,
            // Gone, replaced, expired, or invalidated by Reduce/Purge meanwhile.
            other => {
                guard.pending = other.filter(|t| &t.txid != txid);
                return Err(EngineError::Stale);
            }
        };

        // Base generation and enrollment must still be exactly what we froze.
        let (cur_gen, cur_eid, _pk) = self.head(uid)?;
        if cur_gen != txn.base_gen || cur_eid != txn.enrollment_id {
            return Err(EngineError::Stale);
        }

        let new_gen = txn
            .base_gen
            .checked_add(1)
            .ok_or(EngineError::GenExhausted)?;
        self.commit(uid, &txn.user, new_gen, txn.proposal)
    }

    /// Reduce: disable/drop only, on a verified base. No prompt (`allow_any`).
    /// Invalidates any pending transaction first.
    ///
    /// # Errors
    ///
    /// An [`EngineError`] if the base does not verify, an op names an unknown
    /// class, or the commit fails.
    pub fn reduce(
        &self,
        uid: u32,
        user: &str,
        base_json: &[u8],
        ops: &[ReduceOp],
    ) -> Result<Vec<u8>, EngineError> {
        let cell = self.cell(uid);
        let mut guard = plock(&cell);
        // Invalidate any in-flight transaction and cancel its polkit check
        // (reviewer D3), same as Purge does.
        let cancelled = guard.pending.take().and_then(|t| t.check_id);

        let result = (|| {
            let (gen, _eid, Some(pk)) = self.head(uid)? else {
                return Err(EngineError::NotEnrolled);
            };
            let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk)
                .map_err(|_| EngineError::Verify("pubkey".into()))?;
            let base = relaunch_core::parse_and_verify(base_json, &vk, uid, user, gen)
                .map_err(|e| EngineError::Verify(format!("{e:?}")))?;

            let mut entries = base.entries;
            for op in ops {
                match op {
                    ReduceOp::Disable(class) => {
                        let e = entries
                            .iter_mut()
                            .find(|e| &e.class == class)
                            .ok_or_else(|| EngineError::BadOp(class.clone()))?;
                        e.enabled = false;
                    }
                    ReduceOp::Drop(class) => {
                        let before = entries.len();
                        entries.retain(|e| &e.class != class);
                        if entries.len() == before {
                            return Err(EngineError::BadOp(class.clone()));
                        }
                    }
                }
            }
            let new_gen = gen.checked_add(1).ok_or(EngineError::GenExhausted)?;
            let cfg = SignedConfig {
                v: SIGNED_SCHEMA_VERSION,
                uid,
                user: user.to_string(),
                gen: new_gen,
                pre_launch_ms: base.pre_launch_ms,
                post_launch_ms: base.post_launch_ms,
                entries,
            };
            self.commit(uid, user, new_gen, cfg)
        })();
        drop(guard);
        self.fire_invalidate(uid, cancelled);
        result
    }

    /// Purge: after the caller authenticates as themselves (its own polkit
    /// action, always prompting: deleting the key is destructive), invalidate
    /// any pending transaction, then durably delete the uid's key and state. A
    /// denied or failed check touches nothing. The per-uid guard is held
    /// THROUGH `Store::purge` (reviewer #4), so a same-uid Prepare/commit cannot
    /// interleave with the filesystem mutation and leave a pending txn against
    /// a just-removed enrollment.
    ///
    /// # Errors
    ///
    /// [`EngineError::AuthDenied`] / [`EngineError::AuthError`] if the check
    /// refuses or cannot run; a store failure while deleting the uid's state.
    pub fn purge(&self, uid: u32, sender: &str) -> Result<(), EngineError> {
        let check_id = hex::encode(new_txid().map_err(EngineError::AuthError)?);
        let req = AuthRequest {
            action: AuthAction::Purge,
            uid,
            sender,
            message: PURGE_MESSAGE,
            cancellation_id: &check_id,
            deadline: PURGE_DEADLINE,
        };
        if !self.auth.authorize(req).map_err(EngineError::AuthError)? {
            return Err(EngineError::AuthDenied);
        }
        let cell = self.cell(uid);
        let (result, cancelled) = {
            let mut guard = plock(&cell);
            let cancelled = guard.pending.take().and_then(|t| t.check_id);
            let result = self.store.purge(uid).map_err(EngineError::from);
            (result, cancelled)
        };
        self.fire_invalidate(uid, cancelled); // also on the error path
        result
    }

    /// Whether a (non-expired) transaction is open for this uid. Deliberately
    /// returns no code/diff/txid (spec §4: no pending-code leak).
    pub fn has_pending(&self, uid: u32) -> bool {
        let cell = self.cell(uid);
        let guard = plock(&cell);
        guard
            .pending
            .as_ref()
            .is_some_and(|t| !t.expired_at(self.clock.now_secs()))
    }

    /// True if any uid has a live (non-expired) transaction. The idle-exit
    /// timer consults this so the service never exits mid-lease/commit.
    pub fn any_pending(&self) -> bool {
        let now = self.clock.now_secs();
        let map = plock(&self.cells);
        map.values().any(|c| {
            plock(c)
                .pending
                .as_ref()
                .is_some_and(|t| !t.expired_at(now))
        })
    }

    fn invalidate(cell: &Arc<Mutex<Cell>>, txid: &[u8; 16]) {
        let mut guard = plock(cell);
        if let Some(t) = &guard.pending {
            if &t.txid == txid {
                guard.pending = None;
            }
        }
    }

    /// Invalidate any pending transaction whose Prepare came from `sender`.
    /// Called when that unique bus connection disappears (reviewer I7): the
    /// one-per-uid slot is freed at once, and a late auth success can no longer
    /// find its transaction to sign.
    pub fn invalidate_sender(&self, sender: &str) {
        let cells: Vec<Arc<Mutex<Cell>>> = plock(&self.cells).values().cloned().collect();
        for cell in cells {
            let hit = {
                let mut guard = plock(&cell);
                match &guard.pending {
                    Some(t) if t.sender == sender => {
                        let uid = t.uid;
                        let check_id = guard.pending.take().and_then(|t| t.check_id);
                        Some((uid, check_id))
                    }
                    _ => None,
                }
            };
            if let Some((uid, check_id)) = hit {
                self.fire_invalidate(uid, check_id);
            }
        }
    }

    /// Sign `cfg` (gen already set), publish the new generation durably, and
    /// only then return the serialized `config.json`. Publishing before
    /// returning the signature is what makes a crash fail closed (spec §7).
    fn commit(
        &self,
        uid: u32,
        user: &str,
        new_gen: u64,
        mut cfg: SignedConfig,
    ) -> Result<Vec<u8>, EngineError> {
        cfg.gen = new_gen;
        cfg.validate()?;
        // ensure_enrolled creates the key on first use and recovers a key-only
        // state; it never lowers a committed gen.
        self.store.ensure_enrolled(uid, user)?;
        let key = self.store.signing_key(uid)?;
        let pubkey = key.verifying_key().to_bytes();
        let sig = sign(&key, &cfg);
        let file = to_config_file(&cfg, &sig, vec![]);
        let bytes = serde_json::to_vec(&file).map_err(|e| EngineError::Verify(e.to_string()))?;
        // Refuse to COMMIT a document the anchor would then refuse to read
        // (reviewer #1). Checked before publish_gen, so a rejected oversize
        // change does not advance the generation.
        if bytes.len() > self.doc_limit {
            return Err(EngineError::DocumentTooLarge(bytes.len()));
        }
        self.store.publish_gen(uid, user, new_gen, &pubkey)?;
        Ok(bytes)
    }
}
