//! Login-time verification: the root-owned trust chain for `public.json`, and
//! loading + signature-verifying the user's `config.json` against the committed
//! public key, generation and current account name. Any failure is fail-closed
//! (the caller emits a comment and launches nothing).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use ed25519_dalek::VerifyingKey;
use relaunch_core::{parse_and_verify, SignedConfig};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicState {
    #[allow(dead_code)]
    v: u32,
    uid: u32,
    user: String,
    gen: u64,
    #[serde(rename = "enrollmentId")]
    #[allow(dead_code)]
    enrollment_id: String,
    pubkey: String,
}

/// Every component from `path` up to and including `boundary` must be owned by
/// `trusted_uid` (root in production), be a real file/dir (not a symlink), and
/// carry no group/other write bit. The walk stops at `boundary` (the state
/// root), since the filesystem above it is system-trusted; `path` must lie
/// within it.
pub fn trusted_chain(path: &Path, trusted_uid: u32, boundary: &Path) -> Result<(), String> {
    let mut cur = path;
    loop {
        let md = fs::symlink_metadata(cur).map_err(|e| format!("lstat {}: {e}", cur.display()))?;
        if md.file_type().is_symlink() {
            return Err(format!("symlink in trust path: {}", cur.display()));
        }
        if md.uid() != trusted_uid {
            return Err(format!("not owned by uid {trusted_uid}: {}", cur.display()));
        }
        if md.mode() & 0o022 != 0 {
            return Err(format!("group/other-writable: {}", cur.display()));
        }
        if cur == boundary {
            return Ok(());
        }
        cur = cur
            .parent()
            .ok_or_else(|| "path is not under the state root".to_string())?;
    }
}

/// Verify the trust chain and the signature, binding uid, current account name
/// and generation. Returns the verified config on success.
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
        &crate::fsio::read_regular_capped(&public_path, crate::fsio::MAX_FILE_BYTES)
            .map_err(|e| format!("read public.json: {e}"))?,
    )
    .map_err(|e| format!("parse public.json: {e}"))?;
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
    let json = crate::fsio::read_regular_capped(config_path, relaunch_core::MAX_DOCUMENT_BYTES)
        .map_err(|e| format!("read config.json: {e}"))?;
    parse_and_verify(&json, &vk, uid, user, st.gen).map_err(|e| format!("{e:?}"))
}
