//! What a boot did, and where each app's window ended up (spec §8). Written
//! to `last-boot.json` and `last-boot.log` for the panel, `relaunch last-boot`
//! and `relaunch last-session --diff`.
//!
//! Per launch: `status` says whether it started (`started`, `not_found`,
//! `failed`, `verify-failed`), and `outcome` where its window went —
//! `placed` (on the pinned workspace), `misplaced` (on another; the actual
//! workspace is named) or `no-show` (no window of its class appeared in time).
//! The boot's own `outcome` is `landed` only when every launch was placed.

use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::{json, Value};

/// One launch attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The entry's pin id.
    pub class: String,
    /// The window class its window is recognised by.
    pub window_class: String,
    /// The entry's display name.
    pub label: String,
    /// The pinned workspace.
    pub workspace: u32,
    /// Floating (`true`) or tiled (`false`) as captured, when known.
    pub float: Option<bool>,
    /// The command, as signed.
    pub exec: String,
    /// `started`, `not_found`, `failed` or `verify-failed`.
    pub status: &'static str,
    /// The spawned process, when it started.
    pub pid: Option<u32>,
    /// Why it did not start.
    pub error: Option<String>,
    /// Where its window went; `None` for a launch that did not start.
    pub placement: Option<Placement>,
}

/// Where a launch's window ended up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The window, once one was recognised.
    pub address: Option<String>,
    /// Its workspace when the boot finished.
    pub actual_workspace: Option<i64>,
    /// `placed`, `misplaced` or `no-show`.
    pub outcome: &'static str,
}

impl Launch {
    /// Whether the launch's window is on its pinned workspace.
    #[must_use]
    pub fn placed(&self) -> bool {
        self.placement
            .as_ref()
            .is_some_and(|p| p.outcome == "placed")
    }

    fn to_json(&self) -> Value {
        let mut v = json!({
            "class": self.class,
            "windowClass": self.window_class,
            "label": self.label,
            "workspace": self.workspace,
            "exec": self.exec,
            "status": self.status,
            "pid": self.pid,
            "error": self.error,
            "placed": self.placed(),
        });
        if let (Some(p), Some(map)) = (&self.placement, v.as_object_mut()) {
            map.insert("address".into(), json!(p.address));
            map.insert("actualWorkspace".into(), json!(p.actual_workspace));
            map.insert("outcome".into(), json!(p.outcome));
        }
        v
    }
}

/// A window, as placement needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The compositor's address for it (`0x…`).
    pub address: String,
    /// Its current class.
    pub class: String,
    /// The class it mapped with.
    pub initial_class: String,
    /// Its workspace.
    pub workspace: Option<i64>,
    /// Whether it floats, when reported.
    pub floating: Option<bool>,
}

impl Window {
    /// Whether this is a window of `window_class`: by the class it mapped
    /// with, or its current one (an app may rename itself after mapping).
    #[must_use]
    pub fn is_of(&self, window_class: &str) -> bool {
        self.initial_class == window_class || self.class == window_class
    }
}

/// Decode `hyprctl clients -j`; `None` unless it is a JSON array of clients.
#[must_use]
pub fn parse_clients(bytes: &[u8]) -> Option<Vec<Window>> {
    #[derive(Deserialize)]
    struct Ws {
        #[serde(default)]
        id: Option<i64>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        #[serde(default)]
        address: Option<String>,
        #[serde(default)]
        class: Option<String>,
        #[serde(default)]
        initial_class: Option<String>,
        #[serde(default)]
        workspace: Option<Ws>,
        #[serde(default)]
        floating: Option<bool>,
    }
    let raw: Vec<Raw> = serde_json::from_slice(bytes).ok()?;
    Some(
        raw.into_iter()
            .map(|r| Window {
                address: r.address.unwrap_or_default(),
                class: r.class.unwrap_or_default(),
                initial_class: r.initial_class.unwrap_or_default(),
                workspace: r.workspace.and_then(|w| w.id),
                floating: r.floating,
            })
            .collect(),
    )
}

/// A finished boot.
#[derive(Clone, Debug)]
pub struct Record {
    /// When it started (ISO 8601).
    pub started_at: String,
    /// `landed`, `launched` (not every window placed), `disabled`, `skipped`,
    /// `unverified` or `schema-upgrade-needed`.
    pub outcome: &'static str,
    /// Why, for anything but `landed`/`launched`.
    pub reason: String,
    /// The signed pre-launch delay, as honoured.
    pub pre_launch_ms: u32,
    /// The signed post-launch delay, as honoured.
    pub post_launch_ms: u32,
    /// Each launch.
    pub launches: Vec<Launch>,
}

