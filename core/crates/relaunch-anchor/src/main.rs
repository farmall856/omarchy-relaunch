//! relaunch-anchor binary. Two subcommands, both run as the user from the
//! root-owned binary:
//!   emit-lua  verify and print the Hyprland pins (or a comment); run at login
//!             from the root-owned anchor.lua hook, before Hyprland reads them.
//!   boot      verify again and launch the eligible entries.
//!
//! Paths come only from getuid()/$HOME/`/var/lib/omarchy-relaunch`; no
//! `RELAUNCH_*` override is consulted, so a forged config/state tree in the
//! environment is ignored.
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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relaunch_anchor::boot::{self, Timing};
use relaunch_anchor::report::{self, Launch, Record};
use relaunch_anchor::{decide, hypr, launchable, load_verified, read_launcher, Spawn};
use relaunch_core::{Entry, SignedConfig};
use relaunch_sys::cmd::output_capped;
use relaunch_sys::diag;
use relaunch_sys::fs::write_private_artifact;
use relaunch_sys::paths;

const STATE_ROOT: &str = "/var/lib/omarchy-relaunch";
const TRUSTED_UID: u32 = 0;

/// The anchor's stdout is a protocol, not a log: the Lua text Hyprland
/// evaluates (`emit-lua`) or one canonical exec (`canonical-exec`). Everything
/// it prints goes through here. A write error means the reader went away; there
/// is nothing useful to do about it.
fn emit_stdout(text: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();

    // `canonical-exec <path>` is a pure, read-only utility the user-side capture
    // (the bash engine) calls to turn a launcher .desktop into the canonical
    // `exec` string it will store in a desktop-file proposal entry. It is the
    // SAME parser the boot path uses to verify that string later (§8), so the
    // two can never diverge — there is only one Exec parser. It needs no uid,
    // account, or state, so it runs before all of that.
    if arg == "canonical-exec" {
        let path = std::env::args().nth(2).unwrap_or_default();
        if path.is_empty() {
            diag!("usage: relaunch-anchor canonical-exec <path>");
            std::process::exit(2);
        }
        match canonical_exec_for(&path) {
            Ok(s) => emit_stdout(&format!("{s}\n")),
            Err(e) => {
                diag!("relaunch-anchor: canonical-exec: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let uid = relaunch_sys::current_uid();
    let Some(user) = relaunch_sys::account_name(uid) else {
        // No account: emit nothing, launch nothing.
        if arg == "emit-lua" {
            emit_stdout("-- relaunch-anchor: no account for uid\n");
        }
        return;
    };
    let config_dir = paths::default_config_dir();
    let config_path = config_dir.join(paths::CONFIG_FILE);
    let state_root = Path::new(STATE_ROOT);

    match arg.as_str() {
        "emit-lua" => {
            match load_verified(state_root, uid, &user, &config_path, TRUSTED_UID) {
                Ok(cfg) => emit_stdout(&relaunch_anchor::emit(&cfg)),
                Err(reason) => {
                    // Constant comment on stdout; the reason only on stderr, so no
                    // verifier text reaches the Hyprland-evaluated Lua (reviewer I9).
                    emit_stdout(relaunch_anchor::NOT_LAUNCHING_COMMENT);
                    diag!("relaunch-anchor: not launching ({reason})");
                }
            }
        }
        "boot" => {
            let result = load_verified(state_root, uid, &user, &config_path, TRUSTED_UID);
            run_boot(&config_dir, uid, result);
        }
        _ => {
            diag!("usage: relaunch-anchor emit-lua|boot");
            std::process::exit(2);
        }
    }
}

fn run_boot(config_dir: &Path, uid: u32, result: Result<SignedConfig, String>) {
    let started_at = iso_now();
    let mut host = RealHost {
        path: std::env::var("PATH").unwrap_or_default(),
        config_dir: config_dir.to_path_buf(),
        uid,
    };
    let record = boot::run(config_dir, result, &mut host, Timing::default());
    write_last_boot(config_dir, &started_at, record);
}

/// The real system: spawn, sleep, and read and move windows through the
/// fixed `hyprctl`.
struct RealHost {
    /// The session's `PATH`, which `execve` of a bare program name searches.
    path: String,
    /// Where the launchers are.
    config_dir: PathBuf,
    /// The user the launchers must belong to.
    uid: u32,
}

impl boot::Host for RealHost {
    fn windows(&mut self) -> Option<Vec<report::Window>> {
        hyprctl_clients()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn launch(&mut self, entry: &Entry) -> Launch {
        launch_one(entry, &self.config_dir, self.uid, &self.path)
    }
    fn place(
        &mut self,
        address: &str,
        workspace: Option<u32>,
        float: Option<bool>,
    ) -> Result<(), String> {
        let lua = hypr::place_lua(address, workspace, float).ok_or("not a window address")?;
        output_capped(paths::HYPRCTL, &["eval", &lua]).map(drop)
    }
}

/// The compositor's windows, from the fixed `hyprctl` (never `PATH`), read
/// through the shared bounded reader.
fn hyprctl_clients() -> Option<Vec<report::Window>> {
    let out = output_capped(paths::HYPRCTL, &["clients", "-j"]).ok()?;
    report::parse_clients(&out)
}

/// Now, as `date -Iseconds` prints it (local time); UTC if that fails.
fn iso_now() -> String {
    output_capped(paths::DATE, &["-Iseconds"])
        .ok()
        .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            report::iso_utc(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            )
        })
}

/// Read the launcher once, decide from those bytes, check the program would
/// run, and spawn it.
fn launch_one(entry: &Entry, config_dir: &Path, uid: u32, path: &str) -> Launch {
    let base = |status: &'static str, error: Option<String>| Launch {
        class: entry.class.clone(),
        window_class: entry.window_class.clone(),
        label: entry.label.clone(),
        workspace: entry.workspace,
        float: entry.float,
        exec: entry.exec.clone(),
        status,
        pid: None,
        error,
        placement: None,
    };
    let spawn = match read_launcher(entry, config_dir, uid).and_then(|b| decide(entry, &b)) {
        Ok(s) => s,
        Err(reason) => return base("verify-failed", Some(reason)),
    };
    if let Err(reason) = launchable(&spawn, path) {
        return base("not_found", Some(reason));
    }
    match do_spawn(&spawn) {
        Ok(mut child) => match child.try_wait() {
            Ok(None) => Launch {
                pid: Some(child.id()),
                ..base("started", None)
            },
            _ => base("failed", Some("process exited immediately".to_owned())),
        },
        Err(e) => base("failed", Some(e)),
    }
}

/// The shared canonical-exec (INV-2: the same function the engine calls).
fn canonical_exec_for(path: &str) -> Result<String, String> {
    relaunch_desktop::canonical_exec_file(path)
}

fn do_spawn(spawn: &Spawn) -> Result<std::process::Child, String> {
    let Some((program, args)) = spawn.argv.split_first() else {
        return Err("empty argv".to_string());
    };
    let mut c = Command::new(program);
    c.args(args).env_remove("BASH_ENV");
    if let Some(dir) = &spawn.cwd {
        c.current_dir(dir);
    }
    c.spawn().map_err(|e| e.to_string())
}

fn write_last_boot(config_dir: &Path, started_at: &str, record: Record) {
    let record = Record {
        started_at: started_at.to_owned(),
        ..record
    };
    let _ = write_private_artifact(config_dir, paths::LAST_BOOT_JSON, record.json().as_bytes());
    let _ = write_private_artifact(config_dir, paths::LAST_BOOT_LOG, record.log().as_bytes());
}
