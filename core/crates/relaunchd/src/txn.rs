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
    /// Seconds since an arbitrary fixed origin; never goes backwards.
    fn now_secs(&self) -> u64;
}

/// Where an open transaction is in its lifecycle (spec §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Minted by Prepare; waiting for Authorize.
    Prepared,
    /// Authorize claimed it; the credential check is running.
    Authorizing,
}

/// One open transaction, bound to its sender, uid, account and enrollment.
#[derive(Clone, Debug)]
pub struct Transaction {
    /// The 128-bit capability; never leaves the service except to its sender.
    pub txid: [u8; 16],
    /// The uid it was prepared for.
    pub uid: u32,
    /// The account name at Prepare.
    pub user: String,
    /// The key enrollment it binds to.
    pub enrollment_id: String,
    /// The unique D-Bus sender that prepared it; Authorize must come from the
    /// same connection (spec §5.1).
    pub sender: String,
    /// Generation the base verified at; commit requires it still current.
    pub base_gen: u64,
    /// Proposal with its `gen` already set to `base_gen + 1` (what gets signed).
    pub proposal: SignedConfig,
    /// SHA-256 of the canonical bytes that will be signed.
    pub digest: [u8; 32],
    /// The credential prompt's text ([`crate::diff::Diff::prompt`]).
    pub prompt: String,
    /// Clock time at Prepare.
    pub created_secs: u64,
    /// Lease length.
    pub ttl_secs: u64,
    /// Lifecycle state.
    pub state: State,
    /// The cancellation id of the in-flight polkit check, set when the challenge
    /// is dispatched (phase 2). Used to cancel exactly that check on
    /// invalidation (reviewer M4/#2), never a newer one.
    pub check_id: Option<String>,
}

impl Transaction {
    /// Whether the lease has run out at `now`.
    #[must_use]
    pub fn expired_at(&self, now: u64) -> bool {
        now.saturating_sub(self.created_secs) >= self.ttl_secs
    }

    /// Four-character base32 code derived from the txid: NOT a secret and NOT
    /// the capability (the capability is the txid bound to the connection).
    /// Returned to the preparing connection; no surface shows it.
    #[must_use]
    pub fn code(&self) -> String {
        pairing_code(&self.txid)
    }
}

/// Digest bound into the transaction over the exact canonical bytes that will
/// be signed. The panel echoes a short form so the surfaces agree.
#[must_use]
pub fn digest_of(proposal: &SignedConfig) -> [u8; 32] {
    let bytes = relaunch_core::canonical_bytes(proposal);
    Sha256::digest(bytes).into()
}

/// A fresh 128-bit random id.
///
/// # Errors
///
/// The OS entropy source failed.
pub fn new_txid() -> Result<[u8; 16], String> {
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|e| format!("getrandom: {e}"))?;
    Ok(id)
}

/// The 4-symbol base32 pairing code for a txid: the top 20 bits of its first
/// three bytes.
#[must_use]
pub fn pairing_code(txid: &[u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let [a, b, c, ..] = *txid;
    let acc = u32::from_be_bytes([0, a, b, c]);
    // 3 bytes = 24 bits -> take the top 20 bits as 4 base32 symbols.
    let mut out = String::with_capacity(4);
    for i in 0..4 {
        let shift = 24 - 5 * (i + 1);
        let idx = ((acc >> shift) & 0x1f) as usize;
        #[allow(
            clippy::indexing_slicing,
            reason = "idx is masked to 5 bits and ALPHABET has exactly 32 entries"
        )]
        let symbol = ALPHABET[idx];
        out.push(char::from(symbol));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::pairing_code;

    #[test]
    fn pairing_code_is_pinned() {
        // Golden from the original shift-loop derivation: 0x123456 -> 2, 8, 26, 5.
        let mut txid = [0u8; 16];
        txid[..3].copy_from_slice(&[0x12, 0x34, 0x56]);
        assert_eq!(pairing_code(&txid), "CI2F");
        assert_eq!(pairing_code(&[0u8; 16]), "AAAA");
        assert_eq!(pairing_code(&[0xff; 16]), "7777");
    }
}
