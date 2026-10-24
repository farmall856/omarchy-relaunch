//! Login-time verification: the root-owned trust chain for `public.json`, and
//! loading + signature-verifying the user's `config.json` against the committed
//! public key, generation and current account name. Any failure is fail-closed
//! (the caller emits a comment and launches nothing).

use std::path::Path;

use ed25519_dalek::VerifyingKey;
use relaunch_core::{parse_and_verify, SignedConfig};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicState {
    v: u32,
    uid: u32,
    user: String,
    gen: u64,
    #[serde(rename = "enrollmentId")]
    #[allow(
        dead_code,
        reason = "the service binds transactions to it; the anchor only needs the file to parse"
    )]
    enrollment_id: String,
    pubkey: String,
}

/// [`relaunch_sys::fs::trusted_chain`] with exactly one trusted owner,
/// `trusted_uid` (root in production): the shared walk, so the anchor and the
/// engine judge a root-installed path alike.
///
/// # Errors
///
/// A message naming the first component that is a symlink, is not owned by
/// `trusted_uid`, or is group/world-writable.
pub fn trusted_chain(path: &Path, trusted_uid: u32, boundary: &Path) -> Result<(), String> {
    relaunch_sys::fs::trusted_chain(path, |uid| uid == trusted_uid, boundary)
}

/// Verify the trust chain and the signature, binding uid, current account name
/// and generation. Returns the verified config on success.
///
/// # Errors
///
/// A reason the state is not trusted or the document does not verify;
/// the caller then launches nothing.
pub fn load_verified(
    state_root: &Path,
    uid: u32,
    user: &str,
    config_path: &Path,
    trusted_uid: u32,
) -> Result<SignedConfig, String> {
    let user_dir = state_root.join(uid.to_string());
    let public_path = user_dir.join("public.json");
    trusted_chain(&public_path, trusted_uid, state_root)?;

    let st: PublicState = serde_json::from_slice(
        &relaunch_sys::fs::read_regular_capped(&public_path, relaunch_sys::fs::MAX_FILE_BYTES)
            .map_err(|e| format!("read public.json: {e}"))?,
    )
    .map_err(|e| format!("parse public.json: {e}"))?;
    // Any other public.json version is a format this anchor does not
    // understand (fail closed).
    if st.v != relaunch_core::PUBLIC_STATE_VERSION {
        return Err(format!("public.json version {} unsupported", st.v));
    }
    if st.uid != uid {
        return Err("public.json uid mismatch".into());
    }
    if st.user != user {
        // The committed account name must match the current passwd entry.
        return Err("public.json user does not match current account".into());
    }

    let mut pk = [0u8; 32];
    hex::decode_to_slice(st.pubkey.as_bytes(), &mut pk)
        .map_err(|_| "bad pubkey hex".to_string())?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|_| "bad pubkey".to_string())?;

    if !config_path.exists() {
        return Err("no config.json".into());
    }
    // config.json uses the shared document limit the service enforces at sign
    // time (reviewer #1), so an authorized max-size change is always readable.
    let json =
        relaunch_sys::fs::read_regular_capped(config_path, relaunch_core::MAX_DOCUMENT_BYTES)
            .map_err(|e| format!("read config.json: {e}"))?;
    if signed_version(&json).is_some_and(|v| v != relaunch_core::SIGNED_SCHEMA_VERSION) {
        return Err(crate::boot::SCHEMA_UPGRADE_NEEDED.into());
    }
    parse_and_verify(&json, &vk, uid, user, st.gen).map_err(|e| format!("{e:?}"))
}

/// The signed schema version a `config.json` claims, read without trusting
/// anything else in it. Only the report's wording depends on it: a document of
/// another version launches nothing, as any unverified one does, and is named
/// as needing a fresh Save rather than as tampered.
fn signed_version(json: &[u8]) -> Option<u32> {
    #[derive(Deserialize)]
    struct Signed {
        v: u32,
    }
    #[derive(Deserialize)]
    struct File {
        signed: Signed,
    }
    serde_json::from_slice::<File>(json)
        .ok()
        .map(|f| f.signed.v)
}
