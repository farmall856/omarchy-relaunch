//! The signed-world edits the panel relays (D-P4): `import` and `set-exec` as
//! expanding proposals for `relaunchctl commit`, `drop` as a reducing request
//! for `relaunchctl reduce`. Each reply is relayed exactly as the panel relays
//! it, through the client's own frame parser.
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

use common::{keys, panel_frame, stdout_json, world, World, GONE, OFF, TYPED};
use relaunch_core::{Entry, ExecSource, Proposal, Provenance, SignedConfig};
use serde_json::Value;

fn signed_world(tag: &str) -> World {
    let w = world(tag);
    w.write("cfg/config.json", w.base_doc().as_bytes());
    w
}

/// The proposal a `kind: "commit"` reply carries, through the panel's frame.
fn committed_proposal(w: &World, body: &Value) -> Proposal {
    assert_eq!(
        keys(body),
        BTreeSet::from(["ok", "kind", "base", "proposalJson"])
    );
    assert_eq!(
        (&body["ok"], &body["kind"]),
        (&Value::Bool(true), &Value::from("commit"))
    );
    let frame = panel_frame(
        body["base"].as_str().unwrap(),
        body["proposalJson"].as_str().unwrap(),
    );
    let req = relaunchctl::protocol::parse_prepare(frame.as_bytes()).unwrap();
    assert_eq!(req.base, w.base_doc());
    let p: Proposal = serde_json::from_str(req.proposal.get()).unwrap();
    SignedConfig::from_proposal(1000, "u", 2, p.clone()).unwrap();
    p
}

/// The pin of `window_class` on `workspace`.
fn entry<'a>(p: &'a Proposal, window_class: &str, workspace: u32) -> &'a Entry {
    p.entries
        .iter()
        .find(|e| e.window_class == window_class && e.workspace == workspace)
        .unwrap()
}

fn refused(out: &std::process::Output, needle: &str) {
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let body = stdout_json(out);
    assert_eq!(keys(&body), BTreeSet::from(["ok", "error"]));
    assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
}

