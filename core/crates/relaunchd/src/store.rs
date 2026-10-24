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

use relaunch_core::{PUBLIC_STATE_VERSION, SEED_LEN};

const PUBLIC_V: u32 = PUBLIC_STATE_VERSION;

/// The committed public state for one uid (`public.json`, 0644).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicState {
    /// Schema version.
    pub v: u32,
    /// The uid this state belongs to.
    pub uid: u32,
    /// The account name at enrollment.
    pub user: String,
    /// The committed generation; a signed document must carry exactly this.
    pub gen: u64,
    /// Stable per-key identity = hex(SHA-256(pubkey)[..16]). A new key (e.g.
    /// after Purge + re-enroll) yields a new id, so a recurring `gen` number
    /// under a different enrollment is distinguishable (spec §5.2).
    #[serde(rename = "enrollmentId")]
    pub enrollment_id: String,
    /// Hex Ed25519 public key.
    pub pubkey: String,
}

/// A key-store failure. All are fail-closed.
#[derive(Debug)]
pub enum StoreError {
    /// A filesystem error.
    Io(io::Error),
    /// `public.json` is not valid.
    Json(String),
    /// The on-disk state is inconsistent (e.g. a key with no public state).
    Corrupt(String),
    /// A publish would not advance the generation.
    GenRegression {
        /// The committed generation.
        have: u64,
        /// The generation asked for.
        asked: u64,
    },
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// The per-uid key store under the service's state directory.
pub struct Store {
    root: PathBuf,
}

/// The enrollment id of a public key: hex of the first 16 bytes of its SHA-256.
#[must_use]
pub fn enrollment_id_for(pubkey: &[u8; 32]) -> String {
    let digest = Sha256::digest(pubkey);
    hex::encode(digest.iter().take(16).copied().collect::<Vec<u8>>())
}

impl Store {
    /// A store rooted at `root` (production: `/var/lib/omarchy-relaunch`).
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
    ///
    /// # Errors
    ///
    /// An unreadable or invalid `public.json`.
    pub fn current(&self, uid: u32) -> Result<Option<PublicState>, StoreError> {
        let path = self.public_path(uid);
        match fs::read(&path) {
            Ok(bytes) => {
                let st: PublicState = serde_json::from_slice(&bytes)
                    .map_err(|e| StoreError::Json(relaunch_core::json_error(&e)))?;
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
    ///
    /// # Errors
    ///
    /// A missing, unreadable or malformed key file.
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
    /// first use. Idempotent.
    ///
    /// The generation-reuse rule (spec §7, reviewer I1/M1): only the call that
    /// *just created* the key (`O_EXCL` success) may publish `gen = 0`; it does so
    /// immediately, in this same critical section, so a committed `gen = 0`
    /// always exists after a successful first enrollment. A key that already
    /// exists with **no** committed public state is therefore either a crash in
    /// that microscopic window or a lost `public.json` over a key that has
    /// already signed — the two are indistinguishable, so we **fail closed**
    /// rather than republish `gen = 0` and let a historical signature verify
    /// again. Recovery is an explicit purge + re-adopt (a fresh key).
    ///
    /// # Errors
    ///
    /// A filesystem failure, or an existing key without committed public
    /// state (`Corrupt`, fail-closed).
    pub fn ensure_enrolled(&self, uid: u32, user: &str) -> Result<PublicState, StoreError> {
        fs::create_dir_all(self.user_dir(uid))?;
        set_mode(&self.root, 0o755)?;
        set_mode(&self.user_dir(uid), 0o755)?;
        // Durability of the just-created directory entry.
        sync_dir(&self.root)?;

        if let Some(key) = self.read_or_create_key(uid)? {
            // We created the key this call. Commit gen 0 now, so a key is
            // never left without committed public state except across a
            // crash in this window.
            let pubkey = key.verifying_key().to_bytes();
            self.write_public_durable(uid, user, 0, &pubkey)
        } else {
            // Key already on disk. It MUST have a committed public state.
            let st = self.current(uid)?.ok_or_else(|| {
                StoreError::Corrupt(format!(
                    "signing.key for uid {uid} present without committed public state; \
                     purge to re-enroll"
                ))
            })?;
            let pubkey = self.signing_key(uid)?.verifying_key().to_bytes();
            if st.pubkey != hex::encode(pubkey) {
                return Err(StoreError::Corrupt(format!(
                    "key/public.json mismatch for uid {uid}"
                )));
            }
            Ok(st)
        }
    }

    /// Create `signing.key` with `O_EXCL` if absent and return the new key;
    /// `None` if it already existed (caller loads it). The `O_EXCL` create is the
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
    ///
    /// # Errors
    ///
    /// A filesystem failure, or a generation that would not advance.
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
        // Set, not requested: `mode(0o644)` is narrowed by the process umask,
        // and the anchor (running as the user) must read the trust anchor.
        set_mode(&tmp, 0o644)?;
        fs::rename(&tmp, self.public_path(uid))?;
        sync_dir(&self.user_dir(uid))?;
        Ok(st)
    }

    /// Durably remove all state for a uid (Purge). The caller must already have
    /// invalidated any pending transaction.
    ///
    /// Atomic revocation (spec §7, reviewer I2): `rename` the uid directory to a
    /// tombstone and fsync the state root *first*, so the trust anchor
    /// (`<uid>/public.json`) is gone the instant we report progress; then delete
    /// every tombstone of this uid and fsync the root again. Each step runs on
    /// every call, whether or not the live directory still exists, so a retry
    /// after any failure (a crash, a failed fsync, a failed delete) finishes the
    /// same revocation and success is reported only once the uid has no state,
    /// tombstones and their keys included, durably.
    ///
    /// # Errors
    ///
    /// A filesystem failure in any step; the call can be retried.
    pub fn purge(&self, uid: u32) -> Result<(), StoreError> {
        let dir = self.user_dir(uid);
        if fs::symlink_metadata(&dir).is_ok() {
            let mut rnd = [0u8; 8];
            getrandom::fill(&mut rnd)
                .map_err(|e| StoreError::Corrupt(format!("getrandom: {e}")))?;
            let tomb = self
                .root
                .join(format!("{uid}{TOMBSTONE_INFIX}{}", hex::encode(rnd)));
            fs::rename(&dir, &tomb)?;
        }
        if !self.root.exists() {
            return Ok(());
        }
        sync_dir(&self.root)?;
        // The anchor is now unreachable; delete what it left, durably.
        for tomb in self.tombstones(uid)? {
            #[cfg(test)]
            tests::fault(tests::Fault::Remove)?;
            fs::remove_dir_all(&tomb)?;
        }
        sync_dir(&self.root)?;
        Ok(())
    }

    /// This uid's tombstones: `<uid>.purging-*` in the state root.
    fn tombstones(&self, uid: u32) -> io::Result<Vec<PathBuf>> {
        let prefix = format!("{uid}{TOMBSTONE_INFIX}");
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                out.push(entry.path());
            }
        }
        Ok(out)
    }
}

/// Between a uid and its tombstone's random suffix.
const TOMBSTONE_INFIX: &str = ".purging-";

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    let perm = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, perm)
}

