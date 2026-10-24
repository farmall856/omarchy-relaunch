//! What `config.json` holds, as the panel and the CLI show it.
//!
//! A signed envelope shows its signed entries (bash read top-level `entries`
//! and so showed none). A legacy (pre-signing) file is shown as bash's
//! `normalize_entry` reads it, so the user sees their list until they adopt.
//! One signed under an older schema shows nothing: the next Save replaces it.
//! A malformed signed-looking file is an error, never an empty list.

use std::path::Path;

use relaunch_core::{ExecSource, MAX_DOCUMENT_BYTES};

use crate::propose::{DEFAULT_POST_LAUNCH_MS, DEFAULT_PRE_LAUNCH_MS};
use relaunch_sys::paths::CONFIG_FILE;
use serde::Serialize;
use serde_json::{Number, Value};

use crate::config::{classify, Kind};

/// One stored entry, as shown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoredEntry {
    /// The identity: a signed pin's id, or a legacy entry's window class.
    pub class: String,
    /// The window class the entry's window carries.
    #[serde(rename = "windowClass")]
    pub window_class: String,
    /// The workspace.
    pub workspace: Number,
    /// The stored command; may be empty in a legacy file.
    pub exec: String,
    /// The source's wire name, or empty when unknown.
    #[serde(rename = "execSource")]
    pub exec_source: String,
    /// Whether the entry launches at login.
    pub enabled: bool,
    /// The display label; may be empty.
    pub label: String,
    /// Floating (true) or tiled (false), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub float: Option<bool>,
}

/// The stored configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    /// Milliseconds before each launch within a same-app group.
    pub pre_launch_ms: Number,
    /// Milliseconds after each launch within a same-app group.
    pub post_launch_ms: Number,
    /// Whether the entries are a signed document's (each `exec` is then an
    /// argv, run directly), not a legacy file's shell commands.
    pub signed: bool,
    /// A legacy file's `skipOnce`.
    pub skip_once: bool,
    /// The entries, in file order.
    pub entries: Vec<StoredEntry>,
}

const KNOWN_SOURCES: [&str; 5] = [
    "terminal",
    "overrides-table",
    "desktop-file",
    "cmdline",
    "guess",
];

fn source_name(source: ExecSource) -> String {
    match serde_json::to_value(source) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// jq `tonumber` (absent is 0).
fn to_number(v: Option<&Value>) -> Result<Number, String> {
    match v {
        None | Some(Value::Null) => Ok(Number::from(0)),
        Some(Value::Number(n)) => Ok(n.clone()),
        Some(Value::String(s)) => serde_json::from_str::<Number>(s.trim())
            .map_err(|_| format!("cannot parse {s:?} as a number")),
        Some(other) => Err(format!("cannot parse {other} as a number")),
    }
}

/// A string field (`.x // ""`).
fn text(v: Option<&Value>, what: &str) -> Result<String, String> {
    match v {
        None | Some(Value::Null | Value::Bool(false)) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!("{what} is not a string: {other}")),
    }
}

fn legacy_entry(v: &Value) -> Result<StoredEntry, String> {
    let get = |k: &str| v.get(k);
    let source = text(get("execSource"), "execSource")?;
    let class = text(get("class"), "class")?;
    Ok(StoredEntry {
        window_class: class.clone(),
        class,
        workspace: to_number(get("workspace"))?,
        exec: text(get("exec"), "exec")?,
        exec_source: if KNOWN_SOURCES.contains(&source.as_str()) {
            source
        } else {
            String::new()
        },
        enabled: get("enabled") == Some(&Value::Bool(true)),
        label: match get("label") {
            None | Some(Value::Null | Value::Bool(false)) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        },
        float: get("float").and_then(Value::as_bool),
    })
}

