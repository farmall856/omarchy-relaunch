//! Ed25519 signing and verification over the canonical bytes, plus the
//! parse-and-verify path the anchor and service use.
//!
//! The private key lives only in the root service; verification needs just the
//! public key, so the user-mode anchor never holds a secret. A parsed document
//! is re-serialized canonically and verified, and the *same* parsed value is
//! what the caller then emits/launches — no verify-one-read/execute-another.

use crate::canonical::canonical_bytes;
use crate::model::{ConfigFile, SignedConfig, ValidationError, FILE_FORMAT_VERSION};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

pub const SEED_LEN: usize = 32;
pub const PUBLIC_KEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

#[derive(Debug)]
pub enum VerifyError {
    Json(String),
    Validation(ValidationError),
    FileVersion(u32),
    SignatureEncoding,
    BadSignature,
    UidMismatch { expected: u32, found: u32 },
    UserMismatch { expected: String, found: String },
    GenMismatch { expected: u64, found: u64 },
}

impl From<ValidationError> for VerifyError {
    fn from(e: ValidationError) -> Self {
        VerifyError::Validation(e)
    }
}

/// Build a signing key from a 32-byte seed held by the root service.
pub fn signing_key_from_seed(seed: &[u8; SEED_LEN]) -> SigningKey {
    SigningKey::from_bytes(seed)
}

/// Sign a *validated* config. Returns the raw 64-byte signature. Callers must
/// have called `validate` first (debug-asserted) so the bytes are canonical.
pub fn sign(key: &SigningKey, cfg: &SignedConfig) -> [u8; SIGNATURE_LEN] {
    debug_assert!(cfg.validate().is_ok(), "sign() requires a validated config");
    key.sign(&canonical_bytes(cfg)).to_bytes()
}

/// Verify a signature over `cfg`'s canonical bytes. Does not itself re-validate
/// structure; `parse_and_verify` does both.
pub fn verify(key: &VerifyingKey, cfg: &SignedConfig, sig: &[u8; SIGNATURE_LEN]) -> bool {
    let sig = Signature::from_bytes(sig);
    key.verify(&canonical_bytes(cfg), &sig).is_ok()
}

/// Parse `config.json` bytes, validate structure, verify the signature against
/// `pubkey`, and bind the identity: `uid`, `user` and `gen` must equal the
/// values the trusted root state selected for this caller. Any failure is
/// fail-closed; the caller runs nothing.
pub fn parse_and_verify(
    json: &[u8],
    pubkey: &VerifyingKey,
    expected_uid: u32,
    expected_user: &str,
    expected_gen: u64,
) -> Result<SignedConfig, VerifyError> {
    let file: ConfigFile =
        serde_json::from_slice(json).map_err(|e| VerifyError::Json(e.to_string()))?;
    if file.v != FILE_FORMAT_VERSION {
        return Err(VerifyError::FileVersion(file.v));
    }
    file.signed.validate()?;

    // Identity binding before trusting anything in the document.
    if file.signed.uid != expected_uid {
        return Err(VerifyError::UidMismatch {
            expected: expected_uid,
            found: file.signed.uid,
        });
    }
    if file.signed.user != expected_user {
        return Err(VerifyError::UserMismatch {
            expected: expected_user.to_string(),
            found: file.signed.user.clone(),
        });
    }
    if file.signed.gen != expected_gen {
        return Err(VerifyError::GenMismatch {
            expected: expected_gen,
            found: file.signed.gen,
        });
    }

    let sig_bytes = hex::decode(&file.signature).map_err(|_| VerifyError::SignatureEncoding)?;
    let sig_arr: [u8; SIGNATURE_LEN] = sig_bytes
        .try_into()
        .map_err(|_| VerifyError::SignatureEncoding)?;
    if !verify(pubkey, &file.signed, &sig_arr) {
        return Err(VerifyError::BadSignature);
    }
    Ok(file.signed)
}

/// Serialize a signed config plus signature into the on-disk `config.json`
/// envelope. `pending` is carried through opaquely.
pub fn to_config_file(
    cfg: &SignedConfig,
    sig: &[u8; SIGNATURE_LEN],
    pending: Vec<serde_json::Value>,
) -> ConfigFile {
    ConfigFile {
        v: FILE_FORMAT_VERSION,
        signed: cfg.clone(),
        signature: hex::encode(sig),
        pending,
    }
}
