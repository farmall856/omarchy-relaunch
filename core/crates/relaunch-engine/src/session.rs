//! The session diagnostics: `snapshot` (record the live layout),
//! `last-session` (show it, or diff it against the last boot) and `last-boot`
//! (the anchor's boot record). Opt-in and purely diagnostic: nothing that
//! launches reads them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use relaunch_sys::paths::{LAST_BOOT_JSON, LAST_BOOT_LOG};
use serde::Serialize;
use serde_json::{json, Value};

use crate::compositor::{self, Window};
use crate::env::Env;
use crate::jq::{self, at, interp, or_default};
use crate::list::last_boot_state;
use crate::stored;

/// The snapshot file, in the config directory.
pub const LAST_SESSION_JSON: &str = "last-session.json";

/// One window in a snapshot (field order is the file's).
#[derive(Debug, Serialize)]
struct SnapWindow {
    class: String,
    label: String,
    workspace: i64,
    floating: bool,
    monitor: i64,
    #[serde(rename = "monitorName")]
    monitor_name: String,
    #[serde(rename = "monitorDescription")]
    monitor_description: String,
    pid: i64,
    title: String,
}

#[derive(Debug, Serialize)]
struct Snapshot {
    #[serde(rename = "capturedAt")]
    captured_at: String,
    windows: Vec<SnapWindow>,
}

/// The local time as `date -Iseconds` prints it.
fn now_iso() -> Result<String, String> {
    let out = relaunch_sys::cmd::output_capped(relaunch_sys::paths::DATE, &["-Iseconds"])?;
    let text = String::from_utf8_lossy(&out).trim().to_owned();
    if text.is_empty() {
        return Err("date: no timestamp".into());
    }
    Ok(text)
}

/// `hyprctl monitors -j` by id; empty when it fails (a snapshot does not need
/// monitor names).
fn monitors(env: &Env) -> BTreeMap<i64, (String, String)> {
    #[derive(serde::Deserialize)]
    struct Monitor {
        #[serde(default)]
        id: Option<i64>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    }
    relaunch_sys::cmd::output_capped(&env.hyprctl, &["monitors", "-j"])
        .ok()
        .and_then(|o| serde_json::from_slice::<Vec<Monitor>>(&o).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            (
                m.id.unwrap_or(-1),
                (
                    m.name.unwrap_or_default(),
                    m.description.unwrap_or_default(),
                ),
            )
        })
        .collect()
}

fn snap_window(
    w: &Window,
    mon: &BTreeMap<i64, (String, String)>,
    labels: &BTreeMap<String, String>,
) -> SnapWindow {
    let monitor = w.monitor.unwrap_or(-1);
    let (name, description) = mon.get(&monitor).cloned().unwrap_or_default();
    SnapWindow {
        label: labels
            .get(&w.class)
            .cloned()
            .unwrap_or_else(|| w.class.clone()),
        class: w.class.clone(),
        workspace: w.workspace,
        floating: w.floating == Some(true),
        monitor,
        monitor_name: name,
        monitor_description: description,
        pid: w.pid.unwrap_or(0),
        title: w.title.clone(),
    }
}

