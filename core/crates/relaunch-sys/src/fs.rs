//! Bounded, typed reads and the one private atomic writer (INV-3, INV-7).
//!
//! Every Relaunch-owned file is written through [`write_private`]: through any
//! symlink to its resolved target (a dotfiles-managed link survives), via an
//! `O_EXCL|O_NOFOLLOW` temp beside the target, fsynced, renamed into place,
//! with the directory fsynced, and always at mode 0600.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Default read cap: far above any real config, launcher or state file.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

/// `path` (followed through symlinks) is a regular file this process may
/// execute: what `execve` would run, never a directory.
#[must_use]
pub fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
        && nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
}

/// Read at most `max` bytes of a regular file, opened once with `O_NONBLOCK` and
/// `O_CLOEXEC` so a FIFO or device cannot block the reader.
///
/// No `O_NOFOLLOW`: config.json may legitimately be a dotfiles symlink, and a
/// launcher path is canonicalized by its caller. The regular-file check is on
/// the opened descriptor, not on the path.
///
/// # Errors
///
/// A message if the open fails, the opened object is not a regular file, or it
/// holds more than `max` bytes.
pub fn read_regular_capped(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let md = f.metadata().map_err(|e| format!("fstat: {e}"))?;
    if !md.file_type().is_file() {
        return Err("not a regular file".into());
    }
    let len = usize::try_from(md.len()).unwrap_or(usize::MAX);
    if len > max {
        return Err("file too large".into());
    }
    let mut buf = Vec::with_capacity(len);
    // Cap the read independently of the stat'd size (it can race).
    f.take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if buf.len() > max {
        return Err("file too large".into());
    }
    Ok(buf)
}

/// Every component from `path` up to and including `boundary` is a real file
/// or directory (not a symlink), owned by a uid `is_trusted` accepts, and
/// carries no group/other write bit. The filesystem above `boundary` is taken
/// as trusted; `path` must lie within it.
///
/// # Errors
///
/// A message naming the first component that is a symlink, is owned by an
/// untrusted uid, or is group/world-writable.
pub fn trusted_chain(
    path: &Path,
    is_trusted: impl Fn(u32) -> bool,
    boundary: &Path,
) -> Result<(), String> {
    let mut cur = path;
    loop {
        let md =
            std::fs::symlink_metadata(cur).map_err(|e| format!("lstat {}: {e}", cur.display()))?;
        if md.file_type().is_symlink() {
            return Err(format!("symlink in trust path: {}", cur.display()));
        }
        if !is_trusted(md.uid()) {
            return Err(format!(
                "not owned by a trusted uid ({}): {}",
                md.uid(),
                cur.display()
            ));
        }
        if md.mode() & 0o022 != 0 {
            return Err(format!("group/other-writable: {}", cur.display()));
        }
        if cur == boundary {
            return Ok(());
        }
        cur = cur
            .parent()
            .ok_or_else(|| format!("{} is not under {}", path.display(), boundary.display()))?;
    }
}

/// The path a write to `path` must land on. A rename replaces whatever is at
/// its destination, so renaming onto a symlink would destroy the link; follow
/// the chain and write the target instead. Each link target is joined to the
/// directory holding the link, unsimplified, and the final directory is
/// resolved by the filesystem ([`resolve_dir`]), so a write lands exactly where
/// a read of `path` does. A dangling link (its target, or the target's
/// directories, not created yet) resolves to where creating it would put it.
///
/// # Errors
///
/// An I/O error if the chain cannot be read, is too long, or does not end in a
/// file name.
pub fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
    let mut cur = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => {
                let link = std::fs::read_link(&cur)?;
                cur = cur
                    .parent()
                    .map_or_else(|| link.clone(), |dir| dir.join(&link));
            }
            _ => return in_resolved_dir(&cur),
        }
    }
    Err(io::Error::other(format!(
        "too many symlinks resolving {}",
        path.display()
    )))
}

/// `leaf`'s file name inside its directory as the filesystem resolves it; the
/// leaf itself is not followed.
fn in_resolved_dir(leaf: &Path) -> io::Result<PathBuf> {
    let name = leaf
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} does not name a file", leaf.display())))?;
    let dir = leaf.parent().filter(|d| !d.as_os_str().is_empty());
    Ok(resolve_dir(dir.unwrap_or(Path::new(".")))?.join(name))
}

