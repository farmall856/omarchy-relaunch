//! Unit goldens generated from the bash oracle
//! (`tests/differential/gen_unit_goldens.py`): the Rust engine must reproduce every output byte.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

use std::path::Path;
use std::process::Command;

use relaunch_engine::{argv, key, shell, webapp};
use serde_json::Value;

fn cases(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

/// Every mismatch, not just the first, so a regression shows its whole shape.
/// A case listed in `<file stem>.dlm.jsonl` is a reviewed DLM delta (no
/// application is named, so no terminal is seen through and any program can
/// open a web app): it must give that file's output, which must differ from
/// bash's.
fn check<F: Fn(&Value) -> Value>(file: &str, f: F) {
    let mut bad = Vec::new();
    let all = cases(file);
    let dlm_file = file.replace(".jsonl", ".dlm.jsonl");
    let dlm: Vec<Value> = if Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(&dlm_file)
        .exists()
    {
        cases(&dlm_file)
    } else {
        Vec::new()
    };
    for d in &dlm {
        match all.iter().find(|c| c["in"] == d["in"]) {
            None => bad.push(format!("{dlm_file}: in={} is not a bash case", d["in"])),
            Some(c) if c["out"] == d["out"] => {
                bad.push(format!(
                    "{dlm_file}: in={} equals bash (not a delta)",
                    d["in"]
                ));
            }
            Some(_) => {}
        }
    }
    for c in &all {
        let got = f(&c["in"]);
        let want = dlm
            .iter()
            .find(|d| d["in"] == c["in"])
            .map_or(&c["out"], |d| &d["out"]);
        if got != *want {
            bad.push(format!("in={} want={want} got={got}", c["in"]));
        }
    }
    assert!(
        bad.is_empty(),
        "{file}: {} of {} differ:\n{}",
        bad.len(),
        all.len(),
        bad.join("\n")
    );
}

#[test]
fn quote_argv_matches_bash_printf_q() {
    check("quote.jsonl", |i| {
        Value::from(shell::quote_argv(&strings(i)))
    });
}

#[test]
fn split_stored_matches_bash_unquote_argv_arr() {
    check("split.jsonl", |i| {
        Value::from(shell::split_stored(i.as_str().unwrap()))
    });
}

#[test]
fn split_ifs_matches_bash_split_exec_words() {
    check("split_ifs.jsonl", |i| {
        Value::from(shell::split_ifs(i.as_str().unwrap()))
    });
}

#[test]
fn nkey_matches_bash_normalize_desktop_key() {
    check("nkey.jsonl", |i| {
        Value::from(key::nkey(i.as_str().unwrap()))
    });
}

#[test]
fn webapp_url_matches_bash() {
    check("webapp_url.jsonl", |i| {
        Value::from(webapp::url_from_exec(i.as_str().unwrap()))
    });
}

#[test]
fn webapp_identity_matches_bash() {
    check("webapp_ident.jsonl", |i| {
        Value::from(webapp::identity_from_url(i.as_str().unwrap()))
    });
}

#[test]
fn leaf_matches_bash() {
    check("leaf.jsonl", |i| {
        Value::from(argv::leaf_for_exec(i.as_str().unwrap()))
    });
}

/// D-P2's real gate: whatever the quoter emits, bash must split back into the
/// exact argv — for the parity corpus AND the non-ASCII fallback corpus.
#[test]
fn quoted_argv_round_trips_through_bash() {
    let bash = Path::new("/usr/bin/bash");
    assert!(bash.exists(), "the round-trip gate needs /usr/bin/bash");
    let mut argvs: Vec<Vec<String>> = cases("quote.jsonl")
        .iter()
        .map(|c| strings(&c["in"]))
        .collect();
    argvs.extend(
        cases("quote_roundtrip.jsonl")
            .iter()
            .map(|c| strings(&c["in"])),
    );
    let mut bad = Vec::new();
    for argv in &argvs {
        let exec = shell::quote_argv(argv);
        let out = Command::new(bash)
            .arg("-c")
            .arg(format!("printf '%s\\0' {exec}"))
            .env("LC_ALL", "C.UTF-8")
            .output()
            .unwrap();
        let mut got: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split('\0')
            .map(String::from)
            .collect();
        got.pop();
        if !argv.is_empty() && got != *argv {
            bad.push(format!("{argv:?} -> {exec} -> {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "argv round-trip failures:\n{}",
        bad.join("\n")
    );
}

#[test]
fn exec_guess_is_one_quoted_word() {
    // Sol #2: a class is never shell syntax. Expected values are bash's own
    // `printf %q` of the lowercased class.
    assert_eq!(
        shell::Exec::guess("My Spaced App").as_str(),
        r"my\ spaced\ app"
    );
    assert_eq!(shell::Exec::guess(r"Back\slash").as_str(), r"back\\slash");
    assert_eq!(shell::Exec::guess("a;rm -rf ~").as_str(), r"a\;rm\ -rf\ ~");
}
