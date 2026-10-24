//! `propose`: capture the running windows and build the proposal the panel
//! relays to `relaunchd` (spec §8/§9, §12).
//!
//! A Save is a snapshot: the proposal is exactly the open windows that may be
//! signed, one pin per command per window class per workspace (windows that
//! match in all three are one pin: nothing tells them apart). A window keeps
//! the id of the pin it had ([`correlate`]), so saving an unchanged layout
//! proposes the base unchanged. A kept pin keeps a command
//! the user typed (`overrides-table`), which capture never replaces, and that
//! command's label; everything else, labels included, is what capture
//! observes now. A pin is enabled when its window passed the §9 trust gate;
//! an untrusted window's capture is proposed disabled.
//!
//! The engine assembles the proposal; the panel forwards `proposalJson` as an
//! opaque string (INV-9). Nothing is written here: each pin's launcher is
//! hashed in memory, and written only once the change is signed
//! (`install-signed`).

use std::collections::{BTreeMap, BTreeSet};

use std::path::Path;

use relaunch_core::{Entry, ExecSource, Proposal, SignedConfig, MAX_DOCUMENT_BYTES, MAX_ENTRIES};
use relaunch_sys::paths::CONFIG_FILE;
use serde_json::{json, Map, Value};

use crate::compositor::{self, Window};
use crate::config::{classify, Kind};
use crate::env::Env;
use crate::index::{DesktopIndex, Launcher};
use crate::launcher;
use crate::resolve::{capture_label, capture_resolve, Capture};

/// The pre-launch delay a first Save signs.
pub const DEFAULT_PRE_LAUNCH_MS: u32 = 0;
/// The post-launch delay a first Save signs.
pub const DEFAULT_POST_LAUNCH_MS: u32 = 500;

/// The document a proposal is built on.
#[derive(Debug)]
pub struct Base {
    /// The signed `config.json`, verbatim, for `relaunchd` to verify; empty
    /// when there is none (first-run adopt, spec §11).
    pub text: String,
    /// The base's pre-launch delay.
    pub pre_launch_ms: u32,
    /// The base's post-launch delay.
    pub post_launch_ms: u32,
    /// The base's entries.
    pub entries: Vec<Entry>,
}

impl Default for Base {
    fn default() -> Self {
        Self {
            text: String::new(),
            pre_launch_ms: DEFAULT_PRE_LAUNCH_MS,
            post_launch_ms: DEFAULT_POST_LAUNCH_MS,
            entries: Vec::new(),
        }
    }
}

/// Read the base from `config.json`: a signed envelope is the base; a legacy
/// (pre-signing) file, or one signed under an older schema, is no base — the
/// next authorization replaces it; no file is an empty base.
///
/// # Errors
///
/// A message if the file cannot be read or is neither a strict signed envelope
/// nor a legacy or superseded config — a malformed document is never silently
/// replaced by an empty base.
pub fn read_base(config_dir: &Path) -> Result<Base, String> {
    let path = config_dir.join(CONFIG_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Base::default()),
        _ => {}
    }
    let bytes = relaunch_sys::fs::read_regular_capped(&path, MAX_DOCUMENT_BYTES)?;
    match classify(&bytes) {
        Kind::Signed(cfg) => Ok(Base {
            text: String::from_utf8(bytes).map_err(|_| "config.json is not UTF-8".to_string())?,
            pre_launch_ms: cfg.signed.pre_launch_ms,
            post_launch_ms: cfg.signed.post_launch_ms,
            entries: cfg.signed.entries,
        }),
        Kind::Legacy | Kind::Superseded(_) => Ok(Base::default()),
        Kind::Invalid(e) => Err(format!("config.json is not a valid signed document: {e}")),
    }
}

/// Which capture the panel asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveMode {
    /// "Capture open windows": the boot list becomes exactly the open
    /// windows; a pin whose app is not open is dropped (and named, so the
    /// panel can ask first).
    Replace,
    /// "Add open windows": the open windows are added or refreshed and every
    /// other pin is kept.
    Merge,
}

