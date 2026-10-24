//! Chunk-4 tests: the Desktop Entry parser, the launch predicate, Lua emission,
//! and login verification (trust chain + signature binding).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::SigningKey;
use relaunch_anchor::boot::SCHEMA_UPGRADE_NEEDED;
use relaunch_anchor::desktop::{canonical_exec_string, parse_exec};
use relaunch_anchor::{
    decide, emit, launcher_path, load_verified, read_launcher, trusted_chain, Spawn,
    BOOT_REGISTRATION,
};
use relaunch_core::{
    sign, to_config_file, Entry, ExecSource, Launcher, Provenance, SignedConfig,
    SIGNED_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

static N: AtomicU64 = AtomicU64::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "anchor-test-{}-{}-{}",
            std::process::id(),
            tag,
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

// ---- Desktop Entry Exec parser ----

#[test]
fn parse_exec_strips_field_codes() {
    let d = "[Desktop Entry]\nExec=brave %U\n";
    assert_eq!(parse_exec(d).unwrap(), vec!["brave"]);
}

#[test]
fn parse_exec_handles_quoted_path_with_space() {
    let d = "[Desktop Entry]\nExec=\"/opt/My App/run\" %f\n";
    assert_eq!(parse_exec(d).unwrap(), vec!["/opt/My App/run"]);
    assert_eq!(
        canonical_exec_string(&parse_exec(d).unwrap()),
        "/opt/My\\ App/run"
    );
}

#[test]
fn parse_exec_keeps_inner_args_and_percent_literal() {
    let d = "[Desktop Entry]\nExec=sh -c \"foo bar\" %% baz\n";
    assert_eq!(
        parse_exec(d).unwrap(),
        vec!["sh", "-c", "foo bar", "%", "baz"]
    );
}

#[test]
fn parse_exec_only_reads_the_desktop_entry_group() {
    let d = "[Desktop Action new]\nExec=evil\n[Desktop Entry]\nExec=good\n";
    assert_eq!(parse_exec(d).unwrap(), vec!["good"]);
}

#[test]
fn parse_exec_rejects_unterminated_quote_and_trailing_backslash() {
    assert!(parse_exec("[Desktop Entry]\nExec=\"oops\n").is_err());
    assert!(parse_exec("[Desktop Entry]\nExec=foo\\").is_err());
}

// ---- launch predicate ----

const ID: &str = "relauncher-00000000000000b1";

fn pinned(exec: &str, buf: &[u8]) -> Entry {
    Entry {
        class: ID.into(),
        window_class: "brave-browser".into(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: exec.into(),
        exec_source: ExecSource::DesktopFile,
        provenance: Provenance::None,
        launcher: Launcher {
            path: format!("/h/.config/omarchy-relaunch/launchers/{ID}.desktop"),
            sha256: hex::encode(Sha256::digest(buf)),
        },
        cwd: None,
        enabled: true,
    }
}

#[test]
fn decide_matches_and_returns_argv() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/brave %U\n";
    let argv = parse_exec(std::str::from_utf8(buf).unwrap()).unwrap();
    let entry = pinned(&canonical_exec_string(&argv), buf);
    assert_eq!(
        decide(&entry, buf).unwrap(),
        Spawn {
            argv: vec!["/usr/bin/brave".into()],
            cwd: None
        }
    );
}

#[test]
fn decide_exec_mismatch_is_launcher_changed() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/evil %U\n";
    assert_eq!(
        decide(&pinned("/usr/bin/brave", buf), buf),
        Err("launcher-changed".into())
    );
}

#[test]
fn decide_hash_mismatch_is_launcher_changed() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/brave %U\n";
    let mut entry = pinned("/usr/bin/brave", buf);
    entry.launcher.sha256 = "00".repeat(32);
    assert_eq!(decide(&entry, buf), Err("launcher-changed".into()));
}

#[test]
fn parse_exec_expands_glued_field_code() {
    // --file=%f keeps the literal prefix; the bare code expands to nothing.
    let d = "[Desktop Entry]\nExec=app --file=%f --tag\n";
    assert_eq!(parse_exec(d).unwrap(), vec!["app", "--file=", "--tag"]);
}

#[test]
fn parse_exec_rejects_invalid_field_code() {
    assert!(parse_exec("[Desktop Entry]\nExec=app %z\n").is_err());
}

