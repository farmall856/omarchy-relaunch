//! Chunk-2 tests for the review diff: escaping of dangerous characters and the
//! D7 caps (refuse, never truncate).

use relaunch_core::{Entry, ExecSource, Provenance, SignedConfig, SIGNED_SCHEMA_VERSION};
use relaunchd::diff::{compute, escape, DiffError, MAX_CHANGED};

fn cfg(entries: Vec<Entry>) -> SignedConfig {
    SignedConfig {
        v: SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "laytonf".into(),
        gen: 0,
        stagger_seconds: 0,
        entries,
    }
}

fn e(class: &str) -> Entry {
    Entry {
        class: class.into(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: format!("run-{class}"),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: None,
        enabled: true,
    }
}

#[test]
fn escape_renders_controls_and_bidi_visibly() {
    assert_eq!(escape("a\tb"), "a\\x09b");
    assert_eq!(escape("x\u{202e}y"), "x\\u{202e}y");
    assert_eq!(escape("c:\\path"), "c:\\\\path");
    assert_eq!(escape("plain text-1/2"), "plain text-1/2");
}

#[test]
fn additions_removals_and_changes_are_counted() {
    let base = cfg(vec![e("keep"), e("drop")]);
    let mut changed = e("keep");
    changed.workspace = 9;
    let prop = cfg(vec![changed, e("add")]);
    let d = compute(&base, &prop, "deadbeef").unwrap();
    assert_eq!(d.changed, 3); // keep changed, drop removed, add added
    assert!(d.rendered.contains("+ class=\"add\""));
    assert!(d.rendered.contains("- class=\"drop\""));
    assert!(d.rendered.contains("~ class=\"keep\""));
}

#[test]
fn identical_entry_is_not_a_change() {
    let base = cfg(vec![e("a"), e("b")]);
    let prop = cfg(vec![e("a"), e("b")]);
    assert_eq!(compute(&base, &prop, "deadbeef").unwrap().changed, 0);
}

#[test]
fn more_than_max_changed_is_refused_not_truncated() {
    let base = cfg(vec![]);
    let entries: Vec<Entry> = (0..=MAX_CHANGED)
        .map(|i| e(&format!("app{i:03}")))
        .collect();
    let prop = cfg(entries);
    assert!(
        matches!(compute(&base, &prop, "deadbeef"), Err(DiffError::TooManyChanges(n)) if n == MAX_CHANGED + 1)
    );
}

#[test]
fn exactly_max_changed_is_allowed() {
    let base = cfg(vec![]);
    let entries: Vec<Entry> = (0..MAX_CHANGED).map(|i| e(&format!("app{i:03}"))).collect();
    let prop = cfg(entries);
    assert_eq!(
        compute(&base, &prop, "deadbeef").unwrap().changed,
        MAX_CHANGED
    );
}

#[test]
fn stagger_change_is_shown_and_is_a_change() {
    let base = cfg(vec![e("a")]);
    let mut prop = cfg(vec![e("a")]);
    prop.stagger_seconds = 5;
    let d = compute(&base, &prop, "deadbeef").unwrap();
    assert!(d.rendered.contains("staggerSeconds 0 -> 5"));
    // Stagger-only edit: no changed entries, but still a committable change (L1).
    assert_eq!(d.changed, 0);
    assert!(d.has_change);
}

#[test]
fn different_shell_commands_render_differently() {
    // Reviewer H2: two valid entries with different eligible commands must not
    // produce identical review text, or the pairing-code consent is defeated.
    fn shell(class: &str, exec: &str) -> Entry {
        Entry {
            class: class.into(),
            workspace: 1,
            float: None,
            label: String::new(),
            exec: exec.into(),
            exec_source: ExecSource::OverridesTable,
            provenance: Provenance::None,
            launcher: None,
            enabled: true,
        }
    }
    let base = cfg(vec![]);
    // Crafted so an unquoted concatenation would collide: the class/exec split
    // differs but the raw characters are arranged to look alike.
    let a = compute(&base, &cfg(vec![shell("x", "a b")]), "d")
        .unwrap()
        .rendered;
    let b = compute(&base, &cfg(vec![shell("x", "a  b")]), "d")
        .unwrap()
        .rendered;
    assert_ne!(a, b);
    // And a value containing a quote cannot forge a field boundary.
    let c = compute(
        &base,
        &cfg(vec![shell("x", "a\" enabled=true exec=\"evil")]),
        "d",
    )
    .unwrap()
    .rendered;
    assert_ne!(a, c);
    assert!(c.contains("\\x22"));
}

#[test]
fn header_carries_the_digest_short_hash() {
    let base = cfg(vec![]);
    let d = compute(&base, &cfg(vec![e("a")]), "cafebabe").unwrap();
    assert!(d
        .rendered
        .starts_with("Relaunch: 1 login-start change(s) [cafebabe]"));
}
