//! The spawn predicate (spec §8). For a `desktop-file` entry the launcher
//! bytes are parsed into an argv that is `execve`'d directly; the signed `exec`
//! must equal that argv's canonical form, or the launcher changed since it was
//! approved. For the shell sources the signed `exec` *is* the command and keeps
//! the 1.0.7 `RELAUNCH_EXEC`-via-environment eval.

use relaunch_core::{Entry, ExecSource};
use sha2::{Digest, Sha256};

use crate::desktop::{canonical_exec_string, parse_entry};

#[derive(Debug, PartialEq, Eq)]
pub enum Spawn {
    /// Run via `bash -c` with the command passed in `RELAUNCH_EXEC` (no argv).
    Shell(String),
    /// `execve` this argv directly (desktop-file), optionally in `cwd` (Path=).
    Argv {
        argv: Vec<String>,
        cwd: Option<String>,
    },
}

/// Decide how to launch `entry`. `launcher_buffer` is the bytes the anchor read
/// from the launcher file (required for `desktop-file`, ignored otherwise). An
/// `Err(reason)` refuses the launch; the reason is recorded in `last-boot.json`.
pub fn decide(entry: &Entry, launcher_buffer: Option<&[u8]>) -> Result<Spawn, String> {
    match entry.exec_source {
        ExecSource::DesktopFile => {
            let launcher = entry
                .launcher
                .as_ref()
                .ok_or_else(|| "desktop-file without launcher".to_string())?;
            let buf = launcher_buffer.ok_or_else(|| "no launcher buffer".to_string())?;

            // If a hash was approved, the bytes must match it exactly.
            if let Some(expected) = &launcher.sha256 {
                let got = hex::encode(Sha256::digest(buf));
                if &got != expected {
                    return Err("launcher-changed".into());
                }
            }

            let text = std::str::from_utf8(buf).map_err(|_| "launcher not UTF-8".to_string())?;
            let de = parse_entry(text)?;
            // Unsupported launch semantics are refused, not silently ignored
            // (reviewer M6/I5).
            if de.dbus_activatable {
                return Err("dbus-activatable".into());
            }
            if de.terminal {
                return Err("terminal-launcher".into());
            }
            // The signed exec must be exactly the launcher's Exec as approved.
            if canonical_exec_string(&de.argv) != entry.exec {
                return Err("launcher-changed".into());
            }
            Ok(Spawn::Argv {
                argv: de.argv,
                cwd: de.path,
            })
        }
        ExecSource::OverridesTable
        | ExecSource::Terminal
        | ExecSource::Cmdline
        | ExecSource::Guess => Ok(Spawn::Shell(entry.exec.clone())),
    }
}
