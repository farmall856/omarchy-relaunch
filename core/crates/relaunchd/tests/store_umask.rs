//! The key store's modes do not depend on the service's umask: public.json
//! (the trust anchor the user-run anchor reads) is 0644 and signing.key 0600
//! even under umask 077. Its own test binary, because the umask is per process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed assertion"
)]

use std::os::unix::fs::PermissionsExt;

use nix::sys::stat::{umask, Mode};
use relaunchd::Store;

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn public_json_is_0644_and_signing_key_0600_under_umask_077() {
    let root = std::env::temp_dir().join(format!("relaunchd-umask-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let old = umask(Mode::from_bits_truncate(0o077));
    let store = Store::new(root.clone());
    store.ensure_enrolled(1000, "u").unwrap();
    let key = store.signing_key(1000).unwrap();
    let pk = key.verifying_key().to_bytes();
    store.publish_gen(1000, "u", 1, &pk).unwrap();
    umask(old);

    let dir = root.join("1000");
    assert_eq!(mode(&dir.join("public.json")), 0o644, "public.json");
    assert_eq!(mode(&dir.join("signing.key")), 0o600, "signing.key");
    assert_eq!(mode(&dir), 0o755, "the uid directory");
    let _ = std::fs::remove_dir_all(&root);
}
