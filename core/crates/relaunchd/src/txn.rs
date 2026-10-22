//! The pending authorization transaction and its lifetime.
//!
//! A `Prepare` freezes exactly what will be signed (spec §5.2/§5.4): the
//! canonical proposal, its digest, the bound base generation, the bound bus
//! sender + uid + account name + enrollment id, and a bounded lease. `Authorize`
//! may only commit a transaction that is still live and bound to the same
//! connection; a late success after cancel/expiry/Purge must not sign.

use relaunch_core::SignedConfig;
use sha2::{Digest, Sha256};

/// Lease lifetime. A prepared transaction is dropped after this, so a
/// background `Prepare` cannot hold the one-per-uid slot indefinitely.
pub const DEFAULT_TTL_SECS: u64 = 180;

/// Monotonic time source, injectable for tests.
pub trait Clock {
    fn now_secs(&self) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Prepared,
    Authorizing,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub txid: [u8; 16],
    pub uid: u32,
    pub user: String,
    pub enrollment_id: String,
    /// The unique D-Bus sender that prepared it; Authorize must come from the
    /// same connection (spec §5.1).
    pub sender: String,
    /// Generation the base verified at; commit requires it still current.
    pub base_gen: u64,
    /// Proposal with its `gen` already set to `base_gen + 1` (what gets signed).
    pub proposal: SignedConfig,
    pub digest: [u8; 32],
    pub changed: usize,
    pub created_secs: u64,
    pub ttl_secs: u64,
    pub state: State,
    /// The cancellation id of the in-flight polkit check, set when the challenge
    /// is dispatched (phase 2). Used to cancel exactly that check on
    /// invalidation (reviewer M4/#2), never a newer one.
    pub check_id: Option<String>,
}

impl Transaction {
    pub fn expired_at(&self, now: u64) -> bool {
        now.saturating_sub(self.created_secs) >= self.ttl_secs
    }

    /// Four-character base32 code: a ~20-bit human comparison hint, NOT a
    /// secret and NOT the capability (the capability is the txid bound to the
    /// connection). Derived from the txid so the panel and the prompt agree.
    pub fn code(&self) -> String {
        const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut acc: u32 = 0;
        for b in &self.txid[..3] {
            acc = (acc << 8) | *b as u32;
        }
        // 3 bytes = 24 bits -> take the top 20 bits as 4 base32 symbols.
        let mut out = String::with_capacity(4);
        for i in 0..4 {
            let shift = 24 - 5 * (i + 1);
            out.push(ALPHABET[((acc >> shift) & 0x1f) as usize] as char);
        }
        out
    }
}

/// Digest bound into the transaction over the exact canonical bytes that will
/// be signed. The panel echoes a short form so the surfaces agree.
pub fn digest_of(proposal: &SignedConfig) -> [u8; 32] {
    let bytes = relaunch_core::canonical_bytes(proposal);
    Sha256::digest(bytes).into()
}

pub fn new_txid() -> Result<[u8; 16], String> {
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|e| format!("getrandom: {e}"))?;
    Ok(id)
}
