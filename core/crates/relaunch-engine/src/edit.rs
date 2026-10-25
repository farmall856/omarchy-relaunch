//! The signed-world edits (D-P4). An expanding edit — `import` (add one pin,
//! or replace the pin a class already has on that workspace, with its
//! captured command) and `set-exec` (a command the user typed) —
//! becomes a proposal the panel relays to `relaunchctl commit`, reviewed and
//! authorized like a capture. A reducing edit — `drop` — becomes a request the
//! panel relays to `relaunchctl reduce`, which only ever stops launches. The
//! engine writes nothing: the signed document comes back through
//! `install-signed`.

use std::collections::BTreeMap;

use relaunch_core::{Entry, ExecSource, Proposal, Provenance};
use relaunch_desktop::canonical_exec_string;
use serde_json::{json, Value};

use crate::compositor::{self, Window};
use crate::env::Env;
use crate::index::{DesktopIndex, Launcher};
use crate::launcher;
use crate::propose::{commit_reply, correlate, fresh_id, pin, read_base, Base, Observed};
use crate::resolve::{capture_label, capture_resolve, display_label};

/// The base's entries with `entry` put in its id's place, in canonical order.
fn with_entry(base: &Base, entry: Entry) -> Proposal {
    let mut by_id: BTreeMap<String, Entry> = base
        .entries
        .iter()
        .map(|e| (e.class.clone(), e.clone()))
        .collect();
    by_id.insert(entry.class.clone(), entry);
    Proposal {
        pre_launch_ms: base.pre_launch_ms,
        post_launch_ms: base.post_launch_ms,
        entries: by_id.into_values().collect(),
    }
}

/// The shell a typed command runs under, by absolute path, so nothing earlier
/// on `PATH` can stand in for it.
const SHELL: &str = "/usr/bin/bash";

/// The running window a class names (ASCII case ignored).
fn window_for_class(windows: &[Window], class: &str) -> Option<Window> {
    windows
        .iter()
        .filter(|w| w.is_resolvable())
        .find(|w| w.class.eq_ignore_ascii_case(class))
        .cloned()
}

/// `import --class C --workspace N`: pin a window class to a workspace,
/// enabled, with the command its running window's capture yields (spec
/// §8/§9); without a window only a root-owned system launcher can name one.
/// The class's pin already on that workspace, if any, is replaced and keeps
/// what a Save keeps (its id, an `overrides-table` command, a label).
///
/// # Errors
///
/// A message if the base is unreadable, nothing verifiable names a command for
/// the class, or the result would not validate.
pub fn import(env: &Env, class: &str, workspace: u32) -> Result<Value, String> {
    if workspace < 1 {
        return Err("import requires --workspace N (N>=1)".into());
    }
    let base = read_base(&env.config_dir)?;
    let windows = compositor::clients(env).unwrap_or_default();
    let index = DesktopIndex::build(&env.app_dirs, Launcher::detect(&env.path));
    let window = window_for_class(&windows, class);
    let capture = match &window {
        Some(w) => capture_resolve(
            &env.proc,
            &index,
            &w.class,
            w.host_pid(),
            w.xwayland,
            w.shares_process(&windows),
        ),
        None => capture_resolve(&env.proc, &index, class, 0, Some(true), false),
    };
    let Some(source) = capture.source else {
        return Err(format!(
            "import: no verifiable launch command for {class}: an untrusted (XWayland) window is trusted only through a root-owned launcher. Install a .desktop for it in /usr/share/applications (one under ~/.local/share is not trusted for such a window)."
        ));
    };
    let label = capture_label(&index, &capture);
    let observed = Observed {
        window: window.unwrap_or_else(|| Window {
            address: String::new(),
            pid: None,
            class: capture.class.clone(),
            workspace: i64::from(workspace),
            floating: None,
            xwayland: None,
            monitor: None,
            title: String::new(),
        }),
        capture,
        label,
    };
    let place = (observed.capture.class.clone(), workspace);
    let stored = correlate(
        &base.entries,
        &[(place.clone(), observed.capture.exec.as_str())],
    )
    .pop()
    .flatten();
    let mut taken = base.entries.iter().map(|e| e.class.clone()).collect();
    let new_id = || fresh_id(&mut taken, &place);
    let entry = Entry {
        enabled: true,
        ..pin(
            &env.config_dir,
            stored,
            new_id,
            &observed,
            source,
            workspace,
        )?
    };
    entry.validate().map_err(|e| format!("import: {e:?}"))?;
    Ok(Value::Object(commit_reply(
        &base,
        &with_entry(&base, entry),
    )?))
}