#[test]
fn decide_refuses_dbus_activatable() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nDBusActivatable=true\n";
    assert_eq!(
        decide(&pinned("/usr/bin/x", buf), buf),
        Err("dbus-activatable".into())
    );
}

#[test]
fn decide_refuses_terminal_launcher() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nTerminal=true\n";
    assert_eq!(
        decide(&pinned("/usr/bin/x", buf), buf),
        Err("terminal-launcher".into())
    );
}

#[test]
fn decide_carries_path_as_cwd_only_as_signed() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nPath=/opt/work\n";
    let mut entry = pinned("/usr/bin/x", buf);
    entry.cwd = Some("/opt/work".into());
    assert_eq!(
        decide(&entry, buf),
        Ok(Spawn {
            argv: vec!["/usr/bin/x".into()],
            cwd: Some("/opt/work".into())
        })
    );
    for signed in [None, Some("/opt/other".to_owned())] {
        entry.cwd = signed;
        assert_eq!(decide(&entry, buf), Err("launcher-changed".into()));
    }
    let unsigned_path = b"[Desktop Entry]\nExec=/usr/bin/x\n";
    let mut entry = pinned("/usr/bin/x", unsigned_path);
    entry.cwd = Some("/opt/work".into());
    assert_eq!(
        decide(&entry, unsigned_path),
        Err("launcher-changed".into())
    );
}

// ---- reading the private launcher ----

/// A config dir holding `entry`'s launcher with `bytes`, all `0700`/`0600`.
fn launcher_tree(tmp: &Tmp, bytes: &[u8]) -> (PathBuf, Entry) {
    let cfg = tmp.0.join("omarchy-relaunch");
    let dir = cfg.join("launchers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = launcher_path(&cfg, ID);
    std::fs::write(&file, bytes).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut entry = pinned("/usr/bin/x", bytes);
    entry.launcher.path = file.to_string_lossy().into_owned();
    (cfg, entry)
}

const LAUNCHER: &[u8] = b"[Desktop Entry]\nType=Application\nExec=/usr/bin/x\n";

#[test]
fn a_private_launcher_is_read_and_decided_from_the_bytes_read() {
    let tmp = Tmp::new("launcher-ok");
    let (cfg, entry) = launcher_tree(&tmp, LAUNCHER);
    let bytes = read_launcher(&entry, &cfg, euid()).unwrap();
    assert_eq!(bytes, LAUNCHER);
    // Replacing the file after the read changes nothing about what runs: the
    // decision is made from the bytes in hand, and they still match.
    std::fs::write(
        launcher_path(&cfg, ID),
        b"[Desktop Entry]\nExec=/usr/bin/evil\n",
    )
    .unwrap();
    assert_eq!(decide(&entry, &bytes).unwrap().argv, ["/usr/bin/x"]);
    // A fresh read of the swapped file is refused by the hash.
    let swapped = read_launcher(&entry, &cfg, euid()).unwrap();
    assert_eq!(decide(&entry, &swapped), Err("launcher-changed".into()));
}

#[test]
fn only_the_pins_own_launcher_in_this_config_dir_is_read() {
    let tmp = Tmp::new("launcher-path");
    let (cfg, mut entry) = launcher_tree(&tmp, LAUNCHER);
    let elsewhere = tmp.0.join("other/omarchy-relaunch/launchers");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join(format!("{ID}.desktop")), LAUNCHER).unwrap();
    entry.launcher.path = elsewhere
        .join(format!("{ID}.desktop"))
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        read_launcher(&entry, &cfg, euid()),
        Err("launcher-path".into())
    );
}

