//! Per-uid root key store and the durable generation-commit protocol.
//!
//! Layout (spec §2, §7), all root-owned on the real system:
//! ```text
//! <root>/                 0755   (e.g. /var/lib/omarchy-relaunch)
//!   <uid>/                0755
//!     signing.key         0600   32 raw seed bytes — a SECRET
//!     public.json         0644   { v, uid, user, gen, enrollmentId, pubkey }
//! ```
//! The private seed never leaves root; the anchor verifies with the public key
//! alone. `gen` is monotonic and committed durably (fsync + rename) *before* a
//! signature is handed out, so a crash can only fail closed, never roll back.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use relaunch_core::{SEED_LEN, SIGNED_SCHEMA_VERSION};

const PUBLIC_V: u32 = SIGNED_SCHEMA_VERSION;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicState {
    pub v: u32,
    pub uid: u32,
    pub user: String,
    pub gen: u64,
    /// Stable per-key identity = hex(SHA-256(pubkey)[..16]). A new key (e.g.
    /// after Purge + re-enroll) yields a new id, so a recurring `gen` number
    /// under a different enrollment is distinguishable (spec §5.2).
    #[serde(rename = "enrollmentId")]
    pub enrollment_id: String,
    pub pubkey: String,
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(String),
    Corrupt(String),
    GenRegression { have: u64, asked: u64 },
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub struct Store {
    root: PathBuf,
}

pub fn enrollment_id_for(pubkey: &[u8; 32]) -> String {
    let digest = Sha256::digest(pubkey);
    hex::encode(&digest[..16])
}

impl Store {
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        Store { root: root.into() }
    }

    fn user_dir(&self, uid: u32) -> PathBuf {
        self.root.join(uid.to_string())
    }
    fn key_path(&self, uid: u32) -> PathBuf {
        self.user_dir(uid).join("signing.key")
    }
    fn public_path(&self, uid: u32) -> PathBuf {
        self.user_dir(uid).join("public.json")
    }

    /// Read the committed public state, or `None` if this uid is not enrolled.
    pub fn current(&self, uid: u32) -> Result<Option<PublicState>, StoreError> {
        let path = self.public_path(uid);
        match fs::read(&path) {
            Ok(bytes) => {
                let st: PublicState =
                    serde_json::from_slice(&bytes).map_err(|e| StoreError::Json(e.to_string()))?;
                if st.v != PUBLIC_V || st.uid != uid {
                    return Err(StoreError::Corrupt(format!("public.json for uid {uid}")));
                }
                Ok(Some(st))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Load the signing key for an enrolled uid.
    pub fn signing_key(&self, uid: u32) -> Result<SigningKey, StoreError> {
        let mut f = File::open(self.key_path(uid))?;
        let mut seed = [0u8; SEED_LEN];
        f.read_exact(&mut seed)
            .map_err(|_| StoreError::Corrupt(format!("signing.key for uid {uid}")))?;
        // Reject a trailing byte: the key file is exactly the seed.
        if f.read(&mut [0u8; 1])? != 0 {
            return Err(StoreError::Corrupt(format!(
                "signing.key for uid {uid} too long"
            )));
        }
        Ok(SigningKey::from_bytes(&seed))
    }

    /// Ensure the uid has a key and a committed public state, creating both on
    /// first use. Idempotent. Recovers an interrupted key-only enrollment by
    /// publishing `gen = 0` for the existing key; **never** regenerates a key
    /// that exists and never lowers a committed `gen`.
    pub fn ensure_enrolled(&self, uid: u32, user: &str) -> Result<PublicState, StoreError> {
        fs::create_dir_all(self.user_dir(uid))?;
        set_mode(&self.root, 0o755)?;
        set_mode(&self.user_dir(uid), 0o755)?;

        let key = match self.read_or_create_key(uid)? {
            Some(k) => k,
            None => self.signing_key(uid)?,
        };
        let pubkey = key.verifying_key().to_bytes();

        if let Some(st) = self.current(uid)? {
            // Committed state exists; trust its gen. (A pubkey mismatch means a
            // key was swapped under a stale public.json — corrupt, fail closed.)
            if st.pubkey != hex::encode(pubkey) {
                return Err(StoreError::Corrupt(format!(
                    "key/public.json mismatch for uid {uid}"
                )));
            }
            return Ok(st);
        }
        // Key exists (fresh or recovered) but no committed public state: publish
        // gen 0 durably. No signature has escaped, so this is not a rollback.
        self.write_public_durable(uid, user, 0, &pubkey)
    }

    /// Create `signing.key` with O_EXCL if absent and return the new key;
    /// `None` if it already existed (caller loads it). The O_EXCL create is the
    /// race guard: two concurrent first-authorizes cannot split keys.
    fn read_or_create_key(&self, uid: u32) -> Result<Option<SigningKey>, StoreError> {
        let path = self.key_path(uid);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut f) => {
                let mut seed = [0u8; SEED_LEN];
                getrandom::fill(&mut seed)
                    .map_err(|e| StoreError::Corrupt(format!("getrandom: {e}")))?;
                f.write_all(&seed)?;
                f.sync_all()?;
                sync_dir(&self.user_dir(uid))?;
                Ok(Some(SigningKey::from_bytes(&seed)))
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Durably publish a new committed generation. Enforces monotonicity, then
    /// writes `public.json.tmp`, fsync, rename, fsync dir. The caller signs the
    /// config carrying `new_gen` and only hands the signature out *after* this
    /// returns Ok.
    pub fn publish_gen(
        &self,
        uid: u32,
        user: &str,
        new_gen: u64,
        pubkey: &[u8; 32],
    ) -> Result<PublicState, StoreError> {
        if let Some(st) = self.current(uid)? {
            if new_gen <= st.gen {
                return Err(StoreError::GenRegression {
                    have: st.gen,
                    asked: new_gen,
                });
            }
        }
        self.write_public_durable(uid, user, new_gen, pubkey)
    }

    fn write_public_durable(
        &self,
        uid: u32,
        user: &str,
        gen: u64,
        pubkey: &[u8; 32],
    ) -> Result<PublicState, StoreError> {
        let st = PublicState {
            v: PUBLIC_V,
            uid,
            user: user.to_string(),
            gen,
            enrollment_id: enrollment_id_for(pubkey),
            pubkey: hex::encode(pubkey),
        };
        let bytes = serde_json::to_vec(&st).map_err(|e| StoreError::Json(e.to_string()))?;
        let tmp = self.public_path(uid).with_extension("json.tmp");
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, self.public_path(uid))?;
        sync_dir(&self.user_dir(uid))?;
        Ok(st)
    }

    /// Durably remove all state for a uid (Purge). The caller must already have
    /// invalidated any pending transaction and cached key.
    pub fn purge(&self, uid: u32) -> Result<(), StoreError> {
        let dir = self.user_dir(uid);
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        sync_dir(&self.root)?;
        Ok(())
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    let perm = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, perm)
}

/// fsync a directory so a create/rename/unlink inside it is durable.
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
