//! relaunch-core — the signed launch document, its canonical encoding, and
//! Ed25519 sign/verify.
//!
//! This crate is the shared security kernel for the Relaunch authorization
//! redesign (spec: `relaunch-authz-spec-final.md`). It holds no I/O, no D-Bus
//! and no polkit; the `relaunchd` service, the `relaunch-anchor` login verifier
//! and `relaunchctl` build on it. Keeping the format and the crypto in one
//! audited, side-effect-free place is what lets the service and the anchor
//! agree byte-for-byte on what was authorized.
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

mod canonical;
mod crypto;
mod errors;
mod model;

pub use canonical::{canonical_bytes, DOMAIN};
pub use crypto::{
    parse_and_verify, sign, signing_key_from_seed, to_config_file, verify, VerifyError,
    PUBLIC_KEY_LEN, SEED_LEN, SIGNATURE_LEN,
};
pub use errors::{bounded_message, json_error, MAX_PUBLIC_ERROR};
pub use model::{
    is_launcher_id, ConfigFile, Entry, ExecSource, Launcher, Proposal, Provenance, SignedConfig,
    ValidationError, FILE_FORMAT_VERSION, LAUNCHERS_DIR_SUFFIX, LAUNCHER_ID_HEX,
    LAUNCHER_ID_PREFIX, MAX_CLASS_LEN, MAX_DELAY_MS, MAX_DOCUMENT_BYTES, MAX_ENTRIES, MAX_EXEC_LEN,
    MAX_REDUCE_OPS, MAX_REQUEST_BYTES, PUBLIC_STATE_VERSION, SIGNED_SCHEMA_VERSION,
};