#[test]
fn a_launcher_another_user_could_have_placed_is_refused() {
    for (what, mode_of) in [("file", 0), ("dir", 1), ("config", 2)] {
        let tmp = Tmp::new(&format!("launcher-{what}"));
        let (cfg, entry) = launcher_tree(&tmp, LAUNCHER);
        let target = match mode_of {
            0 => launcher_path(&cfg, ID),
            1 => cfg.join("launchers"),
            _ => cfg.clone(),
        };
        let mode = if mode_of == 0 { 0o620 } else { 0o770 };
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            read_launcher(&entry, &cfg, euid()).is_err(),
            "a group-writable {what} must be refused"
        );
        assert!(
            read_launcher(&entry, &cfg, euid().wrapping_add(1)).is_err(),
            "{what}: another uid's tree must be refused"
        );
    }
    let tmp = Tmp::new("launcher-link");
    let (cfg, entry) = launcher_tree(&tmp, LAUNCHER);
    let real = tmp.0.join("real.desktop");
    std::fs::write(&real, LAUNCHER).unwrap();
    std::fs::remove_file(launcher_path(&cfg, ID)).unwrap();
    symlink(&real, launcher_path(&cfg, ID)).unwrap();
    assert!(
        read_launcher(&entry, &cfg, euid()).is_err(),
        "a symlink is refused"
    );
}

// ---- bounded, typed reads ----

#[test]
fn read_regular_capped_rejects_a_fifo() {
    use relaunch_sys::fs::read_regular_capped;
    let tmp = Tmp::new("fifo");
    let fifo = tmp.0.join("f");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    // O_NONBLOCK means the open returns; the S_ISREG check then rejects it,
    // rather than login blocking forever on a reader.
    assert!(read_regular_capped(&fifo, 1024).is_err());
}

#[test]
fn read_regular_capped_rejects_oversize() {
    use relaunch_sys::fs::read_regular_capped;
    let tmp = Tmp::new("big");
    let f = tmp.0.join("big");
    std::fs::write(&f, vec![b'x'; 2048]).unwrap();
    assert!(read_regular_capped(&f, 1024).is_err());
    assert!(read_regular_capped(&f, 4096).is_ok());
}

#[test]
fn the_anchor_runs_argv_directly_and_never_a_shell_or_gio() {
    let main_src = include_str!("../src/main.rs");
    for gone in ["RELAUNCH_EXEC", "/usr/bin/bash", "\"-c\"", "gio"] {
        assert!(!main_src.contains(gone), "{gone}");
    }
    assert!(main_src.contains("env_remove(\"BASH_ENV\")"));
}

// ---- Lua emission ----

fn shell_entry(name: &str, ws: u32, enabled: bool, float: Option<bool>) -> Entry {
    let mut e = pinned(&format!("run-{name}"), b"x");
    e.window_class = name.into();
    e.workspace = ws;
    e.enabled = enabled;
    e.float = float;
    e
}

fn cfg(entries: Vec<Entry>) -> SignedConfig {
    SignedConfig {
        v: SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "laytonf".into(),
        gen: 1,
        pre_launch_ms: 0,
        post_launch_ms: 500,
        entries,
    }
}

#[test]
fn emit_is_only_the_boot_registration() {
    let c = cfg(vec![shell_entry("brave", 2, true, Some(false))]);
    let out = emit(&c);
    assert!(out.ends_with(BOOT_REGISTRATION));
    assert!(!out.contains("o.window"), "{out}");
    assert_eq!(out.lines().filter(|l| !l.starts_with("--")).count(), 1);
}

// ---- verification ----

fn write_state(
    tmp: &Tmp,
    uid: u32,
    user: &str,
    cfg: &SignedConfig,
    key: &SigningKey,
) -> (PathBuf, PathBuf) {
    let state_root = tmp.0.join("var");
    let udir = state_root.join(uid.to_string());
    std::fs::create_dir_all(&udir).unwrap();
    let pubkey = key.verifying_key().to_bytes();
    let public = serde_json::json!({
        "v": 1, "uid": uid, "user": user, "gen": cfg.gen,
        "enrollmentId": hex::encode(&Sha256::digest(pubkey)[..16]),
        "pubkey": hex::encode(pubkey),
    });
    std::fs::write(
        udir.join("public.json"),
        serde_json::to_vec(&public).unwrap(),
    )
    .unwrap();
    let cfg_dir = tmp.0.join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let sig = sign(key, cfg);
    let file = to_config_file(cfg, &sig, vec![]);
    let cfg_path = cfg_dir.join("config.json");
    std::fs::write(&cfg_path, serde_json::to_vec(&file).unwrap()).unwrap();
    (state_root, cfg_path)
}