/// `dir` as the filesystem resolves it (`canonicalize`), or, when its tail
/// does not exist yet, its longest existing ancestor resolved with the missing
/// names appended. A missing tail may hold only plain names: `..`, `.` or a
/// dangling link there has no filesystem meaning yet, so it is refused rather
/// than simplified.
///
/// # Errors
///
/// An I/O error resolving the existing part, or a refused missing tail.
pub fn resolve_dir(dir: &Path) -> io::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut base = dir;
    loop {
        match std::fs::canonicalize(base) {
            Ok(mut real) => {
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let refused = || io::Error::other(format!("cannot resolve {}: {e}", dir.display()));
                if std::fs::symlink_metadata(base).is_ok() {
                    return Err(refused());
                }
                missing.push(base.file_name().ok_or_else(refused)?.to_owned());
                base = base
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Create the resolved target's missing parent directories: 0700 when it is
/// `private_dir` (Relaunch's own config directory), the umask default otherwise
/// (a dotfiles directory is the user's).
fn ensure_parent(target: &Path, private_dir: &Path) -> io::Result<PathBuf> {
    let dir = target.parent().unwrap_or(Path::new(".")).to_path_buf();
    if !dir.exists() {
        let mut b = DirBuilder::new();
        b.recursive(true);
        if dir == private_dir || resolve_dir(private_dir).is_ok_and(|p| p == dir) {
            b.mode(0o700);
        }
        b.create(&dir)?;
    }
    Ok(dir)
}

/// The one temp-write-rename: `bytes` into an `O_EXCL|O_NOFOLLOW` temp beside
/// `target`, at exactly `mode`, fsynced, renamed over `target`, the directory
/// fsynced. The destination is left untouched on error.
fn replace_atomically(target: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let tmp = dir.join(format!(".{name}.tmp.{}", temp_suffix()));
    let written = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        // The mode is set, not inherited: the umask can neither widen nor
        // narrow it.
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Atomically replace the file at `path` (through any symlink) with `bytes`, at
/// mode 0600 (enforced: a pre-existing 0644 file is not preserved). Missing
/// parent directories are created as [`ensure_parent`] says.
///
/// # Errors
///
/// Any I/O error resolving, creating, writing, syncing or renaming. The
/// destination is left untouched on error.
pub fn write_private(path: &Path, bytes: &[u8], private_dir: &Path) -> io::Result<()> {
    let target = resolve_write_target(path)?;
    ensure_parent(&target, private_dir)?;
    replace_atomically(&target, bytes, 0o600)
}

/// Atomically replace Relaunch's own file `name` in `private_dir` (a boot
/// report, the session snapshot, the inspection copy of the rules) with
/// `bytes` at mode 0600. Unlike [`write_private`] the leaf is never followed:
/// whatever is at that name, a planted symlink included, is replaced by the
/// rename, so the write cannot leave `private_dir`. `name` must be a plain
/// file name.
///
/// # Errors
///
/// An I/O error if `name` is not a plain file name or the write fails; the
/// destination is left untouched on error.
pub fn write_private_artifact(private_dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(io::Error::other(format!("not a plain file name: {name:?}")));
    }
    let target = private_dir.join(name);
    ensure_parent(&target, private_dir)?;
    replace_atomically(&target, bytes, 0o600)
}

/// Create the file at `path` (through any symlink, a dangling one included)
/// holding `bytes` at mode 0600, unless it already exists. Born private, never
/// tightened afterwards.
///
/// # Errors
///
/// Any I/O error resolving or creating it.
pub fn create_private_if_absent(path: &Path, bytes: &[u8], private_dir: &Path) -> io::Result<()> {
    let target = resolve_write_target(path)?;
    ensure_parent(&target, private_dir)?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .mode(0o600)
        .open(&target)
    {
        Ok(mut f) => {
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            f.write_all(bytes)?;
            f.sync_all()
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Rewrite one of the USER's files (`hyprland.lua`, `hyprland.conf`) through
/// any symlink: `edit` maps its bytes to the new bytes, and the result replaces
/// it atomically with the file's own mode kept. Nothing is written when the
/// bytes do not change, or when there is no regular file to edit.
///
/// Returns whether the file was rewritten.
///
/// # Errors
///
/// Any I/O error resolving, reading or replacing it.
pub fn rewrite_user_file(path: &Path, edit: impl FnOnce(&[u8]) -> Vec<u8>) -> io::Result<bool> {
    let target = resolve_write_target(path)?;
    let Ok(md) = std::fs::metadata(&target) else {
        return Ok(false);
    };
    if !md.is_file() {
        return Ok(false);
    }
    let old = read_regular_capped(&target, MAX_FILE_BYTES).map_err(io::Error::other)?;
    let new = edit(&old);
    if new == old {
        return Ok(false);
    }
    replace_atomically(&target, &new, md.permissions().mode() & 0o7777)?;
    Ok(true)
}

/// Create `path` (an empty flag file) at mode 0600 if it does not exist,
/// creating Relaunch's private directory first. An existing file is left as
/// it is: a flag's presence is all that matters.
///
/// # Errors
///
/// Any I/O error creating the directory or the file.
pub fn touch_private(path: &Path, private_dir: &Path) -> io::Result<()> {
    if !private_dir.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(private_dir)?;
    }
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .mode(0o600)
        .open(path)
    {
        Ok(f) => f.set_permissions(std::fs::Permissions::from_mode(0o600)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn temp_suffix() -> String {
    let mut rnd = [0u8; 8];
    // A predictable name costs at most an EEXIST (the temp is O_EXCL and
    // O_NOFOLLOW), never a redirected write; fall back to pid and time.
    if getrandom::fill(&mut rnd).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() ^ u64::from(d.subsec_nanos()));
        rnd = (u64::from(std::process::id()) ^ t).to_le_bytes();
    }
    rnd.iter().fold(String::with_capacity(16), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "relaunch-sys-{tag}-{}-{}",
            std::process::id(),
            temp_suffix()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn write_private_creates_a_0600_file_in_a_0700_private_dir() {
        let root = tmpdir("new");
        let cfg = root.join("cfg");
        write_private(&cfg.join("config.json"), b"{}", &cfg).unwrap();
        assert_eq!(std::fs::read(cfg.join("config.json")).unwrap(), b"{}");
        assert_eq!(mode(&cfg.join("config.json")), 0o600);
        assert_eq!(mode(&cfg), 0o700);
    }

    #[test]
    fn write_private_enforces_0600_over_an_existing_wider_file() {
        let root = tmpdir("wide");
        let f = root.join("config.json");
        std::fs::write(&f, b"old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&f, b"new", &root).unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(std::fs::read(&f).unwrap(), b"new");
    }

    #[test]
    fn write_private_writes_through_a_symlink_and_keeps_it() {
        // github issue #2: a dotfiles-managed link must survive every write.
        let root = tmpdir("link");
        std::fs::create_dir_all(root.join("dotfiles")).unwrap();
        std::fs::write(root.join("dotfiles/config.json"), b"old").unwrap();
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        symlink(
            root.join("dotfiles/config.json"),
            root.join("cfg/config.json"),
        )
        .unwrap();
        write_private(&root.join("cfg/config.json"), b"new", &root.join("cfg")).unwrap();
        assert!(std::fs::symlink_metadata(root.join("cfg/config.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(root.join("dotfiles/config.json")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn write_private_creates_a_missing_target_parent_through_a_link() {
        // PR #18: the "dotfiles repo not cloned yet" shape.
        let root = tmpdir("missing");
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        symlink(
            root.join("dotfiles/rl/config.json"),
            root.join("cfg/config.json"),
        )
        .unwrap();
        write_private(&root.join("cfg/config.json"), b"x", &root.join("cfg")).unwrap();
        assert!(std::fs::symlink_metadata(root.join("cfg/config.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(root.join("dotfiles/rl/config.json")).unwrap(),
            b"x"
        );
    }

    #[test]
    fn relative_symlink_chains_resolve() {
        let root = tmpdir("rel");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/real"), b"").unwrap();
        symlink("../real", root.join("a/b/l1")).unwrap();
        symlink("b/l1", root.join("a/l2")).unwrap();
        assert_eq!(
            resolve_write_target(&root.join("a/l2")).unwrap(),
            root.join("a/real")
        );
    }

    #[test]
    fn a_write_lands_where_a_read_of_the_same_path_does() {
        // cfg/config.json -> x/dl/../victim, dl a directory link to deep/inner:
        // the kernel reads deep/victim; a lexical `..` would pick x/victim.
        let root = tmpdir("dotdot");
        for d in ["cfg", "x", "deep/inner"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        symlink(root.join("deep/inner"), root.join("x/dl")).unwrap();
        std::fs::write(root.join("deep/victim"), b"intended").unwrap();
        std::fs::write(root.join("x/victim"), b"unrelated").unwrap();
        symlink(root.join("x/dl/../victim"), root.join("cfg/config.json")).unwrap();
        assert_eq!(
            std::fs::read(root.join("cfg/config.json")).unwrap(),
            b"intended"
        );
        write_private(&root.join("cfg/config.json"), b"new", &root.join("cfg")).unwrap();
        assert_eq!(std::fs::read(root.join("x/victim")).unwrap(), b"unrelated");
        assert_eq!(std::fs::read(root.join("deep/victim")).unwrap(), b"new");
        assert_eq!(std::fs::read(root.join("cfg/config.json")).unwrap(), b"new");
        let relative = root.join("cfg/rel.json");
        symlink("../x/dl/../victim", &relative).unwrap();
        rewrite_user_file(&relative, |_| b"rel".to_vec()).unwrap();
        assert_eq!(std::fs::read(root.join("x/victim")).unwrap(), b"unrelated");
        assert_eq!(std::fs::read(root.join("deep/victim")).unwrap(), b"rel");
    }

    #[test]
    fn an_artifact_replaces_a_planted_link_instead_of_following_it() {
        let root = tmpdir("artifact");
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        std::fs::write(root.join("victim"), b"victim").unwrap();
        symlink(root.join("victim"), root.join("cfg/last-boot.json")).unwrap();
        write_private_artifact(&root.join("cfg"), "last-boot.json", b"report").unwrap();
        assert_eq!(std::fs::read(root.join("victim")).unwrap(), b"victim");
        let md = std::fs::symlink_metadata(root.join("cfg/last-boot.json")).unwrap();
        assert!(md.file_type().is_file());
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read(root.join("cfg/last-boot.json")).unwrap(),
            b"report"
        );
        for bad in ["", ".", "..", "../x", "a/b"] {
            assert!(
                write_private_artifact(&root.join("cfg"), bad, b"x").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_missing_tail_is_never_simplified() {
        let root = tmpdir("tail");
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        symlink(root.join("nope/../x"), root.join("cfg/a")).unwrap();
        assert!(resolve_write_target(&root.join("cfg/a")).is_err());
        symlink(root.join("dangling"), root.join("dl")).unwrap();
        symlink(root.join("dl/f"), root.join("cfg/b")).unwrap();
        assert!(resolve_write_target(&root.join("cfg/b")).is_err());
        assert_eq!(
            resolve_write_target(&root.join("cfg/new/f")).unwrap(),
            root.join("cfg/new/f")
        );
    }

    #[test]
    fn trusted_chain_walks_file_dir_and_parent() {
        let me = crate::current_uid();
        let root = tmpdir("chain");
        std::fs::create_dir_all(root.join("lib/pkg")).unwrap();
        std::fs::write(root.join("lib/pkg/bin"), b"").unwrap();
        for p in ["lib", "lib/pkg"] {
            std::fs::set_permissions(root.join(p), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(
            root.join("lib/pkg/bin"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let bin = root.join("lib/pkg/bin");
        let top = root.join("lib");
        assert!(trusted_chain(&bin, |u| u == me, &top).is_ok());
        assert!(trusted_chain(&bin, |u| u == me.wrapping_add(1), &top).is_err());
        for writable in ["lib", "lib/pkg"] {
            std::fs::set_permissions(root.join(writable), std::fs::Permissions::from_mode(0o775))
                .unwrap();
            assert!(
                trusted_chain(&bin, |u| u == me, &top).is_err(),
                "{writable}"
            );
            std::fs::set_permissions(root.join(writable), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        symlink(&bin, root.join("lib/pkg/link")).unwrap();
        assert!(trusted_chain(&root.join("lib/pkg/link"), |u| u == me, &top).is_err());
        assert!(trusted_chain(&bin, |u| u == me, &root.join("elsewhere")).is_err());
    }

    #[test]
    fn a_symlink_loop_is_an_error_not_a_hang() {
        let root = tmpdir("loop");
        symlink(root.join("y"), root.join("x")).unwrap();
        symlink(root.join("x"), root.join("y")).unwrap();
        assert!(resolve_write_target(&root.join("x")).is_err());
    }

    #[test]
    fn touch_private_is_0600_under_a_permissive_umask_and_keeps_an_existing_flag() {
        let root = tmpdir("flag");
        let cfg = root.join("cfg");
        let old = nix::sys::stat::umask(nix::sys::stat::Mode::empty());
        let r = touch_private(&cfg.join("skip-once"), &cfg);
        nix::sys::stat::umask(old);
        r.unwrap();
        assert_eq!(mode(&cfg.join("skip-once")), 0o600);
        assert_eq!(mode(&cfg), 0o700);
        std::fs::write(cfg.join("skip-once"), b"keep").unwrap();
        touch_private(&cfg.join("skip-once"), &cfg).unwrap();
        assert_eq!(std::fs::read(cfg.join("skip-once")).unwrap(), b"keep");
    }

    #[test]
    fn read_regular_capped_rejects_oversize_and_non_regular() {
        let root = tmpdir("read");
        std::fs::write(root.join("f"), b"12345").unwrap();
        assert!(read_regular_capped(&root.join("f"), 4).is_err());
        assert_eq!(read_regular_capped(&root.join("f"), 5).unwrap(), b"12345");
        assert!(read_regular_capped(&root, 1024).is_err());
    }
}
