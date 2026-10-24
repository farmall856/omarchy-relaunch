//! The `propose --json` contract the panel depends on (`TxnLifecycle.js`
//! `proposeResult`): pinned here because the panel is not exercised against the
//! Rust engine until the cutover. The output is relayed exactly as the panel
//! relays it — `{"base":<base>,"proposal":<proposalJson>}` through the client's
//! frame parser, then decoded as the service decodes a proposal.
#![cfg(feature = "fixtures")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

mod common;

use std::collections::BTreeSet;

use common::{keys, panel_frame, stdout_json, world, OFF, TYPED};
use relaunch_core::{ExecSource, Proposal, SignedConfig};

#[test]
fn propose_emits_exactly_the_panel_contract() {
    let w = world("contract");
    w.write("cfg/config.json", w.base_doc().as_bytes());
    let out = w.propose();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = stdout_json(&out);
    assert_eq!(
        keys(&body),
        BTreeSet::from([
            "ok",
            "kind",
            "base",
            "preLaunchMs",
            "postLaunchMs",
            "entries",
            "candidates",
            "dropping",
            "merged",
            "proposalJson"
        ])
    );
    assert_eq!(body["ok"], true);
    assert_eq!(body["kind"], "commit");
    assert_eq!(body["base"].as_str(), Some(w.base_doc().as_str()));
    assert_eq!(
        (&body["preLaunchMs"], &body["postLaunchMs"]),
        (&0.into(), &300.into())
    );
    assert_eq!(body["entries"].as_array().map(Vec::len), Some(3));
    assert_eq!(body["candidates"].as_array().map(Vec::len), Some(9));
    let backslash = body["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["class"] == "Back\\slash")
        .unwrap();
    assert_eq!(
        (&backslash["eligible"], &backslash["reason"]),
        (&false.into(), &"no-command".into()),
        "a trusted window with neither a launcher nor a command line is an unsigned guess"
    );

    let frame = panel_frame(
        body["base"].as_str().unwrap(),
        body["proposalJson"].as_str().unwrap(),
    );
    let req = relaunchctl::protocol::parse_prepare(frame.as_bytes()).unwrap();
    assert_eq!(req.base, w.base_doc());
    let proposal: Proposal = serde_json::from_str(req.proposal.get()).unwrap();
    let doc = SignedConfig::from_proposal(1000, "u", 2, proposal.clone()).unwrap();
    assert_eq!(doc.entries, proposal.entries, "already in canonical order");

    assert_eq!(
        (proposal.pre_launch_ms, proposal.post_launch_ms),
        (0, 300),
        "the base's delays"
    );
    check_pins(&w, &proposal);
}

/// What each window's pin in the contract world must be.
fn check_pins(w: &common::World, proposal: &Proposal) {
    let by = |c: &str| {
        let mut it = proposal.entries.iter().filter(|e| e.window_class == c);
        let e = it.next();
        assert!(it.next().is_none(), "{c}: one pin");
        e
    };
    let classes: BTreeSet<&str> = proposal
        .entries
        .iter()
        .map(|e| e.window_class.as_str())
        .collect();
    assert_eq!(
        classes,
        BTreeSet::from(["MyApp", "Off", "Typed", "__proto__", "constructor", "foot"]),
        "exactly the open windows that may be signed: Gone is closed, Sandboxed untrusted, \
         Scratch special, and Back\\slash has no command (no launcher, no command line)"
    );
    let typed = by("Typed").unwrap();
    assert_eq!(
        (
            typed.class.as_str(),
            typed.exec.as_str(),
            typed.exec_source,
            typed.label.as_str(),
            typed.enabled,
            typed.workspace,
            typed.float
        ),
        (
            TYPED,
            "/usr/bin/bash -c my\\ --own",
            ExecSource::OverridesTable,
            "Mine",
            true,
            5,
            Some(true)
        )
    );
    let off = by("Off").unwrap();
    assert_eq!(
        (
            off.class.as_str(),
            off.enabled,
            off.label.as_str(),
            off.workspace,
            off.exec.as_str()
        ),
        (OFF, true, "off", 6, "off"),
        "an open window's pin keeps its id, its label is recomputed, and a Save enables it"
    );
    let myapp = by("MyApp").unwrap();
    assert_eq!(
        (myapp.exec_source, myapp.label.as_str(), myapp.exec.as_str()),
        (ExecSource::Cmdline, "My App", "myapp"),
        "its own command line runs its launcher's program, so it is kept"
    );
    assert!(myapp
        .launcher
        .path
        .starts_with(&w.0.join("cfg/launchers").to_string_lossy().into_owned()));
    assert!(by("Back\\slash").is_none());
    let foot = by("foot").unwrap();
    assert_eq!(
        (foot.exec_source, foot.exec.as_str()),
        (ExecSource::Cmdline, "foot -e herdr"),
        "a terminal's whole command line, flags and all"
    );
    assert!(
        !w.0.join("cfg/launchers").exists(),
        "a proposal writes no launcher"
    );
}

#[test]
fn propose_with_no_config_has_an_empty_base() {
    let w = world("empty");
    let out = w.propose();
    assert_eq!(out.status.code(), Some(0));
    let body = stdout_json(&out);
    assert_eq!(body["base"], "");
    let frame = panel_frame("", body["proposalJson"].as_str().unwrap());
    let req = relaunchctl::protocol::parse_prepare(frame.as_bytes()).unwrap();
    let proposal: Proposal = serde_json::from_str(req.proposal.get()).unwrap();
    assert!(
        proposal.entries.iter().all(|e| e.enabled),
        "with no base, every trusted capture is proposed enabled"
    );
    assert!(proposal.entries.iter().all(|e| e.class != "Sandboxed"));
}

#[test]
fn propose_refuses_a_malformed_signed_config_with_the_error_contract() {
    let w = world("invalid");
    w.write(
        "cfg/config.json",
        br#"{"v":2,"signed":null,"signature":"ab"}"#,
    );
    let out = w.propose();
    assert_eq!(out.status.code(), Some(1));
    let body = stdout_json(&out);
    assert_eq!(keys(&body), BTreeSet::from(["ok", "error"]));
    assert_eq!(body["ok"], false);
    assert!(body["error"]
        .as_str()
        .is_some_and(|e| e.contains("config.json")));
}

#[test]
fn propose_reports_a_failed_hyprctl_with_the_error_contract() {
    let w = world("nohypr");
    std::fs::remove_file(w.0.join("hyprctl")).unwrap();
    let out = w.propose();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout_json(&out)["ok"], false);
}