#[test]
fn load_verified_accepts_a_good_signed_config() {
    let tmp = Tmp::new("ok");
    let uid = euid();
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let mut c = cfg(vec![shell_entry("brave", 1, true, None)]);
    c.uid = uid;
    c.user = "tester".into();
    let (state_root, cfg_path) = write_state(&tmp, uid, "tester", &c, &key);
    let out = load_verified(&state_root, uid, "tester", &cfg_path, euid()).unwrap();
    assert_eq!(out.entries.len(), 1);
}

#[test]
fn load_verified_rejects_tampered_config() {
    let tmp = Tmp::new("tamper");
    let uid = euid();
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let mut c = cfg(vec![shell_entry("brave", 1, true, None)]);
    c.uid = uid;
    c.user = "tester".into();
    let (state_root, cfg_path) = write_state(&tmp, uid, "tester", &c, &key);
    let s = std::fs::read_to_string(&cfg_path)
        .unwrap()
        .replace("\"exec\":\"run-brave\"", "\"exec\":\"evil12345678\"");
    std::fs::write(&cfg_path, s).unwrap();
    assert!(load_verified(&state_root, uid, "tester", &cfg_path, euid()).is_err());
}

#[test]
fn load_verified_rejects_wrong_user_binding() {
    let tmp = Tmp::new("user");
    let uid = euid();
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let mut c = cfg(vec![shell_entry("brave", 1, true, None)]);
    c.uid = uid;
    c.user = "tester".into();
    let (state_root, cfg_path) = write_state(&tmp, uid, "tester", &c, &key);
    assert!(load_verified(&state_root, uid, "someone-else", &cfg_path, euid()).is_err());
}

#[test]
fn load_verified_rejects_an_unknown_public_json_version() {
    let tmp = Tmp::new("pubver");
    let uid = euid();
    let key = SigningKey::from_bytes(&[6u8; 32]);
    let mut c = cfg(vec![shell_entry("brave", 1, true, None)]);
    c.uid = uid;
    c.user = "tester".into();
    let (state_root, cfg_path) = write_state(&tmp, uid, "tester", &c, &key);
    assert!(load_verified(&state_root, uid, "tester", &cfg_path, euid()).is_ok());
    let public = state_root.join(uid.to_string()).join("public.json");
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&public).unwrap()).unwrap();
    v["v"] = serde_json::json!(2);
    std::fs::write(&public, serde_json::to_vec(&v).unwrap()).unwrap();
    let err = load_verified(&state_root, uid, "tester", &cfg_path, euid()).unwrap_err();
    assert!(err.contains("version"), "{err}");
}

#[test]
fn a_v1_config_is_named_as_needing_a_fresh_save() {
    let tmp = Tmp::new("v1");
    let uid = euid();
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let mut c = cfg(vec![shell_entry("brave", 1, true, None)]);
    c.uid = uid;
    c.user = "tester".into();
    let (state_root, cfg_path) = write_state(&tmp, uid, "tester", &c, &key);
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cfg_path).unwrap()).unwrap();
    v["signed"]["v"] = serde_json::json!(1);
    std::fs::write(&cfg_path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(
        load_verified(&state_root, uid, "tester", &cfg_path, euid()),
        Err(SCHEMA_UPGRADE_NEEDED.to_owned())
    );
}

#[test]
fn trusted_chain_rejects_symlinked_public_json() {
    let tmp = Tmp::new("symlink");
    let state_root = tmp.0.join("var");
    let udir = state_root.join("1000");
    std::fs::create_dir_all(&udir).unwrap();
    let real = tmp.0.join("evil.json");
    std::fs::write(&real, b"{}").unwrap();
    let link = udir.join("public.json");
    symlink(&real, &link).unwrap();
    assert!(trusted_chain(&link, euid(), &state_root).is_err());
}

#[test]
fn trusted_chain_rejects_group_writable_component() {
    let tmp = Tmp::new("writable");
    let state_root = tmp.0.join("var");
    let udir = state_root.join("1000");
    std::fs::create_dir_all(&udir).unwrap();
    let pubf = udir.join("public.json");
    std::fs::write(&pubf, b"{}").unwrap();
    std::fs::set_permissions(&udir, std::fs::Permissions::from_mode(0o775)).unwrap();
    assert!(trusted_chain(&pubf, euid(), &state_root).is_err());
}