/// Record the live layout to `last-session.json`. Returns the stdout text.
///
/// # Errors
///
/// A message if the windows, the configuration or the clock cannot be read, or
/// the file cannot be written.
pub fn snapshot(env: &Env, as_json: bool) -> Result<String, String> {
    let windows = compositor::clients(env)?;
    let mon = monitors(env);
    let labels: BTreeMap<String, String> = stored::read(&env.config_dir)?
        .entries
        .into_iter()
        .filter(|e| !e.label.is_empty())
        .map(|e| (e.class, e.label))
        .collect();
    let mut snap = Snapshot {
        captured_at: now_iso()?,
        windows: windows
            .iter()
            .map(|w| snap_window(w, &mon, &labels))
            .collect(),
    };
    snap.windows.sort_by(|a, b| {
        a.workspace
            .cmp(&b.workspace)
            .then_with(|| a.class.cmp(&b.class))
    });
    let pretty = serde_json::to_string_pretty(&snap).map_err(|e| format!("snapshot: {e}"))?;
    let path = env.config_dir.join(LAST_SESSION_JSON);
    relaunch_sys::fs::write_private_artifact(
        &env.config_dir,
        LAST_SESSION_JSON,
        format!("{pretty}\n").as_bytes(),
    )
    .map_err(|e| format!("snapshot: {}: {e}", path.display()))?;
    if as_json {
        let body = json!({ "ok": true, "capturedAt": snap.captured_at, "windows": snap.windows });
        let text = serde_json::to_string_pretty(&body).map_err(|e| format!("snapshot: {e}"))?;
        Ok(format!("{text}\n"))
    } else {
        Ok(format!(
            "Wrote {} ({} windows)\n",
            path.display(),
            snap.windows.len()
        ))
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = relaunch_sys::fs::read_regular_capped(path, relaunch_sys::fs::MAX_FILE_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The snapshot-vs-now report (`last-session --diff`).
fn diff_report(before: &Value, after: &Value, now: &[Window]) -> Value {
    let live: Vec<(String, Value, Value)> = now
        .iter()
        .filter(|w| !w.class.is_empty())
        .map(|w| {
            (
                w.class.clone(),
                json!(w.workspace),
                json!(w.floating == Some(true)),
            )
        })
        .collect();
    let mut windows: Vec<Value> = before
        .get("windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    windows.sort_by(|a, b| jq::cmp(at(a, "class"), at(b, "class")));
    let mut groups: Vec<Vec<Value>> = Vec::new();
    for w in windows {
        match groups.last_mut() {
            Some(g)
                if g.first()
                    .is_some_and(|f| jq::cmp(at(f, "class"), at(&w, "class")).is_eq()) =>
            {
                g.push(w);
            }
            _ => groups.push(vec![w]),
        }
    }
    let classes: Vec<Value> = groups
        .iter()
        .map(|group| {
            let first = group.first().cloned().unwrap_or(Value::Null);
            let class = at(&first, "class").clone();
            let back: Vec<&(String, Value, Value)> = live
                .iter()
                .filter(|(c, _, _)| class.as_str() == Some(c.as_str()))
                .collect();
            let placements: Vec<Value> = group
                .iter()
                .map(|w| {
                    let (ws, fl) = (&at(w, "workspace"), &at(w, "floating"));
                    json!({
                        "wasWorkspace": ws, "wasFloating": fl,
                        "matched": back.iter().any(|(_, bw, bf)| jq::eq(bw, ws) && jq::eq(bf, fl)),
                        "workspaceMatched": back.iter().any(|(_, bw, _)| jq::eq(bw, ws)),
                    })
                })
                .collect();
            let (want, got) = (group.len(), back.len());
            let mismatched = placements.iter().filter(|p| at(p, "matched") != true).count();
            json!({
                "class": class, "label": at(&first, "label"), "want": want, "live": got,
                "missing": i64::try_from(want).unwrap_or(i64::MAX) - i64::try_from(got).unwrap_or(i64::MAX),
                "placements": placements, "mismatched": mismatched,
            })
        })
        .collect();
    json!({
        "capturedAt": at(before, "capturedAt"),
        "bootedAt": or_default(after.get("startedAt"), json!("")),
        "outcome": or_default(after.get("outcome"), json!("")),
        "classes": classes,
    })
}

fn diff_text(report: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "session before: {}", interp(at(report, "capturedAt")));
    let _ = writeln!(
        out,
        "last boot:      {}  outcome: {}",
        interp(at(report, "bootedAt")),
        interp(at(report, "outcome"))
    );
    out.push('\n');
    for c in at(report, "classes")
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let (label, class) = (interp(at(c, "label")), interp(at(c, "class")));
        if at(c, "missing").as_i64().unwrap_or(0) > 0 {
            let _ = writeln!(
                out,
                "  MISSING  {label}  class={class}  had {}, back {}",
                at(c, "want"),
                at(c, "live")
            );
        } else if at(c, "mismatched").as_u64().unwrap_or(0) > 0 {
            let detail: Vec<String> = at(c, "placements")
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|p| at(p, "matched") != true)
                .map(|p| {
                    if at(p, "workspaceMatched") == true {
                        format!("float {}", interp(at(p, "wasFloating")))
                    } else {
                        format!("ws {}", interp(at(p, "wasWorkspace")))
                    }
                })
                .collect();
            let _ = writeln!(
                out,
                "  MOVED    {label}  class={class}  expected {}",
                detail.join(", ")
            );
        } else {
            let _ = writeln!(
                out,
                "  ok       {label}  class={class}  {}/{}",
                at(c, "live"),
                at(c, "want")
            );
        }
    }
    out
}

/// `last-session`: the snapshot as recorded, or (`diff`) against the last boot.
///
/// # Errors
///
/// A message if there is no snapshot (or, for a diff, no boot record or window
/// list).
pub fn last_session(env: &Env, as_json: bool, diff: bool) -> Result<String, String> {
    let path = env.config_dir.join(LAST_SESSION_JSON);
    if !path.is_file() {
        return Err("no session snapshot yet (take one with: relaunch snapshot)".into());
    }
    if !diff {
        if as_json {
            let bytes =
                relaunch_sys::fs::read_regular_capped(&path, relaunch_sys::fs::MAX_FILE_BYTES)?;
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        let snap = read_json(&path)?;
        let mut out = format!("session snapshot {}\n\n", interp(at(&snap, "capturedAt")));
        for w in at(&snap, "windows")
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let _ = writeln!(
                out,
                "  ws {}  {}  class={}",
                interp(at(w, "workspace")),
                interp(at(w, "label")),
                interp(at(w, "class"))
            );
        }
        return Ok(out);
    }
    let boot = env.config_dir.join(LAST_BOOT_JSON);
    if !boot.is_file() {
        return Err("no last-boot log to diff against".into());
    }
    let report = diff_report(
        &read_json(&path)?,
        &read_json(&boot)?,
        &compositor::clients(env)?,
    );
    if as_json {
        let text =
            serde_json::to_string_pretty(&report).map_err(|e| format!("last-session: {e}"))?;
        Ok(format!("{text}\n"))
    } else {
        Ok(diff_text(&report))
    }
}

/// What `last-boot` does.
pub enum LastBoot {
    /// Print the record (`--json`) or the log.
    Show,
    /// Open the log in Omarchy's editor.
    Open,
}

fn on_path(path_var: &str, name: &str) -> Option<PathBuf> {
    path_var
        .split(':')
        .map(|d| Path::new(if d.is_empty() { "." } else { d }).join(name))
        .find(|p| relaunch_sys::fs::is_executable_file(p))
}

/// `last-boot`. Returns the stdout text; `Open` also needs the panel state,
/// which the caller prints for `--json`.
///
/// # Errors
///
/// A message if there is no boot log, or (`Open`) the editor is missing or
/// cannot start.
pub fn last_boot(env: &Env, as_json: bool, what: &LastBoot) -> Result<Option<String>, String> {
    let log = env.config_dir.join(LAST_BOOT_LOG);
    if !log.is_file() {
        return Err("no last-boot log yet (reboot or run: relaunch boot)".into());
    }
    match what {
        LastBoot::Open => {
            let editor = on_path(&env.path, "omarchy-launch-editor")
                .ok_or("omarchy-launch-editor not found")?;
            Command::new(editor)
                .arg(&log)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("omarchy-launch-editor: {e}"))?;
            Ok((!as_json).then(|| format!("Opened {}\n", log.display())))
        }
        LastBoot::Show if as_json => Ok(Some(format!("{}\n", last_boot_state(&env.config_dir)))),
        LastBoot::Show => {
            let bytes =
                relaunch_sys::fs::read_regular_capped(&log, relaunch_sys::fs::MAX_FILE_BYTES)?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }
    }
}
