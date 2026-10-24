//! The service-computed review of a change: a plain-language summary (what
//! will open at login), the escaped diff behind it, the short credential-prompt
//! text, and the D7 caps that keep a change reviewable.
//!
//! The service is filesystem-blind, so the caps are byte/count limits it can
//! know (spec §D7): never QML geometry. The panel shows the summary and diff
//! bytes verbatim (spec §3.1); the service never truncates — it refuses instead.

use relaunch_core::{Entry, SignedConfig};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Max changed entries per transaction (spec §D7). Distinct from the 256 total.
pub const MAX_CHANGED: usize = 64;
/// Byte cap per rendered line, after escaping.
pub const MAX_LINE_BYTES: usize = 8 * 1024;
/// Byte cap on the whole rendered diff.
pub const MAX_DIFF_BYTES: usize = 256 * 1024;

/// Why a proposal's review diff was refused rather than truncated (spec §D7).
#[derive(Debug, PartialEq, Eq)]
pub enum DiffError {
    /// More than `MAX_CHANGED` entries changed (the count).
    TooManyChanges(usize),
    /// One rendered line exceeds `MAX_LINE_BYTES`.
    LineTooLong,
    /// The whole rendering exceeds `MAX_DIFF_BYTES`.
    DiffTooLong,
}

/// The escaped review diff of a proposal against its verified base.
pub struct Diff {
    /// Changed entries, for the D7 cap.
    pub changed: usize,
    /// Whether any signed field changed at all, including the delays. A
    /// delay-only edit has `changed == 0` but `has_change == true`, so it is
    /// still committable (reviewer L1).
    pub has_change: bool,
    /// The full escaped diff, shown verbatim under the panel's Details.
    pub rendered: String,
    /// What the change means, in plain language: one row per app that will
    /// open at login, and the ones that no longer will. Shown verbatim.
    pub summary: String,
    /// The short credential-prompt text: the same summary, bounded. Never
    /// carries a launch command.
    pub prompt: String,
}

/// The fixed credential-prompt line shown above the fingerprint request. The
/// panel already shows the full reviewed change (summary + diff); this stays a
/// single short line and never names apps or carries any exec text.
pub const APPROVE_PROMPT: &str = "Approve this change to your Relaunch login config.";

/// Escape a value so nothing it contains can forge or reorder structure in the
/// review surface: C0/C1 controls, DEL, backslash, and any non-ASCII byte (incl.
/// bidi/format controls, which `relaunch-core` already rejects in a signed
/// field but which may still appear while diffing a rejected proposal) become
/// visible `\xNN` / `\u{...}` escapes.
#[must_use]
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            // A quote is escaped so it cannot close a quoted field early; this is
            // what makes a quoted value's bytes unambiguous (reviewer H2).
            '"' => out.push_str("\\x22"),
            ' '..='~' => out.push(c),
            _ if cp < 0x100 => {
                let _ = write!(out, "\\x{cp:02x}");
            }
            _ => {
                let _ = write!(out, "\\u{{{cp:04x}}}");
            }
        }
    }
    out
}

/// A string field, always quoted and escaped, so two different values can never
/// render to the same bytes (reviewer H2).
fn q(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

fn fmt_entry(e: &Entry) -> String {
    let float = match e.float {
        Some(true) => "float",
        Some(false) => "tile",
        None => "unknown",
    };
    // Every string field is quoted (class/windowClass/label/exec/launcher); the
    // rest are fixed keywords, numbers or hex. `label` is included (reviewer H2).
    format!(
        "class={} window={} label={} ws={} geom={} src={:?} prov={:?} enabled={} exec={} cwd={} launcher={} sha256={}",
        q(&e.class),
        q(&e.window_class),
        q(&e.label),
        e.workspace,
        float,
        e.exec_source,
        e.provenance,
        e.enabled,
        q(&e.exec),
        e.cwd.as_deref().map_or_else(|| "-".to_owned(), q),
        q(&e.launcher.path),
        e.launcher.sha256,
    )
}

/// What a reviewer calls an entry: its label, or its window class without
/// one. Never its pin id.
fn name(e: &Entry) -> &str {
    if e.label.is_empty() {
        &e.window_class
    } else {
        &e.label
    }
}

/// How an app that will open compares with the base.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Same,
    New,
    Changed,
}

/// One app in the summary, in panel order (workspace, then name).
struct App<'a> {
    entry: &'a Entry,
    mark: Mark,
}