/// The `propose --json` body: `{ok, base, preLaunchMs, postLaunchMs, entries,
/// candidates, dropping, merged, proposalJson}`. `entries` are the base's
/// (display); `dropping` names each pin the proposal would remove; `merged`
/// each open window left out as an identical duplicate of one proposed;
/// `proposalJson` is the proposal as text, which the panel forwards unparsed.
///
/// # Errors
///
/// A message if the base or the window list cannot be read, or the proposal
/// would not validate.
pub fn run(env: &Env, mode: SaveMode) -> Result<Value, String> {
    let base = read_base(&env.config_dir)?;
    // Collect launchers no signed pin names (a dropped pin's, or one left by an
    // abandoned install). The signed set itself is untouched.
    let live: BTreeSet<String> = base.entries.iter().map(|e| e.class.clone()).collect();
    launcher::reconcile(&env.config_dir, &live).map_err(|e| format!("launcher dir: {e}"))?;
    let windows = compositor::clients(env)?;
    let index = DesktopIndex::build(&env.app_dirs, Launcher::detect(&env.path));
    let observed = windows
        .iter()
        .filter(|w| w.is_resolvable())
        .map(|window| {
            let capture = capture_resolve(
                &env.proc,
                &index,
                &window.class,
                window.host_pid(),
                window.xwayland,
                window.shares_process(&windows),
            );
            let window = window.clone();
            let label = capture_label(&index, &capture);
            Observed {
                window,
                capture,
                label,
            }
        })
        .collect();
    let mut built = build(&env.config_dir, &base, observed)?;
    if mode == SaveMode::Merge {
        built.proposal = merge(&base, built.proposal);
    }
    let mut reply = commit_reply(&base, &built.proposal)?;
    reply.insert(
        "dropping".into(),
        json!(dropping(&base.entries, &built.proposal)),
    );
    reply.insert("merged".into(), json!(merged(&built.candidates)));
    reply.insert("preLaunchMs".into(), json!(base.pre_launch_ms));
    reply.insert("postLaunchMs".into(), json!(base.post_launch_ms));
    reply.insert("entries".into(), json!(base.entries));
    reply.insert("candidates".into(), json!(built.candidates));
    Ok(Value::Object(reply))
}

/// The panel's reply for an expanding change (`relaunchctl commit`): `{ok,
/// kind: "commit", base, proposalJson}`. The proposal is validated as the
/// document it would become, and travels as text the panel forwards unparsed.
///
/// # Errors
///
/// A message if the proposal would not validate.
pub fn commit_reply(base: &Base, proposal: &Proposal) -> Result<Map<String, Value>, String> {
    SignedConfig::from_proposal(0, "", 0, proposal.clone())
        .map_err(|e| format!("the proposal would not validate: {e:?}"))?;
    let text = serde_json::to_string(proposal).map_err(|e| format!("proposal: {e}"))?;
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    m.insert("kind".into(), json!("commit"));
    m.insert("base".into(), json!(base.text));
    m.insert("proposalJson".into(), json!(text));
    Ok(m)
}

/// `captured` laid over the base: each captured pin takes its id's place, and
/// every other pin of the base is kept as it is.
#[must_use]
pub fn merge(base: &Base, captured: Proposal) -> Proposal {
    let mut by_id: BTreeMap<String, Entry> = base
        .entries
        .iter()
        .map(|e| (e.class.clone(), e.clone()))
        .collect();
    for e in captured.entries {
        by_id.insert(e.class.clone(), e);
    }
    Proposal {
        entries: by_id.into_values().collect(),
        ..captured
    }
}

/// The pins of `base` that `proposal` would remove, as the panel names them:
/// `{label, workspace}`, in workspace order.
#[must_use]
pub fn dropping(base: &[Entry], proposal: &Proposal) -> Vec<Value> {
    let kept: BTreeSet<&str> = proposal.entries.iter().map(|e| e.class.as_str()).collect();
    let mut gone: Vec<&Entry> = base
        .iter()
        .filter(|e| !kept.contains(e.class.as_str()))
        .collect();
    gone.sort_by(|a, b| (a.workspace, &a.class).cmp(&(b.workspace, &b.class)));
    gone.iter()
        .map(|e| {
            let label = if e.label.is_empty() {
                &e.window_class
            } else {
                &e.label
            };
            json!({ "label": label, "workspace": e.workspace })
        })
        .collect()
}

