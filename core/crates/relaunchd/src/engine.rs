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

use relaunch_core::{sign, to_config_file, Entry, SignedConfig, SIGNED_SCHEMA_VERSION};

use crate::store::{Store, StoreError};
use crate::txn::{digest_of, new_txid, Clock, State, Transaction, DEFAULT_TTL_SECS};

/// Runs the trusted-path check (polkit AUTH_SELF). `sender` is the caller's
/// unique bus name for subject binding. Returns Ok(true) only on a real
/// credential success for `uid`.
pub trait Authenticator {
    fn authorize(&self, uid: u32, sender: &str, message: &str) -> Result<bool, String>;
}

#[derive(Debug)]
pub enum EngineError {
    Busy,
    NotEnrolled,
    AuthDenied,
    AuthError(String),
    Stale,
    NoSuchTransaction,
    WrongSender,
    Validation(relaunch_core::ValidationError),
    Diff(crate::diff::DiffError),
    Store(StoreError),
    Verify(String),
    BadOp(String),
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

pub struct Proposal {
    pub stagger_seconds: u32,
    pub entries: Vec<Entry>,
}

pub enum ReduceOp {
    Disable(String),
    Drop(String),
}

pub struct PrepareOk {
    /// None when the diff is empty (nothing to authorize).
    pub txid: Option<[u8; 16]>,
    pub code: Option<String>,
    pub rendered_diff: String,
    pub changed: usize,
}

#[derive(Default)]
struct Cell {
    pending: Option<Transaction>,
}

pub struct Engine<C: Clock, A: Authenticator> {
    store: Store,
    clock: C,
    auth: A,
    ttl_secs: u64,
    cells: Mutex<HashMap<u32, Arc<Mutex<Cell>>>>,
}

impl<C: Clock, A: Authenticator> Engine<C, A> {
    pub fn new(store: Store, clock: C, auth: A) -> Self {
        Engine {
            store,
            clock,
            auth,
            ttl_secs: DEFAULT_TTL_SECS,
            cells: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }

    fn cell(&self, uid: u32) -> Arc<Mutex<Cell>> {
        let mut map = self.cells.lock().unwrap();
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
            stagger_seconds: 0,
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
        &self,
        uid: u32,
        user: &str,
        gen: u64,
        p: Proposal,
    ) -> Result<SignedConfig, EngineError> {
        let mut cfg = SignedConfig {
            v: SIGNED_SCHEMA_VERSION,
            uid,
            user: user.to_string(),
            gen,
            stagger_seconds: p.stagger_seconds,
            entries: p.entries,
        };
        cfg.sort_entries();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Freeze a transaction for `proposal` relative to the verified base.
    pub fn prepare(
        &self,
        uid: u32,
        user: &str,
        sender: &str,
        base_json: Option<&[u8]>,
        proposal: Proposal,
    ) -> Result<PrepareOk, EngineError> {
        let cell = self.cell(uid);
        let mut guard = cell.lock().unwrap();

        // Drop an expired lease so it cannot wedge the one-per-uid slot.
        if let Some(t) = &guard.pending {
            if t.expired_at(self.clock.now_secs()) {
                guard.pending = None;
            } else {
                return Err(EngineError::Busy);
            }
        }

        let (base_gen, enrollment_id, _pk) = self.head(uid)?;
        let base = self.verified_base(uid, user, base_json);
        let cfg = self.build_proposal(uid, user, base_gen + 1, proposal)?;
        let diff = crate::diff::compute(&base, &cfg)?;
        if diff.changed == 0 {
            return Ok(PrepareOk {
                txid: None,
                code: None,
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
            digest: digest_of(&cfg),
            proposal: cfg,
            changed: diff.changed,
            created_secs: self.clock.now_secs(),
            ttl_secs: self.ttl_secs,
            state: State::Prepared,
        };
        let code = txn.code();
        guard.pending = Some(txn);
        Ok(PrepareOk {
            txid: Some(txid),
            code: Some(code),
            rendered_diff: diff.rendered,
            changed: diff.changed,
        })
    }

    /// Authorize and commit a prepared transaction. The lock is released across
    /// the challenge; commit re-validates everything.
    pub fn authorize(
        &self,
        uid: u32,
        sender: &str,
        txid: &[u8; 16],
    ) -> Result<Vec<u8>, EngineError> {
        let cell = self.cell(uid);

        // Phase 1: claim the transaction under the lock.
        let (message, _base_gen) = {
            let mut guard = cell.lock().unwrap();
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
            (
                format!(
                    "Relaunch: {} login-start change(s). Cancel unless your panel shows code {}.",
                    t.changed,
                    t.code()
                ),
                t.base_gen,
            )
        };

        // Phase 2: the human challenge, WITHOUT holding the lock.
        let ok = self.auth.authorize(uid, sender, &message).map_err(|e| {
            self.invalidate(&cell, txid);
            EngineError::AuthError(e)
        })?;
        if !ok {
            self.invalidate(&cell, txid);
            return Err(EngineError::AuthDenied);
        }

        // Phase 3: re-take the lock and re-validate before signing.
        let mut guard = cell.lock().unwrap();
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

        self.commit(uid, &txn.user, txn.base_gen + 1, txn.proposal)
    }

    /// Reduce: disable/drop only, on a verified base. No prompt (allow_any).
    /// Invalidates any pending transaction first.
    pub fn reduce(
        &self,
        uid: u32,
        user: &str,
        base_json: &[u8],
        ops: &[ReduceOp],
    ) -> Result<Vec<u8>, EngineError> {
        let cell = self.cell(uid);
        let mut guard = cell.lock().unwrap();
        guard.pending = None; // invalidate any in-flight transaction

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
        let cfg = SignedConfig {
            v: SIGNED_SCHEMA_VERSION,
            uid,
            user: user.to_string(),
            gen: gen + 1,
            stagger_seconds: base.stagger_seconds,
            entries,
        };
        self.commit(uid, user, gen + 1, cfg)
    }

    /// Purge: invalidate any pending transaction, then durably delete the uid's
    /// key and state.
    pub fn purge(&self, uid: u32) -> Result<(), EngineError> {
        let cell = self.cell(uid);
        let mut guard = cell.lock().unwrap();
        guard.pending = None;
        self.store.purge(uid)?;
        Ok(())
    }

    /// Whether a (non-expired) transaction is open for this uid. Deliberately
    /// returns no code/diff/txid (spec §4: no pending-code leak).
    pub fn has_pending(&self, uid: u32) -> bool {
        let cell = self.cell(uid);
        let guard = cell.lock().unwrap();
        guard
            .pending
            .as_ref()
            .is_some_and(|t| !t.expired_at(self.clock.now_secs()))
    }

    /// True if any uid has a live (non-expired) transaction. The idle-exit
    /// timer consults this so the service never exits mid-lease/commit.
    pub fn any_pending(&self) -> bool {
        let now = self.clock.now_secs();
        let map = self.cells.lock().unwrap();
        map.values().any(|c| {
            c.lock()
                .unwrap()
                .pending
                .as_ref()
                .is_some_and(|t| !t.expired_at(now))
        })
    }

    fn invalidate(&self, cell: &Arc<Mutex<Cell>>, txid: &[u8; 16]) {
        let mut guard = cell.lock().unwrap();
        if let Some(t) = &guard.pending {
            if &t.txid == txid {
                guard.pending = None;
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
        self.store.publish_gen(uid, user, new_gen, &pubkey)?;
        let file = to_config_file(&cfg, &sig, vec![]);
        serde_json::to_vec(&file).map_err(|e| EngineError::Verify(e.to_string()))
    }
}