fn by_workspace(mut apps: Vec<App<'_>>) -> Vec<App<'_>> {
    apps.sort_by(|a, b| {
        (a.entry.workspace, name(a.entry), &a.entry.class).cmp(&(
            b.entry.workspace,
            name(b.entry),
            &b.entry.class,
        ))
    });
    apps
}

/// The apps the proposal opens at login, and those the base opened that it no
/// longer does.
fn apps<'a>(
    old: &BTreeMap<&str, &'a Entry>,
    new: &BTreeMap<&str, &'a Entry>,
) -> (Vec<App<'a>>, Vec<App<'a>>) {
    let opens = new
        .values()
        .filter(|e| e.enabled)
        .map(|e| {
            let mark = match old.get(e.class.as_str()) {
                Some(b) if b == e => Mark::Same,
                Some(b) if b.enabled => Mark::Changed,
                _ => Mark::New,
            };
            App { entry: e, mark }
        })
        .collect();
    let stops = old
        .values()
        .filter(|e| e.enabled && !new.get(e.class.as_str()).is_some_and(|n| n.enabled))
        .map(|e| App {
            entry: e,
            mark: Mark::Same,
        })
        .collect();
    (by_workspace(opens), by_workspace(stops))
}

/// The launch delays, before and after: `(pre, post)` in milliseconds.
type Delays = ((u32, u32), (u32, u32));

fn summary(opens: &[App<'_>], stops: &[App<'_>], delays: Option<Delays>) -> Vec<String> {
    let row = |a: &App<'_>| {
        let mark = match a.mark {
            Mark::Same => "",
            Mark::New => " (new)",
            Mark::Changed => " (changed)",
        };
        format!(
            "  {} \u{2014} workspace {}{mark}",
            escape(name(a.entry)),
            a.entry.workspace
        )
    };
    let mut lines = Vec::new();
    if opens.is_empty() {
        lines.push("Relaunch will open no apps at login.".to_string());
    } else {
        lines.push(format!(
            "Relaunch will open {} {} at login:",
            if opens.len() == 1 { "this" } else { "these" },
            if opens.len() == 1 {
                "app".to_string()
            } else {
                format!("{} apps", opens.len())
            }
        ));
        lines.extend(opens.iter().map(row));
    }
    if !stops.is_empty() {
        lines.push("It will no longer open:".to_string());
        lines.extend(stops.iter().map(row));
    }
    if let Some(((was_pre, was_post), (pre, post))) = delays {
        lines.push(format!(
            "Launch delays: {pre} ms before, {post} ms after (was {was_pre} ms, {was_post} ms)"
        ));
    }
    lines
}

fn index(cfg: &SignedConfig) -> BTreeMap<&str, &Entry> {
    cfg.entries.iter().map(|e| (e.class.as_str(), e)).collect()
}

/// Compute the full escaped diff base -> proposal, enforcing the D7 caps.
/// `digest_short` is the spec §3.1 header tie between the panel and the signed
/// bytes (a short hash of the proposal's canonical bytes, supplied by the
/// engine). `has_change == false` means nothing to authorize.
///
/// # Errors
///
/// A [`DiffError`] when the change exceeds a D7 cap; the proposal must
/// be split into smaller transactions.
pub fn compute(
    base: &SignedConfig,
    proposal: &SignedConfig,
    digest_short: &str,
) -> Result<Diff, DiffError> {
    let old = index(base);
    let new = index(proposal);

    let mut lines: Vec<String> = Vec::new();

    // The delays are signed scheduling; show them when they change.
    let was = (base.pre_launch_ms, base.post_launch_ms);
    let now = (proposal.pre_launch_ms, proposal.post_launch_ms);
    let delays_changed = was != now;
    if was.0 != now.0 {
        lines.push(format!("~ preLaunchMs {} -> {}", was.0, now.0));
    }
    if was.1 != now.1 {
        lines.push(format!("~ postLaunchMs {} -> {}", was.1, now.1));
    }

    let mut classes: Vec<&str> = old.keys().chain(new.keys()).copied().collect();
    classes.sort_unstable();
    classes.dedup();

    let mut changed = 0usize;
    for class in classes {
        match (old.get(class), new.get(class)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                changed += 1;
                lines.push(format!("~ {}", fmt_entry(a)));
                lines.push(format!("  -> {}", fmt_entry(b)));
            }
            (None, Some(b)) => {
                changed += 1;
                lines.push(format!("+ {}", fmt_entry(b)));
            }
            (Some(a), None) => {
                changed += 1;
                lines.push(format!("- {}", fmt_entry(a)));
            }
            (None, None) => unreachable!(),
        }
    }

    if changed > MAX_CHANGED {
        return Err(DiffError::TooManyChanges(changed));
    }
    let (opens, stops) = apps(&old, &new);
    let summary_lines = summary(&opens, &stops, delays_changed.then_some((was, now)));
    for l in lines.iter().chain(&summary_lines) {
        if l.len() > MAX_LINE_BYTES {
            return Err(DiffError::LineTooLong);
        }
    }
    let has_change = changed > 0 || delays_changed;
    let header = format!("Relaunch: {changed} login-start change(s) [{digest_short}]\n");
    let rendered = header + &lines.join("\n");
    let summary = summary_lines.join("\n");
    if rendered.len() + summary.len() > MAX_DIFF_BYTES {
        return Err(DiffError::DiffTooLong);
    }
    Ok(Diff {
        changed,
        has_change,
        rendered,
        summary,
        prompt: APPROVE_PROMPT.to_string(),
    })
}