/// "Capture open windows" replaces, and says which pins that drops so the
/// panel can ask first: here Gone, whose window is not open.
#[test]
fn a_replace_names_every_pin_it_would_drop() {
    let w = world("dropping");
    w.write("cfg/config.json", w.base_doc().as_bytes());
    let body = stdout_json(&w.propose());
    assert_eq!(
        body["dropping"],
        serde_json::json!([{ "label": "Gone", "workspace": 1 }])
    );
    let proposal: Proposal = serde_json::from_str(body["proposalJson"].as_str().unwrap()).unwrap();
    assert!(proposal.entries.iter().all(|e| e.window_class != "Gone"));
}

/// "Add open windows" merges: every pin of the base is kept, the open
/// windows are added or refreshed, and nothing is dropped.
#[test]
fn a_merge_keeps_every_pin_and_drops_nothing() {
    let w = world("merge");
    w.write("cfg/config.json", w.base_doc().as_bytes());
    let out = w.run(&["propose", "--merge", "--json"], b"");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let body = stdout_json(&out);
    assert_eq!(body["dropping"], serde_json::json!([]));
    let proposal: Proposal = serde_json::from_str(body["proposalJson"].as_str().unwrap()).unwrap();
    let base: relaunch_core::ConfigFile = serde_json::from_str(&w.base_doc()).unwrap();
    for pin in &base.signed.entries {
        assert!(
            proposal.entries.iter().any(|e| e.class == pin.class),
            "{} kept",
            pin.window_class
        );
    }
    let gone = proposal
        .entries
        .iter()
        .find(|e| e.window_class == "Gone")
        .unwrap();
    assert_eq!(
        gone, &base.signed.entries[0],
        "a pin whose window is not open is kept unchanged"
    );
    let classes: BTreeSet<&str> = proposal
        .entries
        .iter()
        .map(|e| e.window_class.as_str())
        .collect();
    for open in ["MyApp", "Off", "Typed", "__proto__", "constructor", "foot"] {
        assert!(classes.contains(open), "{open} added or refreshed");
    }
    let off = proposal
        .entries
        .iter()
        .find(|e| e.window_class == "Off")
        .unwrap();
    assert!(
        off.enabled,
        "an open window's pin is refreshed like a capture"
    );
    SignedConfig::from_proposal(1000, "u", 2, proposal).unwrap();
}
