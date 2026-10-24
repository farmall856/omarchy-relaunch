//! `list`: the panel's view of Relaunch — the stored entries, the running
//! windows not yet in Relaunch, the startup inventory, and the boot state.
//! Read only: listing never writes a file (D-P4; the hooks are `ensure-hooks`).

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::Path;

use relaunch_sys::paths::{
    CONFIG_FILE, DISABLED_FLAG, LAST_BOOT_JSON, LAST_BOOT_LOG, SKIP_ONCE_FLAG,
};
use serde_json::{json, Map, Number, Value};

use crate::compositor::{self, Window};
use crate::env::Env;
use crate::execcheck::{exec_is_ok_in, Mode};
use crate::jq::at;
use crate::resolve::override_exec;
use crate::shell::Exec;
use crate::startup::{self, StartupItem};
use crate::stored::{self, Stored, StoredEntry};

/// The generated rules file (shown as `snippetPath`).
const RULES_FILE: &str = "rules.lua";
const OVERRIDES_FILE: &str = "overrides.json";

/// `{disabled, skipOnce, active}` from the flag files (and a legacy file's
/// `skipOnce`).
#[must_use]
pub fn boot_state(config_dir: &Path, stored: &Stored) -> Value {
    let disabled = config_dir.join(DISABLED_FLAG).exists();
    let skip = config_dir.join(SKIP_ONCE_FLAG).exists() || stored.skip_once;
    json!({ "disabled": disabled, "skipOnce": skip, "active": !(disabled || skip) })
}

/// `last-boot.json` plus the log text (`text`, trailing newlines removed) and
/// `logPath`; `null` without a readable record.
#[must_use]
pub fn last_boot_state(config_dir: &Path) -> Value {
    let read = |name: &str| {
        relaunch_sys::fs::read_regular_capped(
            &config_dir.join(name),
            relaunch_sys::fs::MAX_FILE_BYTES,
        )
    };
    let Ok(Value::Object(mut record)) = read(LAST_BOOT_JSON)
        .map_err(|_| ())
        .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| ()))
    else {
        return Value::Null;
    };
    let text = read(LAST_BOOT_LOG)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .trim_end_matches('\n')
                .to_owned()
        })
        .unwrap_or_default();
    record.insert("text".into(), json!(text));
    record.insert(
        "logPath".into(),
        json!(config_dir.join(LAST_BOOT_LOG).to_string_lossy()),
    );
    Value::Object(record)
}

/// The overrides table must be a JSON object when present (it is never
/// created here).
fn require_overrides_parseable(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join(OVERRIDES_FILE);
    if !path.exists() {
        return Ok(());
    }
    relaunch_sys::fs::read_regular_capped(&path, relaunch_sys::fs::MAX_FILE_BYTES)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object)
        .map(|_| ())
        .ok_or_else(|| format!("parse overrides: {}", path.display()))
}

/// jq's order for the numbers a workspace holds.
fn num_cmp(a: &Number, b: &Number) -> Ordering {
    a.as_f64()
        .unwrap_or(0.0)
        .total_cmp(&b.as_f64().unwrap_or(0.0))
}

/// The stored command, or the class guess for an entry with none.
fn exec_or_guess(class: &str, exec: &str) -> String {
    if exec.is_empty() {
        Exec::guess(class).into_string()
    } else {
        exec.to_owned()
    }
}

/// A running window, as the panel lists it.
struct Running {
    class: String,
    workspace: i64,
    exec: Option<String>,
}

fn running(windows: &[Window]) -> Vec<Running> {
    windows
        .iter()
        .map(|w| Running {
            class: w.class.clone(),
            workspace: w.workspace,
            exec: None,
        })
        .collect()
}

