//! relaunch-core — the signed launch document, its canonical encoding, and
//! Ed25519 sign/verify.
//!
//! This crate is the shared security kernel for the Relaunch authorization
//! redesign (spec: `relaunch-authz-spec-final.md`). It holds no I/O, no D-Bus
//! and no polkit; the `relaunchd` service, the `relaunch-anchor` login verifier
//! and `relaunchctl` build on it. Keeping the format and the crypto in one
//! audited, side-effect-free place is what lets the service and the anchor
//! agree byte-for-byte on what was authorized.

mod canonical;
mod crypto;
mod model;

pub use canonical::{canonical_bytes, DOMAIN};
pub use crypto::{
    parse_and_verify, sign, signing_key_from_seed, to_config_file, verify, VerifyError,
    PUBLIC_KEY_LEN, SEED_LEN, SIGNATURE_LEN,
};
pub use model::{
    ConfigFile, Entry, ExecSource, Launcher, Provenance, SignedConfig, ValidationError,
    FILE_FORMAT_VERSION, MAX_ENTRIES, MAX_EXEC_LEN, ROOT_LAUNCHER_PREFIXES, SIGNED_SCHEMA_VERSION,
};