/// `set-exec --class ID --exec-stdin`: give a stored pin a command the user
/// typed (`overrides-table`) — run directly when it is plain words
/// ([`crate::execcheck::plain_argv`]), else as `bash -c <command>` —
/// shown in the reviewed diff and never validated beyond the signed
/// document's structural rules. [`exec_text`] is its inverse.
///
/// # Errors
///
/// A message if the class has no stored entry, the command is empty, or the
/// result would not validate.
pub fn set_exec(env: &Env, class: &str, exec: &str) -> Result<Value, String> {
    if exec.is_empty() {
        return Err("set-exec requires a command on stdin (--exec-stdin)".into());
    }
    let base = read_base(&env.config_dir)?;
    let stored = base
        .entries
        .iter()
        .find(|e| e.class == class)
        .ok_or_else(|| format!("no relaunch entry for class: {class}"))?;
    let index = DesktopIndex::build(&env.app_dirs, Launcher::detect(&env.path));
    let label = if stored.label.is_empty() {
        display_label(&index, &stored.window_class, exec)
    } else {
        stored.label.clone()
    };
    let argv = crate::execcheck::plain_argv(exec)
        .unwrap_or_else(|| vec![SHELL.to_owned(), "-c".to_owned(), exec.to_owned()]);
    let exec = canonical_exec_string(&argv);
    let entry = Entry {
        launcher: launcher::binding(&env.config_dir, &stored.class, &label, &exec, None)?,
        cwd: None,
        exec,
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        label,
        ..stored.clone()
    };
    entry.validate().map_err(|e| format!("set-exec: {e:?}"))?;
    Ok(Value::Object(commit_reply(
        &base,
        &with_entry(&base, entry),
    )?))
}

/// `set-delays --pre MS --post MS`: the signed waits before and after each
/// launch within a same-app group. The pins are kept as they are; the change
/// is reviewed and authorized like any other.
///
/// # Errors
///
/// A message if the base is unreadable or a delay is above the signed bound.
pub fn set_delays(env: &Env, pre: u32, post: u32) -> Result<Value, String> {
    if pre > relaunch_core::MAX_DELAY_MS || post > relaunch_core::MAX_DELAY_MS {
        return Err(format!(
            "set-delays: a delay is at most {} ms",
            relaunch_core::MAX_DELAY_MS
        ));
    }
    let base = read_base(&env.config_dir)?;
    let proposal = Proposal {
        pre_launch_ms: pre,
        post_launch_ms: post,
        entries: base.entries.clone(),
    };
    Ok(Value::Object(commit_reply(&base, &proposal)?))
}

/// A signed `exec` as the command line `set-exec` reads back to it: a typed
/// `bash -c TEXT` is TEXT, any other argv its words, quoted.
#[must_use]
pub fn exec_text(exec: &str) -> String {
    match relaunch_desktop::split_canonical_exec(exec).as_slice() {
        [shell, flag, text] if shell == SHELL && flag == "-c" => text.clone(),
        argv => crate::shell::quote_argv(argv),
    }
}

/// `drop --class C`: the panel's reply for a reducing change (`relaunchctl
/// reduce`): `{ok, kind: "reduce", base, reduceJson}`.
///
/// # Errors
///
/// A message if there is no signed entry for the class.
pub fn drop(env: &Env, class: &str) -> Result<Value, String> {
    let base = read_base(&env.config_dir)?;
    if !base.entries.iter().any(|e| e.class == class) {
        return Err(format!("no relaunch entry for class: {class}"));
    }
    let request = json!({ "base": base.text, "ops": [{ "op": "drop", "class": class }] });
    Ok(
        json!({ "ok": true, "kind": "reduce", "base": base.text, "reduceJson": request.to_string() }),
    )
}