fn rows(stored: &Stored, startup: &[StartupItem], live: Vec<Running>) -> Vec<Map<String, Value>> {
    let obj = |v: Value| match v {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let in_relaunch: Vec<String> = stored
        .entries
        .iter()
        .map(|e| e.window_class.to_lowercase())
        .collect();

    let mut live: Vec<Running> = live
        .into_iter()
        .filter(|r| !r.class.is_empty() && r.workspace >= 1)
        .collect();
    live.sort_by(|a, b| {
        a.workspace
            .cmp(&b.workspace)
            .then_with(|| a.class.cmp(&b.class))
    });
    let mut by_class: Vec<Running> = Vec::new();
    for r in live {
        if !by_class.iter().any(|k| k.class == r.class) {
            by_class.push(r);
        }
    }
    by_class.sort_by(|a, b| a.class.cmp(&b.class));
    let from_running = by_class
        .into_iter()
        .filter(|r| !in_relaunch.contains(&r.class.to_ascii_lowercase()))
        .map(|r| {
            let (exec, source) = match r.exec.filter(|e| !e.is_empty()) {
                Some(e) => (e, "terminal"),
                None => (Exec::guess(&r.class).into_string(), "guess"),
            };
            obj(json!({
                "id": format!("running:{}", r.class), "kind": "running", "class": r.class,
                "label": r.class, "exec": exec, "execSource": source, "workspace": r.workspace,
                "enabled": false, "inRelaunch": false,
            }))
        });

    let mut entries: Vec<&StoredEntry> = stored.entries.iter().collect();
    entries.sort_by(|a, b| {
        num_cmp(&a.workspace, &b.workspace)
            .then_with(|| a.window_class.cmp(&b.window_class))
            .then_with(|| a.class.cmp(&b.class))
    });
    let from_relaunch = entries.into_iter().map(|e| {
        obj(json!({
            "id": format!("relaunch:{}", e.class), "kind": "relaunch", "class": e.class,
            "windowClass": e.window_class,
            "label": if e.label.is_empty() { &e.window_class } else { &e.label },
            "exec": exec_or_guess(&e.window_class, &e.exec), "execSource": e.exec_source,
            "execText": if stored.signed { crate::edit::exec_text(&e.exec) } else { e.exec.clone() },
            "workspace": e.workspace, "enabled": e.enabled, "inRelaunch": true,
        }))
    });

    let from_startup = startup.iter().map(|s| {
        obj(json!({
            "id": s.id, "kind": "startup", "class": s.class,
            "label": if s.class.is_empty() { &s.exec } else { &s.class },
            "exec": s.exec, "workspace": 0, "enabled": false, "inRelaunch": false,
        }))
    });

    from_running
        .chain(from_relaunch)
        .chain(from_startup)
        .collect()
}

/// How boot runs an entry's command: a signed entry's, or a legacy desktop
/// launcher's, as argv; anything else through the shell.
fn run_mode(signed: bool, source: &str) -> Mode {
    if signed || source == "desktop-file" {
        Mode::Direct
    } else {
        Mode::Shell
    }
}

/// Add `execSource` (filled in when unknown), `execOk` and `unverified`.
/// `signed` says the item's `exec` is a signed entry's argv.
fn decorate(env: &Env, mut item: Map<String, Value>, signed: bool) -> Value {
    let field = |k: &str| {
        item.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (class, exec, mut source) = (field("class"), field("exec"), field("execSource"));
    let (ok, unverified) = if class.is_empty() {
        (true, false)
    } else {
        if source.is_empty() {
            if override_exec(&env.config_dir, &class).is_some() {
                "overrides-table".clone_into(&mut source);
            } else if exec == class.to_lowercase() || exec == Exec::guess(&class).as_str() {
                "guess".clone_into(&mut source);
            }
        }
        (
            exec_is_ok_in(&exec, &env.path, run_mode(signed, &source)),
            source == "guess" || source == "cmdline",
        )
    };
    item.insert("execSource".into(), json!(source));
    item.insert("execOk".into(), json!(ok));
    item.insert("unverified".into(), json!(unverified));
    Value::Object(item)
}

/// The full panel state (`list --json`).
///
/// # Errors
///
/// A message if the configuration or the overrides table is malformed, or the
/// window list cannot be read.
pub fn state(env: &Env) -> Result<Value, String> {
    require_overrides_parseable(&env.config_dir)?;
    let stored = stored::read(&env.config_dir)?;
    let startup = env
        .autostart
        .as_deref()
        .map(startup::scan)
        .unwrap_or_default();
    let windows = compositor::clients(env)?;
    let entries: Vec<Value> = stored
        .entries
        .iter()
        .map(|e| match serde_json::to_value(e) {
            Ok(Value::Object(m)) => decorate(env, m, stored.signed),
            _ => Value::Null,
        })
        .collect();
    let rows: Vec<Value> = rows(&stored, &startup, running(&windows))
        .into_iter()
        .map(|r| {
            let signed = stored.signed && r.get("kind") == Some(&json!("relaunch"));
            decorate(env, r, signed)
        })
        .collect();
    let warnings: Vec<Value> = entries
        .iter()
        .filter(|e| at(e, "execOk") == false)
        .map(|e| {
            let exec = at(e, "exec").as_str().unwrap_or_default();
            let first = exec
                .split(' ')
                .next()
                .filter(|_| !exec.is_empty())
                .unwrap_or(exec);
            json!({
                "class": at(e, "class"), "exec": at(e, "exec"), "execSource": at(e, "execSource"),
                "execOk": at(e, "execOk"), "message": format!("command not found: {first}"),
            })
        })
        .collect();
    Ok(json!({
        "ok": true,
        "preLaunchMs": stored.pre_launch_ms,
        "postLaunchMs": stored.post_launch_ms,
        "entries": entries,
        "startup": startup,
        "rows": rows,
        "boot": boot_state(&env.config_dir, &stored),
        "lastBoot": last_boot_state(&env.config_dir),
        "snippetPath": env.config_dir.join(RULES_FILE).to_string_lossy(),
        "configPath": env.config_dir.join(CONFIG_FILE).to_string_lossy(),
        "warnings": warnings,
    }))
}

/// `%-Ns`, padded by characters.
fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    format!("{s}{}", " ".repeat(width.saturating_sub(n)))
}

/// The text listing (`list` without `--json`).
///
/// # Errors
///
/// As [`state`], for the configuration and overrides.
pub fn text(env: &Env) -> Result<String, String> {
    require_overrides_parseable(&env.config_dir)?;
    let stored = stored::read(&env.config_dir)?;
    let boot = boot_state(&env.config_dir, &stored);
    let mut out = String::new();
    let mode = if at(&boot, "disabled") == true {
        "disabled"
    } else if at(&boot, "skipOnce") == true {
        "skip-next"
    } else {
        "on"
    };
    let _ = writeln!(out, "boot: {mode}");
    if stored.entries.is_empty() {
        out.push_str("No relaunch entries. Capture them from the panel.\n");
    } else {
        let _ = writeln!(
            out,
            "{} {} {} {} exec",
            pad("ws", 4),
            pad("on", 3),
            pad("name", 22),
            pad("class", 22)
        );
        for e in &stored.entries {
            let label = if e.label.is_empty() {
                &e.window_class
            } else {
                &e.label
            };
            let exec = if e.exec.is_empty() {
                format!("{} *", Exec::guess(&e.window_class).as_str())
            } else {
                e.exec.clone()
            };
            let on = if e.enabled { "x" } else { " " };
            let _ = writeln!(
                out,
                "{} [{on}] {} {} {exec}",
                pad(&e.workspace.to_string(), 4),
                pad(label, 22),
                pad(&e.window_class, 22)
            );
        }
    }
    let startup = env
        .autostart
        .as_deref()
        .map(startup::scan)
        .unwrap_or_default();
    if !startup.is_empty() {
        out.push_str("\nstartup apps (autostart.lua):\n");
        for s in &startup {
            let _ = writeln!(out, "  [{}] {}  class={}", s.kind, s.exec, s.class);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launcher_is_judged_as_argv_and_a_shell_command_as_shell() {
        let dir = std::env::temp_dir().join(format!("relaunch-list-mode-{}", std::process::id()));
        let env = Env::from_vars(
            |name| match name {
                "RELAUNCH_CONFIG_DIR" => Some(dir.clone().into_os_string()),
                "PATH" => Some("/usr/bin:/bin".into()),
                _ => None,
            },
            &relaunch_sys::paths::base_dirs(),
        );
        let ok = |source: &str, signed: bool| {
            let mut m = Map::new();
            m.insert("class".into(), json!("c"));
            m.insert("exec".into(), json!("cd /"));
            m.insert("execSource".into(), json!(source));
            at(&decorate(&env, m, signed), "execOk").clone()
        };
        assert_eq!(
            ok("desktop-file", false),
            json!(false),
            "execve cannot run a builtin"
        );
        assert_eq!(
            ok("overrides-table", false),
            json!(true),
            "a legacy shell command"
        );
        assert_eq!(
            ok("overrides-table", true),
            json!(false),
            "every signed entry is run as argv"
        );
    }

    fn win(class: &str, workspace: i64) -> Running {
        Running {
            class: class.to_owned(),
            workspace,
            exec: None,
        }
    }

    #[test]
    fn running_rows_dedupe_to_the_lowest_workspace_and_skip_stored_classes() {
        let stored = Stored {
            entries: vec![StoredEntry {
                class: "relauncher-00000000000000a1".into(),
                window_class: "Kept".into(),
                workspace: Number::from(1),
                exec: String::new(),
                exec_source: String::new(),
                enabled: true,
                label: String::new(),
                float: None,
            }],
            ..Stored::empty()
        };
        let live = vec![
            win("Twice", 5),
            win("kept", 3),
            win("Twice", 2),
            win("Special", -98),
            win("", 1),
        ];
        let r = rows(&stored, &[], live);
        let running: Vec<(&str, i64)> = r
            .iter()
            .filter(|m| m.get("kind") == Some(&json!("running")))
            .map(|m| {
                (
                    m.get("class").and_then(Value::as_str).unwrap_or_default(),
                    m.get("workspace")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(running, [("Twice", 2)]);
        let relaunch = r
            .iter()
            .find(|m| m.get("kind") == Some(&json!("relaunch")))
            .unwrap();
        assert_eq!(
            relaunch.get("exec"),
            Some(&json!("kept")),
            "an empty exec shows the guess"
        );
    }

    #[test]
    fn columns_are_padded_by_characters() {
        assert_eq!(pad("Café", 6), "Café  ");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("toolongvalue", 4), "toolongvalue");
    }
}