/// The open windows left out as identical duplicates (same class, workspace
/// and command as a window that is proposed), as the panel names them:
/// `{label, workspace}`, in compositor order.
#[must_use]
pub fn merged(candidates: &[Value]) -> Vec<Value> {
    candidates
        .iter()
        .filter(|c| c["reason"] == DUPLICATE_WINDOW)
        .map(|c| json!({ "label": c["label"], "workspace": c["workspace"] }))
        .collect()
}

/// The candidate reason of a window that is an identical duplicate.
const DUPLICATE_WINDOW: &str = "duplicate-window";

/// One window's capture, with the label a new pin would get.
pub struct Observed {
    /// The window.
    pub window: Window,
    /// Its authorization-gated capture.
    pub capture: Capture,
    /// The label for a new pin.
    pub label: String,
}

/// What `propose` emits besides the base.
#[derive(Debug)]
pub struct Built {
    /// The proposal to relay, entries in canonical order.
    pub proposal: Proposal,
    /// One display row per window, in compositor order.
    pub candidates: Vec<Value>,
}

/// Where a pin's window is: its window class and its workspace. Several pins
/// may share one place, each running a different command.
pub type Place = (String, u32);

/// The stored pin each capture continues, if any, index for index with
/// `wanted` (each capture's place and command). A stored pin is continued at
/// most once. Per place:
///
/// - one stored pin and one capture: they are the same window, whatever its
///   command now (a command the user typed never equals the captured one, and
///   a changed command shows as changed rather than as a new pin);
/// - otherwise a capture continues the stored pin with exactly its command,
///   and every other capture is a new pin. A typed command on a place several
///   windows share continues only a window running exactly it.
#[must_use]
pub fn correlate<'a>(stored: &'a [Entry], wanted: &[(Place, &str)]) -> Vec<Option<&'a Entry>> {
    let mut at: BTreeMap<Place, Vec<&Entry>> = BTreeMap::new();
    for e in stored {
        at.entry((e.window_class.clone(), e.workspace))
            .or_default()
            .push(e);
    }
    let mut sharing: BTreeMap<&Place, usize> = BTreeMap::new();
    for (place, _) in wanted {
        *sharing.entry(place).or_default() += 1;
    }
    let mut used: BTreeSet<&str> = BTreeSet::new();
    wanted
        .iter()
        .map(|(place, exec)| {
            let pins = at.get(place).map_or(&[][..], Vec::as_slice);
            let found = match pins {
                [only] if sharing.get(place) == Some(&1) => Some(*only),
                _ => pins
                    .iter()
                    .copied()
                    .find(|e| e.exec == *exec && !used.contains(e.class.as_str())),
            };
            if let Some(e) = found {
                used.insert(e.class.as_str());
            }
            found
        })
        .collect()
}

/// A new pin's id for a window at `place`: one no pin in `taken` has (a
/// fixtures build derives ids from the place, so a second new pin there takes
/// the next derivation). The id is added to `taken`.
pub(crate) fn fresh_id(taken: &mut BTreeSet<String>, place: &Place) -> String {
    let mut nth = 0u32;
    loop {
        let id = launcher::new_id(&place.0, place.1, nth);
        if taken.insert(id.clone()) {
            return id;
        }
        nth = nth.saturating_add(1);
    }
}