/// fsync a directory so a create/rename/unlink inside it is durable.
fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    tests::fault(tests::Fault::Sync)?;
    File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A filesystem step a test can make fail once.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Fault {
        Sync,
        Remove,
    }

    thread_local! {
        static FAULTS: RefCell<Vec<Fault>> = const { RefCell::new(Vec::new()) };
        static SEEN: RefCell<Vec<Fault>> = const { RefCell::new(Vec::new()) };
    }

    /// Record `step`, and fail it if a test armed that fault.
    pub(super) fn fault(step: Fault) -> io::Result<()> {
        SEEN.with(|s| s.borrow_mut().push(step));
        let armed = FAULTS.with(|f| {
            let mut f = f.borrow_mut();
            f.iter().position(|x| *x == step).map(|i| f.remove(i))
        });
        match armed {
            Some(_) => Err(io::Error::other(format!("injected {step:?} failure"))),
            None => Ok(()),
        }
    }

    fn arm(step: Fault) {
        FAULTS.with(|f| f.borrow_mut().push(step));
    }

    fn seen() -> Vec<Fault> {
        SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("relaunchd-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Nothing under `root` but `root` itself: no live dir, no tombstone, no key.
    fn assert_empty(root: &Path) {
        let left: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn a_retry_after_a_failed_root_sync_finishes_the_purge() {
        let dir = root("sync");
        let store = Store::new(dir.clone());
        store.ensure_enrolled(1000, "u").unwrap();
        arm(Fault::Sync);
        assert!(
            store.purge(1000).is_err(),
            "a failed root sync must not report success"
        );
        // The retry must sync the root again: an injected failure there fails it too.
        arm(Fault::Sync);
        assert!(
            store.purge(1000).is_err(),
            "the retry must re-sync the root"
        );
        let _ = seen();
        store.purge(1000).unwrap();
        assert!(
            seen().contains(&Fault::Sync),
            "a successful retry syncs the root"
        );
        assert_empty(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_retry_after_a_failed_cleanup_removes_the_tombstone_and_its_key() {
        let dir = root("remove");
        let store = Store::new(dir.clone());
        store.ensure_enrolled(1000, "u").unwrap();
        arm(Fault::Remove);
        assert!(store.purge(1000).is_err());
        assert!(
            store.current(1000).unwrap().is_none(),
            "the anchor is gone at once"
        );
        store.purge(1000).unwrap();
        assert_empty(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_public_json_written_before_the_signed_schema_moved_still_reads() {
        // Installs that enrolled under signed schema v1 hold `"v":1` here; a
        // new signed schema must still find the key it is re-authorized under.
        let dir = root("public-v1");
        let store = Store::new(dir.clone());
        let st = store.ensure_enrolled(1000, "u").unwrap();
        let on_disk: serde_json::Value =
            serde_json::from_slice(&fs::read(store.public_path(1000)).unwrap()).unwrap();
        assert_eq!(on_disk["v"], 1, "public.json's own format is still v1");
        let hand_written = format!(
            r#"{{"v":1,"uid":1000,"user":"u","gen":{},"enrollmentId":"{}","pubkey":"{}"}}"#,
            st.gen, st.enrollment_id, st.pubkey
        );
        fs::write(store.public_path(1000), hand_written).unwrap();
        assert_eq!(store.current(1000).unwrap(), Some(st));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_finishes_only_this_uids_tombstones() {
        let dir = root("others");
        let store = Store::new(dir.clone());
        fs::create_dir_all(dir.join("1000.purging-0011223344556677")).unwrap();
        fs::write(
            dir.join("1000.purging-0011223344556677/signing.key"),
            [7u8; 32],
        )
        .unwrap();
        fs::create_dir_all(dir.join("10001.purging-aa")).unwrap();
        store.ensure_enrolled(2000, "v").unwrap();
        store.purge(1000).unwrap();
        assert!(!dir.join("1000.purging-0011223344556677").exists());
        assert!(
            dir.join("10001.purging-aa").exists(),
            "another uid's tombstone is not ours"
        );
        assert!(store.current(2000).unwrap().is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