#[test]
fn import_adds_one_pin_and_keeps_the_rest_of_the_base() {
    let w = signed_world("import");
    let out = w.run(
        &["import", "--class", "myapp", "--workspace", "7", "--json"],
        b"",
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    let e = entry(&p, "MyApp", 7);
    assert_eq!(
        (e.enabled, e.exec_source, e.label.as_str(), e.exec.as_str()),
        (true, ExecSource::Cmdline, "My App", "myapp"),
        "its own command runs the program its launcher does, so it is the one kept"
    );
    assert!(relaunch_core::is_launcher_id(&e.class));
    let mut ids: Vec<&str> = p.entries.iter().map(|e| e.class.as_str()).collect();
    ids.retain(|id| *id != e.class);
    assert_eq!(ids, [GONE, OFF, TYPED], "the base is kept whole");
    assert_eq!(entry(&p, "Gone", 1).exec, "gone");
}

#[test]
fn import_onto_a_pins_own_workspace_keeps_its_id_typed_command_and_label() {
    let w = signed_world("import-typed");
    let out = w.run(
        &["import", "--class", "Typed", "--workspace", "5", "--json"],
        b"",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    assert_eq!(p.entries.len(), 3, "the pin is replaced, not added");
    let e = entry(&p, "Typed", 5);
    assert_eq!(
        (
            e.class.as_str(),
            e.exec.as_str(),
            e.exec_source,
            e.label.as_str(),
            e.enabled,
            e.float
        ),
        (
            TYPED,
            "/usr/bin/bash -c my\\ --own",
            ExecSource::OverridesTable,
            "Mine",
            true,
            Some(true)
        )
    );
}

#[test]
fn import_enables_a_stored_disabled_pin_and_a_new_workspace_is_a_second_pin() {
    let w = signed_world("import-enable");
    let out = w.run(
        &["import", "--class", "Off", "--workspace", "6", "--json"],
        b"",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    let e = entry(&p, "Off", 6);
    assert_eq!(
        (e.class.as_str(), e.enabled, e.label.as_str()),
        (OFF, true, "off"),
        "a recaptured pin's label is recomputed (it is not a user field)"
    );
    let out = w.run(
        &["import", "--class", "Off", "--workspace", "4", "--json"],
        b"",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    assert_eq!(p.entries.len(), 4);
    assert_ne!(entry(&p, "Off", 4).class, OFF);
    assert!(!entry(&p, "Off", 6).enabled, "the other pin is untouched");
}

#[test]
fn import_keeps_a_terminals_whole_command_line() {
    let w = signed_world("import-hosted");
    let out = w.run(
        &["import", "--class", "foot", "--workspace", "3", "--json"],
        b"",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    let e = entry(&p, "foot", 3);
    assert_eq!(
        (e.exec_source, e.provenance, e.exec.as_str()),
        (ExecSource::Cmdline, Provenance::Host, "foot -e herdr")
    );
}

#[test]
fn import_refuses_what_cannot_be_verified_or_named() {
    let w = signed_world("import-refused");
    refused(
        &w.run(
            &[
                "import",
                "--class",
                "Sandboxed",
                "--workspace",
                "2",
                "--json",
            ],
            b"",
        ),
        "no verifiable launch command",
    );
    refused(
        &w.run(
            &[
                "import",
                "--class",
                "NotRunning",
                "--workspace",
                "2",
                "--json",
            ],
            b"",
        ),
        "no verifiable launch command",
    );
    refused(
        &w.run(
            &["import", "--class", "MyApp", "--workspace", "0", "--json"],
            b"",
        ),
        "--workspace",
    );
    refused(
        &w.run(&["import", "--workspace", "2", "--json"], b""),
        "--class",
    );
}

#[test]
fn set_exec_runs_plain_words_directly_and_shell_syntax_through_bash() {
    let w = signed_world("set-exec");
    let out = w.run(
        &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
        b"off --custom 'a b'\n",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    let e = entry(&p, "Off", 6);
    assert_eq!(
        (
            e.exec.as_str(),
            e.exec_source,
            e.provenance,
            e.label.as_str(),
            e.enabled,
            e.workspace
        ),
        (
            "off --custom a\\ b",
            ExecSource::OverridesTable,
            Provenance::None,
            "Kept",
            false,
            6
        )
    );
    assert_eq!(
        e.launcher,
        relaunch_engine::launcher::binding(&w.0.join("cfg"), OFF, "Kept", &e.exec, None).unwrap(),
        "the pin's launcher is rebound to the new command"
    );
    let out = w.run(
        &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
        b"off | tee \"$HOME/log\"\n",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    assert_eq!(
        entry(&p, "Off", 6).exec,
        "/usr/bin/bash -c off\\ |\\ tee\\ \"$HOME/log\"",
        "shell syntax runs under bash"
    );
}

/// The panel edits a pin's `execText`; set-exec reads it back to exactly the
/// signed `exec`, so saving an unedited command is no change (and never nests
/// a second `bash -c`).
#[test]
fn a_pins_command_text_reads_back_to_its_exec() {
    use relaunch_engine::edit::exec_text;
    for exec in [
        "proton-mail",
        "/opt/My\\ App/run --x",
        "foot --app-id=herdr -e herdr",
        "/usr/bin/bash -c proton-mail\\ |\\ tee\\ log",
        "/usr/bin/bash -c cd\\ /tmp\\ &&\\ make",
        "it's \"q\"",
    ] {
        let w = signed_world("exec-text");
        let text = exec_text(exec);
        let out = w.run(
            &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
            format!("{text}\n").as_bytes(),
        );
        let p = committed_proposal(&w, &stdout_json(&out));
        assert_eq!(entry(&p, "Off", 6).exec, exec, "{exec:?} shown as {text:?}");
    }
    assert_eq!(exec_text("/usr/bin/bash -c proton-mail"), "proton-mail");
    assert_eq!(exec_text("/opt/My\\ App/run --x"), "/opt/My\\ App/run --x");
}

#[test]
fn set_exec_refuses_a_command_line_an_unknown_class_and_bad_input() {
    let w = signed_world("set-exec-refused");
    let argv = w.run(&["set-exec", "--class", OFF, "--exec", "x", "--json"], b"");
    assert_eq!(argv.status.code(), Some(2), "a command on argv is refused");
    assert!(
        String::from_utf8_lossy(&argv.stderr).contains("--exec-stdin"),
        "and the refusal says how"
    );
    refused(
        &w.run(
            &["set-exec", "--class", "Nope", "--exec-stdin", "--json"],
            b"x\n",
        ),
        "no relaunch entry",
    );
    refused(
        &w.run(
            &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
            b"\n",
        ),
        "requires a command",
    );
    refused(
        &w.run(
            &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
            b"a\x1bb\n",
        ),
        "control character",
    );
    refused(
        &w.run(&["set-exec", "--class", OFF, "--json"], b"x\n"),
        "--exec-stdin",
    );
}

#[test]
fn drop_is_a_reduce_request_for_the_client() {
    let w = signed_world("drop");
    let out = w.run(&["drop", "--class", GONE, "--json"], b"");
    assert_eq!(out.status.code(), Some(0));
    let body = stdout_json(&out);
    assert_eq!(
        keys(&body),
        BTreeSet::from(["ok", "kind", "base", "reduceJson"])
    );
    assert_eq!(body["kind"], "reduce");
    let req = relaunchctl::protocol::parse_reduce(body["reduceJson"].as_str().unwrap().as_bytes())
        .unwrap();
    assert_eq!(req.base, w.base_doc());
    assert_eq!(req.ops.len(), 1);
    assert_eq!(
        (req.ops[0].op, req.ops[0].class.as_str()),
        (relaunchctl::protocol::ReduceKind::Drop, GONE)
    );
    refused(
        &w.run(&["drop", "--class", "Nope", "--json"], b""),
        "no relaunch entry",
    );
}

#[test]
fn edits_never_write_a_file() {
    let w = signed_world("no-writes");
    let before = std::fs::read_dir(w.0.join("cfg")).unwrap().count();
    for args in [
        &["import", "--class", "MyApp", "--workspace", "7", "--json"][..],
        &["set-exec", "--class", OFF, "--exec-stdin", "--json"],
        &["drop", "--class", GONE, "--json"],
    ] {
        w.run(args, b"cmd\n");
    }
    assert_eq!(std::fs::read_dir(w.0.join("cfg")).unwrap().count(), before);
    assert_eq!(
        std::fs::read_to_string(w.0.join("cfg/config.json")).unwrap(),
        w.base_doc()
    );
}

#[test]
fn set_delays_keeps_every_pin_and_changes_only_the_delays() {
    let w = signed_world("delays");
    let out = w.run(
        &["set-delays", "--pre", "40", "--post", "1200", "--json"],
        b"",
    );
    let p = committed_proposal(&w, &stdout_json(&out));
    assert_eq!((p.pre_launch_ms, p.post_launch_ms), (40, 1200));
    let base: relaunch_core::ConfigFile = serde_json::from_str(&w.base_doc()).unwrap();
    assert_eq!(p.entries, base.signed.entries);
    refused(
        &w.run(
            &["set-delays", "--pre", "0", "--post", "60001", "--json"],
            b"",
        ),
        "at most 60000 ms",
    );
    refused(
        &w.run(&["set-delays", "--pre", "-1", "--post", "1", "--json"], b""),
        "--pre MS",
    );
    refused(
        &w.run(&["set-delays", "--pre", "1", "--json"], b""),
        "--post MS",
    );
}
