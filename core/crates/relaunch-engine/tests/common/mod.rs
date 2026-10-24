//! The fixture world the engine's contract tests share: a fake `hyprctl`, a
//! `/proc` fixture tree, one `.desktop` file and a signed base.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    dead_code,
    reason = "test code shared by several test crates; each uses a subset"
)]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

pub struct World(pub PathBuf);

impl World {
    pub fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("relaunch-propose-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["cfg", "cmd", "data/applications"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        World(dir)
    }
    pub fn write(&self, rel: &str, body: &[u8]) {
        std::fs::write(self.0.join(rel), body).unwrap();
    }
    /// Run the fixtures engine on `args`, feeding `stdin`.
    pub fn run(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_relaunch"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.0.join("home"))
            .env("RELAUNCH_CONFIG_DIR", self.0.join("cfg"))
            .env("RELAUNCH_CMDLINE_DIR", self.0.join("cmd"))
            .env("RELAUNCH_DATA_DIRS", self.0.join("data"))
            .env("HYPRCTL", self.0.join("hyprctl"))
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }
    pub fn propose(&self) -> Output {
        self.run(&["propose", "--json"], b"")
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub const CLIENTS: &str = r#"[
  {"address":"0x1","pid":10,"class":"MyApp","initialClass":"MyApp","workspace":{"id":2},"floating":false,"xwayland":false},
  {"address":"0x2","pid":11,"class":"__proto__","initialClass":"__proto__","workspace":{"id":3},"floating":false,"xwayland":false},
  {"address":"0x3","pid":12,"class":"constructor","initialClass":"constructor","workspace":{"id":3},"floating":false,"xwayland":false},
  {"address":"0x4","pid":13,"class":"Back\\slash","initialClass":"Back\\slash","workspace":{"id":4},"floating":false,"xwayland":false},
  {"address":"0x5","pid":14,"class":"Typed","initialClass":"Typed","workspace":{"id":5},"floating":true,"xwayland":false},
  {"address":"0x6","pid":15,"class":"Off","initialClass":"Off","workspace":{"id":6},"floating":false,"xwayland":false},
  {"address":"0x7","pid":16,"class":"Scratch","initialClass":"Scratch","workspace":{"id":-98},"floating":true,"xwayland":false},
  {"address":"0x8","pid":17,"class":"Sandboxed","initialClass":"Sandboxed","workspace":{"id":7},"floating":false,"xwayland":false},
  {"address":"0x9","pid":18,"class":"foot","initialClass":"foot","workspace":{"id":8},"floating":false,"xwayland":false}
]"#;

/// The base's pin ids.
pub const GONE: &str = "relauncher-000000000000000a";
pub const OFF: &str = "relauncher-000000000000000b";
pub const TYPED: &str = "relauncher-000000000000000c";

/// One base pin, bound to its launcher in `cfg_dir`.
pub fn base_pin(
    cfg_dir: &std::path::Path,
    (id, window_class, workspace): (&str, &str, u32),
    label: &str,
    exec: &str,
    source: relaunch_core::ExecSource,
    enabled: bool,
) -> relaunch_core::Entry {
    use relaunch_core::{ExecSource, Provenance};
    relaunch_core::Entry {
        class: id.into(),
        window_class: window_class.into(),
        workspace,
        float: (source == ExecSource::OverridesTable).then_some(false),
        label: label.into(),
        exec: exec.into(),
        exec_source: source,
        provenance: if source == ExecSource::OverridesTable {
            Provenance::None
        } else {
            Provenance::Host
        },
        launcher: relaunch_engine::launcher::binding(cfg_dir, id, label, exec, None).unwrap(),
        cwd: None,
        enabled,
    }
}

/// A signed (structurally; the signature is not checked here) base for
/// `cfg_dir`: Gone (ws 1), Off (ws 6, disabled, labelled), Typed (ws 5, a
/// typed command).
pub fn base_doc_in(cfg_dir: &std::path::Path) -> String {
    use relaunch_core::ExecSource::{Cmdline, OverridesTable};
    let signed = relaunch_core::SignedConfig {
        v: relaunch_core::SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "u".into(),
        gen: 1,
        pre_launch_ms: 0,
        post_launch_ms: 300,
        entries: vec![
            base_pin(cfg_dir, (GONE, "Gone", 1), "", "gone", Cmdline, true),
            base_pin(cfg_dir, (OFF, "Off", 6), "Kept", "off", Cmdline, false),
            base_pin(
                cfg_dir,
                (TYPED, "Typed", 5),
                "Mine",
                "/usr/bin/bash -c my\\ --own",
                OverridesTable,
                true,
            ),
        ],
    };
    signed.validate().unwrap();
    let file = relaunch_core::ConfigFile {
        v: relaunch_core::FILE_FORMAT_VERSION,
        signed,
        signature: "0".repeat(128),
        pending: vec![],
    };
    serde_json::to_string(&file).unwrap()
}

impl World {
    /// [`base_doc_in`] for this world's config dir.
    pub fn base_doc(&self) -> String {
        base_doc_in(&self.0.join("cfg"))
    }
}

pub fn world(tag: &str) -> World {
    let w = World::new(tag);
    w.write("clients.json", CLIENTS.as_bytes());
    let script = format!("#!/bin/sh\ncat '{}'\n", w.0.join("clients.json").display());
    w.write("hyprctl", script.as_bytes());
    std::fs::set_permissions(w.0.join("hyprctl"), std::fs::Permissions::from_mode(0o755)).unwrap();
    w.write(
        "data/applications/myapp.desktop",
        b"[Desktop Entry]\nType=Application\nName=My App\nStartupWMClass=MyApp\nExec=myapp %U\n",
    );
    for (pid, argv) in [
        (10, &b"myapp\0"[..]),
        (11, b"proto-bin\0"),
        (12, b"ctor-bin\0"),
        (14, b"typed\0"),
        (15, b"off\0"),
        (18, b"foot\0-e\0herdr\0"),
    ] {
        w.write(&format!("cmd/{pid}"), argv);
    }
    w.write("cmd/17.ns", b"foreign\n");
    w
}

pub fn stdout_json(out: &Output) -> Value {
    let text = std::str::from_utf8(&out.stdout).unwrap();
    assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
    serde_json::from_str(text).unwrap()
}

pub fn keys(v: &Value) -> BTreeSet<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// The panel's frame: `TxnLifecycle.js` `proposeEnded`.
pub fn panel_frame(base: &str, proposal_json: &str) -> String {
    format!(
        r#"{{"base":{},"proposal":{proposal_json}}}"#,
        serde_json::to_string(base).unwrap()
    )
}
