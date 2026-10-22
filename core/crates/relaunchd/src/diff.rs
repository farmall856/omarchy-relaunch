//! The service-computed, escaped diff the panel renders in full, and the D7
//! caps that keep a change reviewable.
//!
//! The service is filesystem-blind, so the caps are byte/count limits it can
//! know (spec §D7): never QML geometry. The panel shows these exact bytes
//! verbatim (spec §3.1); the service never truncates — it refuses instead.

use relaunch_core::{Entry, SignedConfig};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Max changed entries per transaction (spec §D7). Distinct from the 256 total.
pub const MAX_CHANGED: usize = 64;
/// Byte cap per rendered line, after escaping.
pub const MAX_LINE_BYTES: usize = 8 * 1024;
/// Byte cap on the whole rendered diff.
pub const MAX_DIFF_BYTES: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum DiffError {
    TooManyChanges(usize),
    LineTooLong,
    DiffTooLong,
}

pub struct Diff {
    pub changed: usize,
    pub rendered: String,
}

/// Escape a value so nothing it contains can forge or reorder structure in the
/// review surface: C0/C1 controls, DEL, backslash, and any non-ASCII byte (incl.
/// bidi/format controls, which `relaunch-core` already rejects in a signed
/// field but which may still appear while diffing a rejected proposal) become
/// visible `\xNN` / `\u{...}` escapes.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c), // printable ASCII except backslash (handled above)
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

fn fmt_entry(e: &Entry) -> String {
    let launcher = match &e.launcher {
        Some(l) => format!(
            " launcher={}{}",
            escape(&l.path),
            l.sha256
                .as_deref()
                .map(|h| format!("#{h}"))
                .unwrap_or_default()
        ),
        None => String::new(),
    };
    let float = match e.float {
        Some(true) => " float",
        Some(false) => " tile",
        None => "",
    };
    format!(
        "class={} ws={}{} src={:?} prov={:?} enabled={} exec={}{}",
        escape(&e.class),
        e.workspace,
        float,
        e.exec_source,
        e.provenance,
        e.enabled,
        escape(&e.exec),
        launcher,
    )
}

fn index(cfg: &SignedConfig) -> BTreeMap<&str, &Entry> {
    cfg.entries.iter().map(|e| (e.class.as_str(), e)).collect()
}

/// Compute the full escaped diff base -> proposal, enforcing the D7 caps.
/// `changed == 0` means an empty diff (caller returns "nothing to do").
pub fn compute(base: &SignedConfig, proposal: &SignedConfig) -> Result<Diff, DiffError> {
    let old = index(base);
    let new = index(proposal);

    let mut lines: Vec<String> = Vec::new();

    // Stagger is security-relevant to scheduling; show it when it changes.
    if base.stagger_seconds != proposal.stagger_seconds {
        lines.push(format!(
            "~ staggerSeconds {} -> {}",
            base.stagger_seconds, proposal.stagger_seconds
        ));
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
    for l in &lines {
        if l.len() > MAX_LINE_BYTES {
            return Err(DiffError::LineTooLong);
        }
    }
    let header = format!("Relaunch: {changed} login-start change(s)\n");
    let rendered = header + &lines.join("\n");
    if rendered.len() > MAX_DIFF_BYTES {
        return Err(DiffError::DiffTooLong);
    }
    Ok(Diff { changed, rendered })
}