#[test]
fn trusted_chain_accepts_tight_chain() {
    let tmp = Tmp::new("tight");
    let state_root = tmp.0.join("var");
    let udir = state_root.join("1000");
    std::fs::create_dir_all(&udir).unwrap();
    std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&udir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pubf = udir.join("public.json");
    std::fs::write(&pubf, b"{}").unwrap();
    std::fs::set_permissions(&pubf, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(trusted_chain(&pubf, euid(), &state_root).is_ok());
}

// ---- the binary consults no RELAUNCH_* override ----

#[test]
fn binary_reads_no_relaunch_env_override() {
    let main_src = include_str!("../src/main.rs");
    for forbidden in [
        "RELAUNCH_CONFIG_DIR",
        "RELAUNCH_STATE_DIR",
        "RELAUNCH_CMDLINE_DIR",
    ] {
        assert!(
            !main_src.contains(forbidden),
            "anchor must not read {forbidden}"
        );
    }
}

#[test]
fn decide_unescapes_path_for_cwd() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nPath=/opt/my\\sdir\n";
    let mut entry = pinned("/usr/bin/x", buf);
    entry.cwd = Some("/opt/my dir".into());
    assert_eq!(
        decide(&entry, buf),
        Ok(Spawn {
            argv: vec!["/usr/bin/x".into()],
            cwd: Some("/opt/my dir".into())
        })
    );
}

#[test]
fn parse_exec_refuses_informational_field_code() {
    assert!(parse_exec("[Desktop Entry]\nExec=app %c\n").is_err());
    assert!(parse_exec("[Desktop Entry]\nExec=app %k\n").is_err());
}

/// The panel's `execOk` (`execcheck` on the stored exec, as argv) and boot's
/// `not_found` (`launchable` on the decided spawn) are one verdict.
#[test]
fn boot_and_the_panel_judge_a_command_alike() {
    use relaunch_anchor::{launchable, Spawn};
    use relaunch_shell::execcheck::{exec_is_ok_in, Mode};

    let dir = std::env::temp_dir().join(format!("relaunch-verdict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("my bin")).unwrap();
    let prog = dir.join("my bin/tool");
    std::fs::write(&prog, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = "/usr/bin:/bin";
    let argvs: Vec<Vec<String>> = vec![
        vec![prog.to_string_lossy().into_owned(), "--x".into()],
        vec!["sh".into(), "-c".into(), "true".into()],
        vec!["definitely-not-installed-xyz".into()],
        vec![dir.join("my bin").to_string_lossy().into_owned()],
        vec!["cd".into()],
    ];
    for argv in argvs {
        let shown = canonical_exec_string(&argv);
        let boot = launchable(
            &Spawn {
                argv: argv.clone(),
                cwd: None,
            },
            path,
        )
        .is_ok();
        assert_eq!(
            boot,
            exec_is_ok_in(&shown, path, Mode::Direct),
            "argv {argv:?} shown as {shown:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The boot reports are Relaunch's own files: a symlink planted at either name
/// is replaced, never followed, so a report cannot land outside the config dir.
#[test]
fn boot_reports_replace_a_planted_link_and_never_write_through_it() {
    let t = Tmp::new("report-links");
    let cfg = t.0.join("xdg/omarchy-relaunch");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(t.0.join("outside")).unwrap();
    std::fs::write(cfg.join("config.json"), b"{not a signed document").unwrap();
    for name in ["last-boot.json", "last-boot.log"] {
        let victim = t.0.join("outside").join(name);
        std::fs::write(&victim, b"victim").unwrap();
        std::os::unix::fs::symlink(&victim, cfg.join(name)).unwrap();
    }
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_relaunch-anchor"))
        .arg("boot")
        .env_clear()
        .env("HOME", &t.0)
        .env("XDG_CONFIG_HOME", t.0.join("xdg"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    for name in ["last-boot.json", "last-boot.log"] {
        assert_eq!(
            std::fs::read(t.0.join("outside").join(name)).unwrap(),
            b"victim",
            "{name}"
        );
        let md = std::fs::symlink_metadata(cfg.join(name)).unwrap();
        assert!(
            md.file_type().is_file(),
            "{name} must be a regular file now"
        );
        assert!(!std::fs::read(cfg.join(name)).unwrap().is_empty(), "{name}");
    }
}