impl Record {
    /// `last-boot.json`'s body.
    #[must_use]
    pub fn json(&self) -> String {
        let doc = json!({
            "startedAt": self.started_at,
            "outcome": self.outcome,
            "reason": self.reason,
            "preLaunchMs": self.pre_launch_ms,
            "postLaunchMs": self.post_launch_ms,
            "launches": self.launches.iter().map(Launch::to_json).collect::<Vec<_>>(),
        });
        format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_owned())
        )
    }

    /// `last-boot.log`: the record for a person.
    #[must_use]
    pub fn log(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "relaunch boot  {}", self.started_at);
        let _ = writeln!(out, "outcome: {}", self.outcome);
        if !self.reason.is_empty() {
            let _ = writeln!(out, "reason:  {}", self.reason);
        }
        let _ = writeln!(
            out,
            "delays:  {} ms before, {} ms after",
            self.pre_launch_ms, self.post_launch_ms
        );
        if !self.launches.is_empty() {
            out.push_str("\nlaunches:\n");
            for l in &self.launches {
                let pid = l.pid.map_or_else(|| "-".to_owned(), |p| p.to_string());
                let name = if l.label.is_empty() {
                    &l.window_class
                } else {
                    &l.label
                };
                let _ = write!(out, "  ws {}  {name}  {}  pid={pid}", l.workspace, l.status);
                if let Some(p) = &l.placement {
                    match (p.outcome, p.actual_workspace) {
                        ("misplaced", Some(ws)) => {
                            let _ = write!(out, "  MISPLACED on ws {ws}");
                        }
                        ("misplaced", None) => out.push_str("  MISPLACED"),
                        ("no-show", _) => out.push_str("  NO-SHOW (no window)"),
                        _ => out.push_str("  placed"),
                    }
                }
                let _ = write!(out, "  {}", l.exec);
                if let Some(e) = &l.error {
                    let _ = write!(out, "  ({e})");
                }
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(name: &str, workspace: u32, outcome: Option<(&'static str, Option<i64>)>) -> Launch {
        Launch {
            class: format!("relauncher-{name}"),
            window_class: name.into(),
            label: String::new(),
            workspace,
            float: None,
            exec: name.to_lowercase(),
            status: "started",
            pid: Some(10),
            error: None,
            placement: outcome.map(|(outcome, actual_workspace)| Placement {
                address: Some("0xa".into()),
                actual_workspace,
                outcome,
            }),
        }
    }

    #[test]
    fn clients_are_read_with_address_and_both_classes() {
        let w = parse_clients(
            br#"[{"address":"0x5a","class":"brave-browser","initialClass":"Brave",
                  "workspace":{"id":3},"floating":true},{"workspace":{}}]"#,
        )
        .unwrap();
        assert_eq!(
            w[0],
            Window {
                address: "0x5a".into(),
                class: "brave-browser".into(),
                initial_class: "Brave".into(),
                workspace: Some(3),
                floating: Some(true),
            }
        );
        assert!(w[0].is_of("Brave") && w[0].is_of("brave-browser") && !w[0].is_of("foot"));
        assert_eq!(w[1].workspace, None);
        assert!(parse_clients(b"{}").is_none());
    }

    #[test]
    fn the_log_and_record_carry_each_verdict() {
        let mut gone = launch("Gone", 4, None);
        gone.status = "verify-failed";
        gone.pid = None;
        gone.error = Some("launcher-changed".into());
        let mut named = launch("Here", 1, Some(("placed", Some(1))));
        named.label = "Here App".into();
        let rec = Record {
            started_at: "2026-10-22T08:00:00+01:00".into(),
            outcome: "launched",
            reason: String::new(),
            pre_launch_ms: 0,
            post_launch_ms: 500,
            launches: vec![
                named,
                launch("Moved", 2, Some(("misplaced", Some(5)))),
                launch("Late", 3, Some(("no-show", None))),
                gone,
            ],
        };
        assert_eq!(
            rec.log(),
            "relaunch boot  2026-10-22T08:00:00+01:00\noutcome: launched\n\
             delays:  0 ms before, 500 ms after\n\nlaunches:\n\
             \x20 ws 1  Here App  started  pid=10  placed  here\n\
             \x20 ws 2  Moved  started  pid=10  MISPLACED on ws 5  moved\n\
             \x20 ws 3  Late  started  pid=10  NO-SHOW (no window)  late\n\
             \x20 ws 4  Gone  verify-failed  pid=-  gone  (launcher-changed)\n"
        );
        let v: Value = serde_json::from_str(&rec.json()).unwrap();
        assert_eq!(
            (v["preLaunchMs"].clone(), v["postLaunchMs"].clone()),
            (json!(0), json!(500))
        );
        assert_eq!(v["launches"][0]["placed"], true);
        assert_eq!(v["launches"][0]["label"], "Here App");
        assert_eq!(v["launches"][1]["outcome"], "misplaced");
        assert_eq!(v["launches"][1]["actualWorkspace"], 5);
        assert_eq!(v["launches"][2]["outcome"], "no-show");
        assert_eq!(v["launches"][3]["placed"], false);
        assert_eq!(v["launches"][3]["outcome"], Value::Null);
        assert_eq!(v["launches"][3]["windowClass"], "Gone");
    }

    #[test]
    fn a_reason_is_logged_only_when_there_is_one() {
        let rec = Record {
            started_at: "T".into(),
            outcome: "disabled",
            reason: "disabled flag present".into(),
            pre_launch_ms: 0,
            post_launch_ms: 0,
            launches: Vec::new(),
        };
        assert_eq!(
            rec.log(),
            "relaunch boot  T\noutcome: disabled\nreason:  disabled flag present\ndelays:  0 ms before, 0 ms after\n"
        );
    }
}

/// Seconds since the epoch as ISO 8601 UTC (`2026-10-22T13:00:00+00:00`): the
/// fallback when the local clock cannot be formatted.
#[must_use]
pub fn iso_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod clock_tests {
    use super::iso_utc;

    #[test]
    fn utc_formatting() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00+00:00");
        assert_eq!(iso_utc(1_792_674_000), "2026-10-22T13:00:00+00:00");
    }
}
