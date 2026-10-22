//! Chunk-4 tests: the Desktop Entry parser, the launch predicate, Lua emission,
//! and login verification (trust chain + signature binding).

use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::SigningKey;
use relaunch_anchor::desktop::{canonical_exec_string, parse_exec};
use relaunch_anchor::{decide, emit, load_verified, trusted_chain, Spawn, BOOT_REGISTRATION};
use relaunch_core::{sign, to_config_file, Entry, ExecSource, Launcher, Provenance, SignedConfig};
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
    unsafe { libc::geteuid() }
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

fn desktop_entry(exec: &str, path: &str, sha: Option<String>) -> Entry {
    Entry {
        class: "brave".into(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: exec.into(),
        exec_source: ExecSource::DesktopFile,
        provenance: Provenance::None,
        launcher: Some(Launcher {
            path: path.into(),
            sha256: sha,
        }),
        enabled: true,
    }
}

#[test]
fn decide_desktop_file_matches_and_returns_argv() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/brave %U\n";
    let sha = hex::encode(Sha256::digest(buf));
    let argv = parse_exec(std::str::from_utf8(buf).unwrap()).unwrap();
    let entry = desktop_entry(
        &canonical_exec_string(&argv),
        "/usr/share/applications/brave.desktop",
        Some(sha),
    );
    assert_eq!(
        decide(&entry, Some(buf)).unwrap(),
        Spawn::Argv {
            argv: vec!["/usr/bin/brave".into()],
            cwd: None
        }
    );
}

#[test]
fn decide_desktop_file_exec_mismatch_is_launcher_changed() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/evil %U\n";
    let sha = hex::encode(Sha256::digest(buf));
    let entry = desktop_entry(
        "/usr/bin/brave",
        "/usr/share/applications/brave.desktop",
        Some(sha),
    );
    assert_eq!(decide(&entry, Some(buf)), Err("launcher-changed".into()));
}

#[test]
fn decide_desktop_file_hash_mismatch_is_launcher_changed() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/brave %U\n";
    let entry = desktop_entry("/usr/bin/brave", "/x.desktop", Some("00".repeat(32)));
    assert_eq!(decide(&entry, Some(buf)), Err("launcher-changed".into()));
}

#[test]
fn decide_shell_source_runs_the_signed_string() {
    let entry = Entry {
        class: "term".into(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: "xdg-terminal-exec --app-id=term -e nvim".into(),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: None,
        enabled: true,
    };
    assert_eq!(
        decide(&entry, None),
        Ok(Spawn::Shell(
            "xdg-terminal-exec --app-id=term -e nvim".into()
        ))
    );
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
    let sha = hex::encode(Sha256::digest(buf));
    let entry = desktop_entry("/usr/bin/x", "/usr/share/applications/x.desktop", Some(sha));
    assert_eq!(decide(&entry, Some(buf)), Err("dbus-activatable".into()));
}

#[test]
fn decide_refuses_terminal_launcher() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nTerminal=true\n";
    let sha = hex::encode(Sha256::digest(buf));
    let entry = desktop_entry("/usr/bin/x", "/usr/share/applications/x.desktop", Some(sha));
    assert_eq!(decide(&entry, Some(buf)), Err("terminal-launcher".into()));
}

#[test]
fn decide_desktop_file_carries_path_as_cwd() {
    let buf = b"[Desktop Entry]\nExec=/usr/bin/x\nPath=/opt/work\n";
    let sha = hex::encode(Sha256::digest(buf));
    let entry = desktop_entry("/usr/bin/x", "/usr/share/applications/x.desktop", Some(sha));
    assert_eq!(
        decide(&entry, Some(buf)),
        Ok(Spawn::Argv {
            argv: vec!["/usr/bin/x".into()],
            cwd: Some("/opt/work".into())
        })
    );
}

// ---- bounded, typed reads ----

#[test]
fn read_regular_capped_rejects_a_fifo() {
    use relaunch_anchor::fsio::read_regular_capped;
    let tmp = Tmp::new("fifo");
    let fifo = tmp.0.join("f");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // O_NONBLOCK means the open returns; the S_ISREG check then rejects it,
    // rather than login blocking forever on a reader.
    assert!(read_regular_capped(&fifo, 1024).is_err());
}

#[test]
fn read_regular_capped_rejects_oversize() {
    use relaunch_anchor::fsio::read_regular_capped;
    let tmp = Tmp::new("big");
    let f = tmp.0.join("big");
    std::fs::write(&f, vec![b'x'; 2048]).unwrap();
    assert!(read_regular_capped(&f, 1024).is_err());
    assert!(read_regular_capped(&f, 4096).is_ok());
}

#[test]
fn anchor_uses_absolute_bash_not_path() {
    let main_src = include_str!("../src/main.rs");
    assert!(main_src.contains("/usr/bin/bash"));
    assert!(!main_src.contains("Command::new(\"bash\")"));
    assert!(main_src.contains("env_remove(\"BASH_ENV\")"));
}

// ---- Lua emission ----

fn shell_entry(class: &str, ws: u32, enabled: bool, float: Option<bool>) -> Entry {
    Entry {
        class: class.into(),
        workspace: ws,
        float,
        label: String::new(),
        exec: format!("run-{class}"),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: None,
        enabled,
    }
}

fn cfg(entries: Vec<Entry>) -> SignedConfig {
    SignedConfig {
        v: 1,
        uid: 1000,
        user: "laytonf".into(),
        gen: 1,
        stagger_seconds: 0,
        entries,
    }
}

#[test]
fn emit_pins_only_enabled_and_always_registers_boot() {
    let c = cfg(vec![
        shell_entry("brave", 2, true, None),
        shell_entry("off", 3, false, None),
    ]);
    let out = emit(&c);
    assert!(out.contains("class = \"^(brave)$\""));
    assert!(out.contains("workspace = \"2 silent\""));
    assert!(!out.contains("^(off)$"));
    assert!(out.ends_with(BOOT_REGISTRATION));
}

#[test]
fn emit_tiled_entry_removes_floating_tag_first() {
    let c = cfg(vec![shell_entry("herdr", 1, true, Some(false))]);
    let out = emit(&c);
    assert!(out.contains("{ tag = \"-floating-window\" }"));
    assert!(out.contains("tile = true"));
}

#[test]
fn emit_escapes_dotted_class_for_lua_and_regex() {
    let c = cfg(vec![shell_entry("org.omarchy.agent", 1, true, Some(true))]);
    let out = emit(&c);
    // Dots regex-escaped then backslash-doubled for the Lua string literal.
    assert!(out.contains("^(org\\\\.omarchy\\\\.agent)$"));
    assert!(out.contains("float = true"));
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
        .replace("run-brave", "evil12345678");
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
    let sha = hex::encode(Sha256::digest(buf));
    let entry = desktop_entry("/usr/bin/x", "/usr/share/applications/x.desktop", Some(sha));
    assert_eq!(
        decide(&entry, Some(buf)),
        Ok(Spawn::Argv {
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
