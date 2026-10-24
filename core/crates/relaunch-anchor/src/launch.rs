//! The spawn predicate (spec §8). Every entry launches from its own private
//! launcher, `<config_dir>/launchers/<class>.desktop`. The anchor reads that
//! file once, and everything after works on those bytes: the hash the human
//! approved, the parse into an argv, the check that the argv is the signed
//! `exec`, and the `execve`. Nothing re-reads the path, so a file swapped
//! after the check is never what runs.

use std::path::{Path, PathBuf};

use relaunch_core::Entry;
use sha2::{Digest, Sha256};

use crate::desktop::{canonical_exec_string, parse_entry};

/// The config-dir subdirectory every launcher lives in.
pub const LAUNCHERS_SUBDIR: &str = "launchers";

/// A verified entry's launch: `execve` this argv directly, optionally in `cwd`
/// (the launcher's `Path=`).
#[derive(Debug, PartialEq, Eq)]
pub struct Spawn {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// The working directory from the launcher's `Path=`.
    pub cwd: Option<String>,
}

/// Where `class`'s launcher must be: built from the anchor's own config dir,
/// never taken from the signed path alone.
#[must_use]
pub fn launcher_path(config_dir: &Path, class: &str) -> PathBuf {
    config_dir
        .join(LAUNCHERS_SUBDIR)
        .join(format!("{class}.desktop"))
}

/// Read `entry`'s launcher: it must be the signed path, at
/// [`launcher_path`], with the file, the launchers directory and the config
/// dir each a real file or directory owned by `uid` and not group/other
/// writable. The bytes are then bound by [`decide`]'s hash check, so the walk
/// here only refuses a launcher another user could have put in place.
///
/// # Errors
///
/// `launcher-path` when the signed path is not the pin's own launcher, the
/// first untrusted component, or the read failure.
pub fn read_launcher(entry: &Entry, config_dir: &Path, uid: u32) -> Result<Vec<u8>, String> {
    let expected = launcher_path(config_dir, &entry.class);
    if Path::new(&entry.launcher.path) != expected {
        return Err("launcher-path".into());
    }
    relaunch_sys::fs::trusted_chain(&expected, |owner| owner == uid, config_dir)?;
    relaunch_sys::fs::read_regular_capped(&expected, relaunch_sys::fs::MAX_FILE_BYTES)
}

/// Decide how to launch `entry` from `buffer`, the launcher bytes the anchor
/// read. An `Err(reason)` refuses the launch; the reason is recorded in
/// `last-boot.json`.
///
/// # Errors
///
/// `launcher-changed` when the bytes are not the approved ones, or their
/// `Exec` or `Path` is not the signed `exec` or `cwd`; a parse failure; or a
/// launcher kind that is not run as a command.
pub fn decide(entry: &Entry, buffer: &[u8]) -> Result<Spawn, String> {
    if hex::encode(Sha256::digest(buffer)) != entry.launcher.sha256 {
        return Err("launcher-changed".into());
    }
    let text = std::str::from_utf8(buffer).map_err(|_| "launcher not UTF-8".to_string())?;
    let de = parse_entry(text)?;
    // Unsupported launch semantics are refused, not silently ignored.
    if de.dbus_activatable {
        return Err("dbus-activatable".into());
    }
    if de.terminal {
        return Err("terminal-launcher".into());
    }
    if canonical_exec_string(&de.argv) != entry.exec || de.path != entry.cwd {
        return Err("launcher-changed".into());
    }
    Ok(Spawn {
        argv: de.argv,
        cwd: de.path,
    })
}

/// Whether a decided spawn's program would run with this `PATH`; the reason
/// when not. The same verdict as the panel's `execOk`: both are
/// `relaunch_shell::execcheck` (INV-2).
///
/// # Errors
///
/// `command not found: <program>` when it would not run.
pub fn launchable(spawn: &Spawn, path: &str) -> Result<(), String> {
    use relaunch_shell::execcheck::{word_is_ok, Mode};
    match spawn.argv.first() {
        Some(p) if word_is_ok(p, path, Mode::Direct) => Ok(()),
        Some(p) => Err(format!("command not found: {p}")),
        None => Err("command not found: ".into()),
    }
}
