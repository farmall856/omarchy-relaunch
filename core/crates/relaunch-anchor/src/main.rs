//! relaunch-anchor binary. Two subcommands, both run as the user from the
//! root-owned binary:
//!   emit-lua  verify and print the Hyprland pins (or a comment); run at login
//!             from the root-owned anchor.lua hook, before Hyprland reads them.
//!   boot      verify again and launch the eligible entries.
//!
//! Paths come only from getuid()/$HOME/`/var/lib/omarchy-relaunch`; no
//! `RELAUNCH_*` override is consulted, so a forged config/state tree in the
//! environment is ignored.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relaunch_anchor::{decide, emit_lua_output, load_verified, verify::trusted_chain, Spawn};
use relaunch_core::{Entry, ExecSource, SignedConfig};

const STATE_ROOT: &str = "/var/lib/omarchy-relaunch";
const TRUSTED_UID: u32 = 0;
const STAGGER_MAX: u32 = 60;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let uid = unsafe { libc::getuid() } as u32;
    let user = match name_for_uid(uid) {
        Some(u) => u,
        None => {
            // No account: emit nothing, launch nothing.
            if arg == "emit-lua" {
                println!("-- relaunch-anchor: no account for uid");
            }
            return;
        }
    };
    let config_dir = config_dir();
    let config_path = config_dir.join("config.json");
    let state_root = Path::new(STATE_ROOT);

    match arg.as_str() {
        "emit-lua" => {
            let result = load_verified(state_root, uid, &user, &config_path, TRUSTED_UID);
            print!("{}", emit_lua_output(&result));
        }
        "boot" => {
            let result = load_verified(state_root, uid, &user, &config_path, TRUSTED_UID);
            run_boot(&config_dir, result);
        }
        _ => {
            eprintln!("usage: relaunch-anchor emit-lua|boot");
            std::process::exit(2);
        }
    }
}

fn run_boot(config_dir: &Path, result: Result<SignedConfig, String>) {
    let started = now();
    let cfg = match result {
        Ok(c) => c,
        Err(reason) => {
            write_last_boot(config_dir, started, "unverified", &reason, &[]);
            return;
        }
    };

    if config_dir.join("disabled").exists() {
        write_last_boot(config_dir, started, "disabled", "boot disabled", &[]);
        return;
    }
    if config_dir.join("skip-once").exists() {
        let _ = fs::remove_file(config_dir.join("skip-once"));
        write_last_boot(config_dir, started, "skipped", "skip-once consumed", &[]);
        return;
    }

    let stagger = cfg.stagger_seconds.min(STAGGER_MAX);
    let mut statuses: Vec<(String, String)> = Vec::new();
    let mut first = true;
    for entry in cfg.entries.iter().filter(|e| e.enabled) {
        if !first && stagger > 0 {
            std::thread::sleep(Duration::from_secs(stagger as u64));
        }
        first = false;
        statuses.push((entry.class.clone(), launch_one(entry)));
    }
    write_last_boot(config_dir, started, "launched", "", &statuses);
}

/// Read the launcher bytes (desktop-file), decide, and spawn. Returns the
/// status string recorded for the entry.
fn launch_one(entry: &Entry) -> String {
    let buffer = match entry.exec_source {
        ExecSource::DesktopFile => match read_launcher(entry) {
            Ok(b) => Some(b),
            Err(reason) => return format!("unverified:{reason}"),
        },
        _ => None,
    };
    match decide(entry, buffer.as_deref()) {
        Ok(spawn) => match do_spawn(&spawn) {
            Ok(()) => "started".to_string(),
            Err(e) => format!("failed:{e}"),
        },
        Err(reason) => format!("unverified:{reason}"),
    }
}

/// Resolve the launcher path (following symlinks to the real target) and read
/// its bytes. For a launcher with no approved hash (a root-prefix launcher), the
/// resolved file and its whole chain must be root-owned and not
/// unprivileged-writable; a hashed launcher is bound by its bytes instead.
fn read_launcher(entry: &Entry) -> Result<Vec<u8>, String> {
    let launcher = entry
        .launcher
        .as_ref()
        .ok_or_else(|| "no launcher".to_string())?;
    let resolved =
        fs::canonicalize(&launcher.path).map_err(|e| format!("resolve launcher: {e}"))?;
    if launcher.sha256.is_none() {
        trusted_chain(&resolved, TRUSTED_UID, Path::new("/"))?;
    }
    fs::read(&resolved).map_err(|e| format!("read launcher: {e}"))
}

fn do_spawn(spawn: &Spawn) -> Result<(), String> {
    let child = match spawn {
        // 1.0.7 contract: the command reaches bash through the environment, not
        // argv, and is unset before it runs so the app does not inherit it.
        Spawn::Shell(cmd) => Command::new("bash")
            .arg("-c")
            .arg("c=$RELAUNCH_EXEC; unset RELAUNCH_EXEC; eval \"$c\"")
            .env("RELAUNCH_EXEC", cmd)
            .spawn(),
        Spawn::Argv(argv) => {
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]);
            c.spawn()
        }
    };
    child.map(|_| ()).map_err(|e| e.to_string())
}

fn write_last_boot(
    config_dir: &Path,
    started: u64,
    outcome: &str,
    note: &str,
    statuses: &[(String, String)],
) {
    let entries: Vec<serde_json::Value> = statuses
        .iter()
        .map(|(class, status)| serde_json::json!({ "class": class, "status": status }))
        .collect();
    let doc = serde_json::json!({
        "startedAt": started,
        "outcome": outcome,
        "note": note,
        "entries": entries,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
        let _ = fs::write(config_dir.join("last-boot.json"), bytes);
    }
}

fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("omarchy-relaunch");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("omarchy-relaunch")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Current login name for a uid via getpwuid_r (trusted account lookup).
fn name_for_uid(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(pwd.pw_name) };
        return name.to_str().ok().map(|s| s.to_string());
    }
}