/// The pin a proposable capture becomes, given the pin its window had
/// ([`correlate`]), if any, whose id it keeps; a new pin takes `new_id`.
///
/// # Errors
///
/// A message if its launcher cannot be built.
pub(crate) fn pin(
    config_dir: &Path,
    stored: Option<&Entry>,
    new_id: impl FnOnce() -> String,
    o: &Observed,
    source: ExecSource,
    workspace: u32,
) -> Result<Entry, String> {
    let id = stored.map_or_else(new_id, |b| b.class.clone());
    let float = o.window.floating.or_else(|| stored.and_then(|b| b.float));
    // A label is not a user field: recapture recomputes it, except for a
    // command the user typed, whose label describes that command.
    let label = stored
        .filter(|b| b.exec_source == ExecSource::OverridesTable)
        .map(|b| b.label.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| o.label.clone());
    let (exec, exec_source, provenance, cwd) = match stored {
        Some(b) if b.exec_source == ExecSource::OverridesTable => {
            (b.exec.clone(), b.exec_source, b.provenance, b.cwd.clone())
        }
        _ => (
            o.capture.exec.clone(),
            source,
            o.capture.provenance,
            o.capture.cwd.clone(),
        ),
    };
    let launcher = launcher::binding(config_dir, &id, &label, &exec, cwd.as_deref())?;
    Ok(Entry {
        class: id,
        window_class: o.capture.class.clone(),
        workspace,
        float,
        label,
        exec,
        exec_source,
        provenance,
        launcher,
        cwd,
        enabled: o.capture.trusted_window,
    })
}

