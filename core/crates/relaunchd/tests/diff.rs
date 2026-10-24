//! Chunk-2 tests for the review diff: escaping of dangerous characters and the
//! D7 caps (refuse, never truncate).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

use relaunch_core::{Entry, ExecSource, Launcher, Provenance, SignedConfig, SIGNED_SCHEMA_VERSION};
use relaunchd::diff::{compute, escape, DiffError, APPROVE_PROMPT, MAX_CHANGED};

fn cfg(entries: Vec<Entry>) -> SignedConfig {
    SignedConfig {
        v: SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "laytonf".into(),
        gen: 0,
        pre_launch_ms: 0,
        post_launch_ms: 500,
        entries,
    }
}

/// A deterministic pin id for a readable test name (FNV-1a), so a test names
/// apps the way a reviewer sees them while the signed key stays a real id.
fn pin(name: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("relauncher-{h:016x}")
}

fn launcher(id: &str) -> Launcher {
    Launcher {
        path: format!("/home/laytonf/.config/omarchy-relaunch/launchers/{id}.desktop"),
        sha256: "ab".repeat(32),
    }
}

fn e(name: &str) -> Entry {
    Entry {
        class: pin(name),
        window_class: name.into(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: format!("run-{name}"),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: launcher(&pin(name)),
        cwd: None,
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
    assert!(d
        .rendered
        .contains(&format!("+ class=\"{}\" window=\"add\"", pin("add"))));
    assert!(d
        .rendered
        .contains(&format!("- class=\"{}\" window=\"drop\"", pin("drop"))));
    assert!(d
        .rendered
        .contains(&format!("~ class=\"{}\" window=\"keep\"", pin("keep"))));
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
fn a_delay_change_is_shown_and_is_a_change() {
    let base = cfg(vec![e("a")]);
    let mut prop = cfg(vec![e("a")]);
    prop.pre_launch_ms = 100;
    prop.post_launch_ms = 750;
    let d = compute(&base, &prop, "deadbeef").unwrap();
    assert!(
        d.rendered.contains("~ preLaunchMs 0 -> 100"),
        "{}",
        d.rendered
    );
    assert!(
        d.rendered.contains("~ postLaunchMs 500 -> 750"),
        "{}",
        d.rendered
    );
    assert!(
        d.summary
            .ends_with("Launch delays: 100 ms before, 750 ms after (was 0 ms, 500 ms)"),
        "{}",
        d.summary
    );
    // Delay-only edit: no changed entries, but still a committable change (L1).
    assert_eq!(d.changed, 0);
    assert!(d.has_change);
}

#[test]
fn different_shell_commands_render_differently() {
    // Reviewer H2: two valid entries with different eligible commands must not
    // produce identical review text, or the review cannot tell them apart.
    fn shell(name: &str, exec: &str) -> Entry {
        Entry {
            class: pin(name),
            window_class: name.into(),
            workspace: 1,
            float: None,
            label: String::new(),
            exec: exec.into(),
            exec_source: ExecSource::OverridesTable,
            provenance: Provenance::None,
            launcher: launcher(&pin(name)),
            cwd: None,
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

fn app(class: &str, label: &str, ws: u32) -> Entry {
    Entry {
        label: label.into(),
        workspace: ws,
        ..e(class)
    }
}

#[test]
fn the_summary_names_every_app_that_will_open_by_label_and_workspace() {
    let mut edited = app("pass", "Proton Pass", 4);
    let base = cfg(vec![
        edited.clone(),
        app("maps", "Google Maps", 8),
        app("gone", "Old App", 2),
        app("quiet", "Quiet", 3),
    ]);
    edited.exec = "something-else".into();
    let mut off = app("quiet", "Quiet", 3);
    off.enabled = false;
    let prop = cfg(vec![
        edited,
        app("maps", "Google Maps", 8),
        app("chat.example", "", 1),
        off,
    ]);
    let d = compute(&base, &prop, "deadbeef").unwrap();
    assert_eq!(
        d.summary,
        "Relaunch will open these 3 apps at login:\n\
         \x20 chat.example \u{2014} workspace 1 (new)\n\
         \x20 Proton Pass \u{2014} workspace 4 (changed)\n\
         \x20 Google Maps \u{2014} workspace 8\n\
         It will no longer open:\n\
         \x20 Old App \u{2014} workspace 2\n\
         \x20 Quiet \u{2014} workspace 3"
    );
    assert!(
        !d.summary.contains("something-else"),
        "no exec in the summary"
    );
    assert_eq!(d.prompt, APPROVE_PROMPT);
    assert!(
        !d.prompt.contains("something-else"),
        "no exec in the prompt"
    );
    assert!(
        d.rendered.contains("exec=\"something-else\""),
        "the detail keeps it"
    );
}

#[test]
fn the_summary_escapes_a_label_and_lists_many_apps() {
    let d = compute(
        &cfg(vec![]),
        &cfg(vec![app("x", "A\n  Fake \u{2014} workspace 9", 1)]),
        "d",
    )
    .unwrap();
    assert_eq!(d.summary.lines().count(), 2, "{}", d.summary);
    assert!(d.summary.contains("A\\x0a  Fake"), "{}", d.summary);

    let many: Vec<Entry> = (0..20)
        .map(|i| app(&format!("c{i:02}"), &"L".repeat(300), 1))
        .collect();
    let d = compute(&cfg(vec![]), &cfg(many), "d").unwrap();
    assert_eq!(d.prompt, APPROVE_PROMPT);
    assert_eq!(d.summary.lines().count(), 21);
}

#[test]
fn the_summary_says_when_nothing_will_open() {
    let base = cfg(vec![app("a", "A", 1)]);
    let d = compute(&base, &cfg(vec![]), "d").unwrap();
    assert_eq!(
        d.summary,
        "Relaunch will open no apps at login.\nIt will no longer open:\n  A \u{2014} workspace 1"
    );
    assert_eq!(d.prompt, APPROVE_PROMPT);
}