fn legacy(v: &Value) -> Result<Stored, String> {
    let entries = match v.get("entries") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(legacy_entry).collect::<Result<_, _>>()?,
        Some(other) => return Err(format!("entries is not an array: {other}")),
    };
    // A legacy file's seconds are not carried over: they mean something else.
    to_number(v.get("staggerSeconds"))?;
    Ok(Stored {
        signed: false,
        skip_once: v.get("skipOnce") == Some(&Value::Bool(true)),
        ..Stored::empty()
    }
    .with_entries(entries))
}

impl Stored {
    /// No entries, the default delays.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            pre_launch_ms: Number::from(DEFAULT_PRE_LAUNCH_MS),
            post_launch_ms: Number::from(DEFAULT_POST_LAUNCH_MS),
            signed: true,
            skip_once: false,
            entries: Vec::new(),
        }
    }

    fn with_entries(self, entries: Vec<StoredEntry>) -> Self {
        Self { entries, ..self }
    }
}

/// Read `config.json` from `config_dir`; no file is an empty configuration.
///
/// # Errors
///
/// `parse config: <path>: <why>` for an unreadable or malformed file.
pub fn read(config_dir: &Path) -> Result<Stored, String> {
    let path = config_dir.join(CONFIG_FILE);
    let fail = |why: String| format!("parse config: {}: {why}", path.display());
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::empty()),
        _ => {}
    }
    let bytes = relaunch_sys::fs::read_regular_capped(&path, MAX_DOCUMENT_BYTES).map_err(fail)?;
    match classify(&bytes) {
        Kind::Signed(cfg) => Ok(Stored {
            pre_launch_ms: Number::from(cfg.signed.pre_launch_ms),
            post_launch_ms: Number::from(cfg.signed.post_launch_ms),
            signed: true,
            skip_once: false,
            entries: cfg
                .signed
                .entries
                .into_iter()
                .map(|e| StoredEntry {
                    class: e.class,
                    window_class: e.window_class,
                    workspace: Number::from(e.workspace),
                    exec: e.exec,
                    exec_source: source_name(e.exec_source),
                    enabled: e.enabled,
                    label: e.label,
                    float: e.float,
                })
                .collect(),
        }),
        Kind::Legacy => {
            let v: Value = serde_json::from_slice(&bytes).map_err(|e| fail(e.to_string()))?;
            legacy(&v).map_err(fail)
        }
        Kind::Superseded(_) => Ok(Stored::empty()),
        Kind::Invalid(e) => Err(fail(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_entry_is_normalized_like_bash() {
        let v: Value = serde_json::from_str(
            r#"{"staggerSeconds":"3","skipOnce":true,"entries":[
                {"class":"A","workspace":"2","exec":"a","execSource":"bogus","enabled":1,"label":7,"float":"no"},
                {"class":"B","workspace":4,"enabled":true,"float":false}]}"#,
        )
        .unwrap();
        let s = legacy(&v).unwrap();
        assert_eq!(
            (s.post_launch_ms.as_u64(), s.skip_once, s.signed),
            (Some(u64::from(DEFAULT_POST_LAUNCH_MS)), true, false),
            "a legacy stagger in seconds is not carried into the millisecond delays"
        );
        assert_eq!(s.entries[1].window_class, "B");
        let a = &s.entries[0];
        assert_eq!(
            (
                a.workspace.as_u64(),
                a.exec_source.as_str(),
                a.enabled,
                a.label.as_str(),
                a.float
            ),
            (Some(2), "", false, "7", None)
        );
        let b = &s.entries[1];
        assert_eq!(
            (b.exec.as_str(), b.enabled, b.float),
            ("", true, Some(false))
        );
    }

    #[test]
    fn a_malformed_legacy_field_is_refused() {
        let v: Value =
            serde_json::from_str(r#"{"entries":[{"class":"A","workspace":"x"}]}"#).unwrap();
        assert!(legacy(&v).is_err());
        let v: Value = serde_json::from_str(r#"{"entries":{}}"#).unwrap();
        assert!(legacy(&v).is_err());
    }
}