/// Build the proposal: the base's delays, and one pin per proposable window.
///
/// # Errors
///
/// A message if the resulting document would not validate.
pub fn build(config_dir: &Path, base: &Base, observed: Vec<Observed>) -> Result<Built, String> {
    // Of several proposable windows with one class, workspace and command, the
    // first in compositor order is the pin's; the rest are its duplicates.
    let mut chosen: BTreeMap<(Place, String), usize> = BTreeMap::new();
    for (i, o) in observed.iter().enumerate() {
        if let (Some(_), Ok(ws)) = (o.capture.source, u32::try_from(o.window.workspace)) {
            if ws >= 1 {
                chosen
                    .entry(((o.capture.class.clone(), ws), o.capture.exec.clone()))
                    .or_insert(i);
            }
        }
    }
    // Correlated in compositor order, so of several new pins at one place the
    // first window's is minted first.
    let mut order: Vec<(&(Place, String), usize)> = chosen.iter().map(|(k, &i)| (k, i)).collect();
    order.sort_by_key(|&(_, i)| i);
    let wanted: Vec<(Place, &str)> = order
        .iter()
        .map(|((p, x), _)| (p.clone(), x.as_str()))
        .collect();
    let continues: BTreeMap<usize, Option<&Entry>> = order
        .iter()
        .map(|&(_, i)| i)
        .zip(correlate(&base.entries, &wanted))
        .collect();
    let mut taken: BTreeSet<String> = base.entries.iter().map(|e| e.class.clone()).collect();
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut candidates = Vec::with_capacity(observed.len());
    for (i, o) in observed.into_iter().enumerate() {
        let mut reason = None;
        let mut proposed: Option<Entry> = None;
        if let Some(source) = o.capture.source {
            match u32::try_from(o.window.workspace).ok().filter(|&w| w >= 1) {
                None => reason = Some("special-workspace".to_owned()),
                Some(ws) => {
                    let place = (o.capture.class.clone(), ws);
                    if let Some(stored) = continues.get(&i).filter(|_| entries.len() < MAX_ENTRIES)
                    {
                        let new_id = || fresh_id(&mut taken, &place);
                        match pin(config_dir, *stored, new_id, &o, source, ws)
                            .and_then(|e| e.validate().map(|()| e).map_err(|e| format!("{e:?}")))
                        {
                            Ok(e) => proposed = Some(e),
                            Err(e) => reason = Some(format!("invalid: {e}")),
                        }
                    } else if continues.contains_key(&i) {
                        reason = Some("entry-limit".to_owned());
                    } else {
                        reason = Some(DUPLICATE_WINDOW.to_owned());
                    }
                }
            }
        }
        let mut row = o.capture.to_json();
        if let Some(map) = row.as_object_mut() {
            map.insert("addr".into(), json!(o.window.address));
            map.insert("workspace".into(), json!(o.window.workspace));
            map.insert("floating".into(), json!(o.window.floating));
            map.insert(
                "label".into(),
                json!(proposed.as_ref().map_or(&o.label, |e| &e.label)),
            );
            map.insert(
                "enabled".into(),
                json!(proposed.as_ref().is_some_and(|e| e.enabled)),
            );
            map.insert("proposed".into(), json!(proposed.is_some()));
            if let Some(r) = &reason {
                map.insert("reason".into(), json!(r));
            }
        }
        candidates.push(row);
        if let Some(entry) = proposed {
            entries.insert(entry.class.clone(), entry);
        }
    }
    let proposal = Proposal {
        pre_launch_ms: base.pre_launch_ms,
        post_launch_ms: base.post_launch_ms,
        entries: entries.into_values().collect(),
    };
    SignedConfig::from_proposal(0, "", 0, proposal.clone())
        .map_err(|e| format!("propose: the proposal would not validate: {e:?}"))?;
    Ok(Built {
        proposal,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use relaunch_core::Provenance;

    const DIR: &str = "/home/u/.config/omarchy-relaunch";

    fn dir() -> &'static Path {
        Path::new(DIR)
    }

    fn window(class: &str, workspace: i64, floating: Option<bool>) -> Window {
        Window {
            address: format!("0x{workspace}"),
            pid: Some(1),
            class: class.to_owned(),
            workspace,
            floating,
            xwayland: Some(false),
            monitor: None,
            title: String::new(),
        }
    }

    fn host(class: &str, cmd: &str) -> Capture {
        let argv = vec![cmd.to_owned()];
        Capture {
            class: class.to_owned(),
            exec: relaunch_desktop::canonical_exec_string(&argv),
            argv,
            source: Some(ExecSource::Cmdline),
            provenance: Provenance::Host,
            desktop: None,
            cwd: None,
            hosted: None,
            trusted_window: true,
        }
    }

    fn untrusted(class: &str) -> Capture {
        Capture {
            source: None,
            provenance: Provenance::None,
            trusted_window: false,
            ..host(class, "x")
        }
    }

    /// An untrusted window's capture that is still eligible: a root-owned
    /// system launcher's command (spec §9).
    fn untrusted_system_desktop(class: &str) -> Capture {
        Capture {
            source: Some(ExecSource::DesktopFile),
            provenance: Provenance::None,
            desktop: Some(format!("/usr/share/applications/{class}.desktop")),
            ..untrusted(class)
        }
    }

    fn seen(class: &str, workspace: i64, floating: Option<bool>, capture: Capture) -> Observed {
        Observed {
            window: window(class, workspace, floating),
            capture,
            label: format!("{class} label"),
        }
    }

    fn base(entries: Vec<Entry>) -> Base {
        Base {
            entries,
            ..Base::default()
        }
    }

    fn go(b: &Base, observed: Vec<Observed>) -> Built {
        build(dir(), b, observed).unwrap()
    }

    fn pins<'a>(b: &'a Built, window_class: &str) -> Vec<&'a Entry> {
        b.proposal
            .entries
            .iter()
            .filter(|e| e.window_class == window_class)
            .collect()
    }

    fn one<'a>(b: &'a Built, window_class: &str) -> &'a Entry {
        let p = pins(b, window_class);
        assert_eq!(p.len(), 1, "{window_class}: {p:?}");
        p[0]
    }

    fn reason(b: &Built, i: usize) -> Option<&str> {
        b.candidates[i]["reason"].as_str()
    }

    #[test]
    fn saving_an_unchanged_layout_proposes_the_base_unchanged() {
        let layout = || {
            vec![
                seen("foot", 1, Some(false), host("foot", "foot")),
                seen("foot", 3, Some(false), host("foot", "foot")),
                seen("Signal", 2, None, host("Signal", "signal-desktop")),
            ]
        };
        let first = go(&base(vec![]), layout());
        let again = go(&base(first.proposal.entries.clone()), layout());
        assert_eq!(again.proposal, first.proposal);
    }

    #[test]
    fn a_save_is_exactly_the_open_windows_all_enabled() {
        let first = go(
            &base(vec![]),
            vec![
                seen("Gone", 1, None, host("Gone", "gone")),
                seen("Off", 2, None, host("Off", "off")),
            ],
        );
        let mut stored = first.proposal.entries.clone();
        for e in &mut stored {
            e.enabled = e.window_class != "Off";
        }
        let off_id = one(&first, "Off").class.clone();
        let b = go(
            &base(stored),
            vec![seen("Off", 2, None, host("Off", "off"))],
        );
        assert!(
            pins(&b, "Gone").is_empty(),
            "a closed window's pin is dropped"
        );
        let off = one(&b, "Off");
        assert!(
            off.enabled,
            "a disabled pin whose window is open comes back enabled"
        );
        assert_eq!(off.class, off_id, "and keeps its id");
    }

    #[test]
    fn identical_windows_at_one_place_are_one_pin() {
        let b = go(
            &base(vec![]),
            vec![
                seen("foot", 1, None, host("foot", "foot")),
                seen("foot", 4, None, host("foot", "foot")),
                seen("foot", 4, None, host("foot", "foot")),
                seen("foot", -98, None, host("foot", "foot")),
            ],
        );
        let mut ws: Vec<u32> = pins(&b, "foot").iter().map(|e| e.workspace).collect();
        ws.sort_unstable();
        assert_eq!(ws, [1, 4]);
        assert_eq!(reason(&b, 2), Some("duplicate-window"));
        assert_eq!(reason(&b, 3), Some("special-workspace"));
        let ids: BTreeSet<&str> = pins(&b, "foot").iter().map(|e| e.class.as_str()).collect();
        assert_eq!(ids.len(), 2, "each pin has its own id");
    }

    /// A capture of `argv` (a terminal hosting a command, say).
    fn runs(class: &str, argv: &[&str]) -> Capture {
        let argv: Vec<String> = argv.iter().map(|&a| a.to_owned()).collect();
        Capture {
            exec: relaunch_desktop::canonical_exec_string(&argv),
            argv,
            ..host(class, "x")
        }
    }

    /// Workspace 6 of the 2026-10-23 report: three foots, one bare.
    fn terminals() -> Vec<Observed> {
        vec![
            seen("foot", 6, Some(false), runs("foot", &["/usr/bin/foot"])),
            seen(
                "foot",
                6,
                Some(false),
                runs("foot", &["foot", "-e", "cliamp"]),
            ),
            seen(
                "foot",
                6,
                Some(false),
                runs("foot", &["foot", "-e", "htop"]),
            ),
        ]
    }

    fn execs(b: &Built, window_class: &str) -> BTreeSet<String> {
        pins(b, window_class)
            .iter()
            .map(|e| e.exec.clone())
            .collect()
    }

    #[test]
    fn windows_of_one_class_running_different_commands_are_each_a_pin() {
        let b = go(&base(vec![]), terminals());
        assert_eq!(
            execs(&b, "foot"),
            BTreeSet::from([
                "/usr/bin/foot".into(),
                "foot -e cliamp".into(),
                "foot -e htop".into()
            ])
        );
        let ids: BTreeSet<&str> = pins(&b, "foot").iter().map(|e| e.class.as_str()).collect();
        assert_eq!(ids.len(), 3, "each pin has its own id");
        assert!(
            b.candidates.iter().all(|c| c["proposed"] == true),
            "{:?}",
            b.candidates
        );
        assert!(merged(&b.candidates).is_empty());
    }

    #[test]
    fn an_identical_duplicate_is_merged_and_named() {
        let b = go(
            &base(vec![]),
            vec![
                seen("foot", 2, None, runs("foot", &["foot"])),
                seen("foot", 2, None, runs("foot", &["foot"])),
            ],
        );
        assert_eq!(pins(&b, "foot").len(), 1);
        assert_eq!(reason(&b, 1), Some(DUPLICATE_WINDOW));
        assert_eq!(
            merged(&b.candidates),
            [json!({"label": "foot label", "workspace": 2})]
        );
    }

    #[test]
    fn recapturing_several_terminals_unchanged_is_no_change() {
        let first = go(&base(vec![]), terminals());
        let mut reordered = terminals();
        reordered.reverse();
        let again = go(&base(first.proposal.entries.clone()), reordered);
        assert_eq!(
            again.proposal, first.proposal,
            "each window keeps its pin's id"
        );
    }

    #[test]
    fn at_a_shared_place_a_new_command_is_a_new_pin_and_the_rest_keep_theirs() {
        let first = go(&base(vec![]), terminals());
        let id_of = |b: &Built, exec: &str| {
            pins(b, "foot")
                .iter()
                .find(|e| e.exec == exec)
                .map(|e| e.class.clone())
        };
        let mut now = terminals();
        now[2] = seen("foot", 6, Some(false), runs("foot", &["foot", "-e", "btm"]));
        let b = go(&base(first.proposal.entries.clone()), now);
        for kept in ["/usr/bin/foot", "foot -e cliamp"] {
            assert_eq!(id_of(&b, kept), id_of(&first, kept), "{kept}");
        }
        let btm = id_of(&b, "foot -e btm").unwrap();
        assert!(
            first.proposal.entries.iter().all(|e| e.class != btm),
            "a new command at a shared place is a new pin"
        );
        assert!(
            id_of(&b, "foot -e htop").is_none(),
            "htop's window is closed"
        );
    }

    #[test]
    fn a_lone_windows_changed_command_keeps_its_pin() {
        let first = go(
            &base(vec![]),
            vec![seen(
                "foot",
                3,
                None,
                runs("foot", &["foot", "-e", "herdr"]),
            )],
        );
        let b = go(
            &base(first.proposal.entries.clone()),
            vec![seen("foot", 3, None, runs("foot", &["foot", "-e", "htop"]))],
        );
        let (was, now) = (one(&first, "foot"), one(&b, "foot"));
        assert_eq!(
            now.class, was.class,
            "same id, so the review shows it changed"
        );
        assert_eq!(now.exec, "foot -e htop");
    }

    #[test]
    fn a_typed_command_on_a_lone_window_survives_recapture_unchanged() {
        let first = go(
            &base(vec![]),
            vec![seen("foot", 3, None, runs("foot", &["foot"]))],
        );
        let mut typed = first.proposal.entries[0].clone();
        typed.exec_source = ExecSource::OverridesTable;
        typed.provenance = Provenance::None;
        typed.exec = "foot -e my-tui".into();
        typed.label = "Mine".into();
        typed.launcher =
            launcher::binding(dir(), &typed.class, &typed.label, &typed.exec, None).unwrap();
        let stored = base(vec![typed.clone()]);
        for window in [
            runs("foot", &["foot"]),
            runs("foot", &["foot", "-e", "something-else"]),
        ] {
            let b = go(&stored, vec![seen("foot", 3, None, window)]);
            assert_eq!(
                b.proposal.entries,
                [typed.clone()],
                "no change: the typed command is kept"
            );
        }
    }

    #[test]
    fn a_typed_command_at_a_shared_place_stays_with_the_window_running_it() {
        let first = go(&base(vec![]), terminals());
        let mut stored = first.proposal.entries.clone();
        let cliamp = stored
            .iter_mut()
            .find(|e| e.exec == "foot -e cliamp")
            .unwrap();
        cliamp.exec_source = ExecSource::OverridesTable;
        cliamp.exec = "foot -e cliamp --resume".into();
        let typed_id = cliamp.class.clone();
        let b = go(&base(stored.clone()), terminals());
        assert!(
            b.proposal.entries.iter().all(|e| e.class != typed_id),
            "no window runs the typed command, so its pin is not continued"
        );
        assert_eq!(pins(&b, "foot").len(), 3);
        let merged_in = merge(&base(stored), b.proposal);
        assert!(
            merged_in.entries.iter().any(|e| e.class == typed_id),
            "Add open windows keeps it"
        );
    }

    #[test]
    fn an_untrusted_windows_capture_is_proposed_disabled() {
        let b = go(
            &base(vec![]),
            vec![
                seen("Trusted", 1, None, host("Trusted", "t")),
                seen("Untrusted", 2, None, untrusted_system_desktop("Untrusted")),
                seen("Sandboxed", 3, None, untrusted("Sandboxed")),
            ],
        );
        assert!(one(&b, "Trusted").enabled);
        assert!(
            !one(&b, "Untrusted").enabled,
            "§9: an untrusted window stays disabled"
        );
        assert!(pins(&b, "Sandboxed").is_empty());
        assert_eq!(reason(&b, 2), Some("untrusted-window"));
    }

    #[test]
    fn a_kept_pin_keeps_a_typed_command_and_its_label() {
        let first = go(
            &base(vec![]),
            vec![seen("Typed", 4, None, host("Typed", "captured"))],
        );
        let mut typed = first.proposal.entries[0].clone();
        typed.exec_source = ExecSource::OverridesTable;
        typed.provenance = Provenance::None;
        typed.exec = "/usr/bin/bash -c my\\ --own\\ command".into();
        typed.label = "Mine".into();
        let b = go(
            &base(vec![typed.clone()]),
            vec![seen("Typed", 4, Some(true), host("Typed", "captured"))],
        );
        let e = one(&b, "Typed");
        assert_eq!(
            (
                e.exec.as_str(),
                e.exec_source,
                e.provenance,
                e.label.as_str(),
                e.class.as_str()
            ),
            (
                typed.exec.as_str(),
                ExecSource::OverridesTable,
                Provenance::None,
                "Mine",
                typed.class.as_str()
            )
        );
        assert_eq!(e.float, Some(true), "float follows the window");
    }

    #[test]
    fn each_pin_binds_its_own_launcher_by_the_bytes_it_will_hold() {
        let b = go(
            &base(vec![]),
            vec![seen("App", 1, None, host("App", "app"))],
        );
        let e = one(&b, "App");
        assert!(relaunch_core::is_launcher_id(&e.class), "{}", e.class);
        assert_eq!(
            e.launcher.path,
            format!("{DIR}/launchers/{}.desktop", e.class)
        );
        assert_eq!(
            e.launcher.sha256,
            launcher::sha256_hex(&launcher::launcher_bytes(e).unwrap())
        );
        assert_eq!(e.exec, "app");
    }

    #[test]
    fn new_pins_stop_at_the_entry_limit() {
        let observed = (0..=MAX_ENTRIES)
            .map(|i| {
                let c = format!("c{i:04}");
                seen(&c, 1, None, host(&c, "x"))
            })
            .collect();
        let b = go(&base(vec![]), observed);
        assert_eq!(b.proposal.entries.len(), MAX_ENTRIES);
        assert_eq!(reason(&b, MAX_ENTRIES), Some("entry-limit"));
    }

    #[test]
    fn an_entry_that_would_not_validate_is_shown_not_proposed() {
        let b = go(
            &base(vec![]),
            vec![
                seen("Bad\u{202e}Class", 1, None, host("Bad\u{202e}Class", "x")),
                seen("Good", 1, None, host("Good", "x")),
            ],
        );
        assert_eq!(pins(&b, "Good").len(), 1);
        assert!(reason(&b, 0).is_some_and(|r| r.starts_with("invalid:")));
        assert_eq!(b.proposal.entries.len(), 1);
    }

    #[test]
    fn window_class_bytes_survive_the_whole_build() {
        let classes = [
            "__proto__",
            "constructor",
            "Back\\slash",
            "a\\\\b",
            "org.x.Y",
        ];
        let b = go(
            &base(vec![]),
            classes
                .iter()
                .map(|c| seen(c, 1, None, host(c, "x")))
                .collect(),
        );
        let wire: Proposal =
            serde_json::from_str(&serde_json::to_string(&b.proposal).unwrap()).unwrap();
        let mut got: Vec<&str> = wire
            .entries
            .iter()
            .map(|e| e.window_class.as_str())
            .collect();
        let mut want = classes.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn the_base_delays_are_carried() {
        let mut b0 = base(vec![]);
        b0.pre_launch_ms = 7;
        b0.post_launch_ms = 900;
        let b = go(&b0, vec![seen("A", 1, None, host("A", "a"))]);
        assert_eq!(
            (b.proposal.pre_launch_ms, b.proposal.post_launch_ms),
            (7, 900)
        );
        let fresh = go(&base(vec![]), vec![]);
        assert_eq!(
            (fresh.proposal.pre_launch_ms, fresh.proposal.post_launch_ms),
            (DEFAULT_PRE_LAUNCH_MS, DEFAULT_POST_LAUNCH_MS)
        );
    }
}
