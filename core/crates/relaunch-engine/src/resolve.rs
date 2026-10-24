//! Resolving a window to a launch command, the §9 trust gate, and labels.
//!
//! Two resolvers:
//! - [`resolve_launch`]: the legacy order (overrides table, `.desktop`,
//!   cmdline, guess) behind the bash engine's display and labels.
//! - [`capture_resolve`]: the authorization-gated capture behind `propose`,
//!   which also decides provenance and eligibility (spec §8/§9). It weighs two
//!   candidates and knows no application by name: the `.desktop` that claims
//!   the window's class, and the window process's own command line.
//!
//! Every argv comes through the one host walker (INV-1); a legacy command is
//! an [`Exec`] (INV-8): a class guess is one quoted word, never shell syntax.

use std::path::Path;

use relaunch_core::{ExecSource, Provenance};
use relaunch_desktop::canonical_exec_string;
use serde::de::{MapAccess, Visitor};
use serde::Deserializer;
use serde_json::{json, Value};

use crate::argv::{basename, leaf_for_exec, program};
use crate::host::{Foreground, Proc};
use crate::index::DesktopIndex;
use crate::shell::{split_stored, Exec};

/// §9 window trust: a native Wayland surface (not `XWayland`) whose process is
/// on the host. An absent `xwayland` field is `XWayland` — fail closed — except
/// in a fixture tree, whose fixtures predate the field.
#[must_use]
pub fn window_trusted(proc: &Proc, xwayland: Option<bool>, pid: u32) -> bool {
    match xwayland {
        Some(true) => false,
        Some(false) => proc.is_host(pid),
        None => proc.is_fixture() && proc.is_host(pid),
    }
}

/// A subprocess of a multi-process program (any `--type=` argument), never
/// the command to relaunch: the walk goes on to its parent.
fn is_helper_argv(argv: &[String]) -> bool {
    argv.iter().any(|a| a.starts_with("--type="))
}

/// A command line whose separators were overwritten: one field holding
/// spaces that is not itself a program. Chromium and Electron rewrite their
/// own command line this way, and its words cannot be told apart again, so it
/// is never split back into an argv.
fn is_title_smashed(argv: &[String]) -> bool {
    match argv {
        [only] => only.contains(' ') && !relaunch_sys::fs::is_executable_file(Path::new(only)),
        _ => false,
    }
}

/// The command a window's process (or nearest non-helper host ancestor) was
/// started with.
#[must_use]
pub fn cmdline_argv(proc: &Proc, pid: u32) -> Option<Vec<String>> {
    proc.walk(pid, |argv| (!is_helper_argv(argv)).then(|| argv.to_vec()))
}

/// [`cmdline_argv`] as a shell command, one argv word each.
#[must_use]
pub fn cmdline_exec(proc: &Proc, pid: u32) -> Option<Exec> {
    cmdline_argv(proc, pid).map(|argv| Exec::from_argv(&argv))
}

/// The user's `overrides.json` command for a class: the first key (in file
/// order) equal to the class ignoring ASCII case. Read-only; a missing or
/// unparseable table has no overrides.
#[must_use]
pub fn override_exec(config_dir: &Path, class: &str) -> Option<Exec> {
    let bytes = relaunch_sys::fs::read_regular_capped(
        &config_dir.join("overrides.json"),
        relaunch_sys::fs::MAX_FILE_BYTES,
    )
    .ok()?;
    let pairs = ordered_pairs(&bytes)?;
    let (_, value) = pairs
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(class))?;
    match value {
        Value::Null | Value::Bool(false) => None,
        Value::String(s) => (!s.is_empty()).then(|| Exec::stored(s)),
        other => Some(Exec::stored(other.to_string())),
    }
}

/// A JSON object's entries in document order (jq `to_entries`), without
/// switching `serde_json` to order-preserving maps workspace-wide.
fn ordered_pairs(bytes: &[u8]) -> Option<Vec<(String, Value)>> {
    struct Pairs;
    impl<'de> Visitor<'de> for Pairs {
        type Value = Vec<(String, Value)>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(entry) = map.next_entry::<String, Value>()? {
                out.push(entry);
            }
            Ok(out)
        }
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let pairs = de.deserialize_map(Pairs).ok()?;
    de.end().ok()?;
    Some(pairs)
}

/// Where a resolved command came from (the legacy sources).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A terminal host's hosted command.
    Terminal,
    /// The user's overrides table.
    OverridesTable,
    /// A `.desktop` launcher.
    DesktopFile,
    /// The window process's argv.
    Cmdline,
    /// The lowercased class.
    Guess,
}

impl Source {
    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::OverridesTable => "overrides-table",
            Self::DesktopFile => "desktop-file",
            Self::Cmdline => "cmdline",
            Self::Guess => "guess",
        }
    }
}

/// The legacy resolution order for a window.
#[must_use]
pub fn resolve_launch(
    proc: &Proc,
    index: &DesktopIndex,
    config_dir: &Path,
    class: &str,
    pid: u32,
) -> (Exec, Source) {
    if let Some(e) = override_exec(config_dir, class) {
        return (e, Source::OverridesTable);
    }
    if let Some(e) = index.exec_for_class(class) {
        return (Exec::stored(e.to_owned()), Source::DesktopFile);
    }
    if let Some(e) = cmdline_exec(proc, pid) {
        return (e, Source::Cmdline);
    }
    (Exec::guess(class), Source::Guess)
}

/// The system application directories. A `.desktop` file in one, owned by
/// root down from that directory, is the one source an untrusted window's
/// capture may take its command from (spec §9): nothing running as the user
/// can have written it.
const ROOT_APPLICATION_DIRS: [&str; 3] = [
    "/usr/share/applications",
    "/usr/local/share/applications",
    "/var/lib/flatpak/exports/share/applications",
];

/// Whether the `.desktop` file at `path` resolves into a system application
/// directory along a root-owned chain. A fixture build also trusts the
/// current user, whose fixture trees stand in for the system.
fn root_owned_application(path: &str) -> bool {
    let trusted =
        |uid: u32| uid == 0 || (cfg!(feature = "fixtures") && uid == relaunch_sys::current_uid());
    let Ok(resolved) = std::fs::canonicalize(path) else {
        return false;
    };
    ROOT_APPLICATION_DIRS.iter().any(|dir| {
        let dir = Path::new(dir);
        resolved.starts_with(dir)
            && relaunch_sys::fs::trusted_chain(&resolved, trusted, dir).is_ok()
    })
}

/// One window's authorization-gated capture (spec §8/§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    /// The window class the relaunched window will carry: the hosted
    /// command's class for a terminal window, otherwise the window's class,
    /// byte for byte.
    pub class: String,
    /// The launch command, one argument each.
    pub argv: Vec<String>,
    /// [`Capture::argv`] as the signed `exec` ([`canonical_exec_string`]).
    pub exec: String,
    /// Where the command came from; `None` is `unverified` (not eligible for
    /// signing).
    pub source: Option<ExecSource>,
    /// Host provenance, for an argv-derived command.
    pub provenance: Provenance,
    /// The `.desktop` file a desktop-file capture copied its `Exec` from.
    pub desktop: Option<String>,
    /// The directory the command runs in: the window process's own, when its
    /// command line is the one captured.
    pub cwd: Option<String>,
    /// The program a terminal window runs — named on its command line, or
    /// found in its foreground — when the capture is the terminal running it:
    /// what the pin is called.
    pub hosted: Option<String>,
    /// Whether the window passed the §9 trust gate (a native, host window).
    pub trusted_window: bool,
}

impl Capture {
    /// Whether the capture may enter a signed document.
    #[must_use]
    pub fn eligible(&self) -> bool {
        self.source.is_some()
    }

    /// The capture row: `class, exec, execSource, provenance, desktopFile?,
    /// eligible, reason?`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut row = json!({
            "class": self.class,
            "exec": self.exec,
            "execSource": self.source.map_or(json!("unverified"), |s| json!(s)),
            "provenance": self.provenance,
            "eligible": self.eligible(),
        });
        if let (Some(desktop), Some(map)) = (&self.desktop, row.as_object_mut()) {
            map.insert("desktopFile".into(), json!(desktop));
        }
        if let (Some(cwd), Some(map)) = (&self.cwd, row.as_object_mut()) {
            map.insert("cwd".into(), json!(cwd));
        }
        if let (false, Some(map)) = (self.eligible(), row.as_object_mut()) {
            let why = if self.trusted_window {
                "no-command"
            } else {
                "untrusted-window"
            };
            map.insert("reason".into(), json!(why));
        }
        row
    }
}

/// Authorization-gated capture of one window (spec §8/§9).
///
/// Two candidates, and no application is named:
/// - D, the `.desktop` that claims the class (`StartupWMClass`, desktop-file
///   id, web-app identity, `Name`), its `Exec` as argv — none for a
///   `Terminal=true` or `DBusActivatable` file, which is not a command;
/// - P, the window process's command line (past `--type=` subprocesses).
///
/// P is not the window's own command when it was title-smashed, when its
/// process also owns a window of another class (`shares_process`), or when
/// only a subprocess was found. Then D, if any. Otherwise P whole — flags and
/// all, with its working directory — unless D runs a different program, which
/// means the process is not what launches the window. Without either, the
/// capture is an unsigned guess: a guess is never a launch command.
///
/// Untrusted (§9): only a root-owned system `.desktop`; otherwise ineligible.
#[must_use]
pub fn capture_resolve(
    proc: &Proc,
    index: &DesktopIndex,
    class: &str,
    pid: u32,
    xwayland: Option<bool>,
    shares_process: bool,
) -> Capture {
    let trusted = window_trusted(proc, xwayland, pid);
    let capture = |argv: Vec<String>, source: Option<ExecSource>| Capture {
        class: class.to_owned(),
        exec: canonical_exec_string(&argv),
        argv,
        source,
        provenance: if source == Some(ExecSource::Cmdline) {
            Provenance::Host
        } else {
            Provenance::None
        },
        desktop: None,
        cwd: None,
        hosted: None,
        trusted_window: trusted,
    };
    let desktop = |root_only: bool| -> Option<Capture> {
        let path = index.launcher_for_class(class)?;
        if root_only && !root_owned_application(path) {
            return None;
        }
        let argv = relaunch_desktop::exec_argv_file(path)
            .ok()
            .filter(|a| !a.is_empty())?;
        Some(Capture {
            desktop: Some(path.to_owned()),
            ..capture(argv, Some(ExecSource::DesktopFile))
        })
    };
    let guess = || capture(vec![class.to_lowercase()], None);
    if !trusted {
        return desktop(true).unwrap_or_else(guess);
    }
    let own = (!shares_process)
        .then(|| {
            proc.walk_pid(pid, |at, argv| {
                (!is_helper_argv(argv)).then(|| (at, argv.to_vec()))
            })
        })
        .flatten()
        .filter(|(_, argv)| !is_title_smashed(argv));
    let d = desktop(false);
    if let Some((at, argv)) = own {
        if d.as_ref()
            .is_none_or(|d| program(&d.argv) == program(&argv))
        {
            let shown = terminal_shows(proc, index, at, &argv);
            if let Some(Shown::Command { pid, argv: hosted }) = shown {
                return Capture {
                    cwd: proc.cwd(pid),
                    hosted: program(&hosted).map(str::to_owned),
                    ..capture(
                        hosted_command(index, &argv, hosted),
                        Some(ExecSource::Cmdline),
                    )
                };
            }
            // A terminal works where its shell is, not where it was started.
            let cwd_of = match shown {
                Some(Shown::Shell { pid }) => pid,
                _ => at,
            };
            return Capture {
                cwd: proc.cwd(cwd_of),
                hosted: named_command(index, &argv),
                ..capture(argv, Some(ExecSource::Cmdline))
            };
        }
    }
    d.unwrap_or_else(guess)
}

/// The option a terminal emulator `argv` takes before a command to run, when
/// its program is one (by its `.desktop` `Categories`, never by name).
fn terminal_option<'a>(index: &'a DesktopIndex, argv: &[String]) -> Option<&'a str> {
    index.terminal_arg_exec(program(argv)?)
}

/// Whether a terminal's `argv` already names a command to run: its option is
/// there, or (a terminal taking no option) a word follows its program.
fn names_command(argv: &[String], option: &str) -> bool {
    if option.is_empty() {
        let after = argv
            .iter()
            .skip_while(|w| program(std::slice::from_ref(*w)).is_none())
            .skip(1);
        after.into_iter().any(|w| !w.starts_with('-'))
    } else {
        argv.iter().any(|w| w == option)
    }
}

/// The program a terminal emulator's `argv` names to run in it: the word
/// after its option, or (a terminal taking no option) after its program.
fn named_command(index: &DesktopIndex, argv: &[String]) -> Option<String> {
    let option = terminal_option(index, argv)?;
    let rest: Vec<String> = if option.is_empty() {
        argv.iter()
            .skip_while(|w| program(std::slice::from_ref(*w)).is_none())
            .skip(1)
            .skip_while(|w| w.starts_with('-'))
            .cloned()
            .collect()
    } else {
        argv.iter()
            .skip_while(|w| *w != option)
            .skip(1)
            .cloned()
            .collect()
    };
    program(&rest).map(str::to_owned)
}

/// What a terminal emulator window shows in its foreground.
enum Shown {
    /// Its own shell (pid): nothing was started in it, or its command line
    /// already names the command it runs.
    Shell { pid: u32 },
    /// A command started in it, whose leader is `pid`.
    Command { pid: u32, argv: Vec<String> },
}

/// For a terminal emulator window (`argv`, at pid `at`): its pty's
/// foreground. A command counts only when the terminal's own command line
/// names none and the command was not title-smashed. `None` when it is not a
/// terminal or the foreground cannot be read on the host.
fn terminal_shows(proc: &Proc, index: &DesktopIndex, at: u32, argv: &[String]) -> Option<Shown> {
    let option = terminal_option(index, argv)?;
    match proc.foreground(at)? {
        Foreground::Command { pid, argv: fg }
            if !names_command(argv, option) && !fg.is_empty() && !is_title_smashed(&fg) =>
        {
            Some(Shown::Command { pid, argv: fg })
        }
        Foreground::Shell { pid } | Foreground::Command { pid, .. } => Some(Shown::Shell { pid }),
    }
}

/// The terminal's whole command line, then its option, then the command it
/// was running.
fn hosted_command(index: &DesktopIndex, terminal: &[String], hosted: Vec<String>) -> Vec<String> {
    let mut out = terminal.to_vec();
    if let Some(option) = terminal_option(index, terminal).filter(|o| !o.is_empty()) {
        out.push(option.to_owned());
    }
    out.extend(hosted);
    out
}

/// The label a newly captured entry gets: its launcher's `Name`, else the
/// [`display_label`] tiers.
#[must_use]
pub fn capture_label(index: &DesktopIndex, capture: &Capture) -> String {
    if let Some(hosted) = capture.hosted.as_ref().filter(|h| !h.is_empty()) {
        return hosted.clone();
    }
    capture
        .desktop
        .as_deref()
        .or_else(|| index.launcher_for_class(&capture.class))
        .and_then(relaunch_desktop::name_from_file)
        .unwrap_or_else(|| display_label(index, &capture.class, &capture.exec))
}

/// The display label for an entry: a `gio launch` file's `Name`, then the
/// `Name` of a launcher whose shell- or wrapper-run leaf it runs, then the
/// leaf command, then the class. Cosmetic only; never a lookup key.
#[must_use]
pub fn display_label(index: &DesktopIndex, class: &str, exec: &str) -> String {
    if !exec.is_empty() {
        let argv = split_stored(exec);
        if let [cmd, sub, file, ..] = argv.as_slice() {
            if basename(cmd) == "gio" && sub == "launch" {
                if let Some(name) = relaunch_desktop::name_from_file(file) {
                    return name;
                }
            }
        }
        let leaf = leaf_for_exec(exec).filter(|l| !l.is_empty());
        if let Some(leaf) = leaf {
            if let Some(name) = index.name_for_leaf(&leaf).filter(|n| !n.is_empty()) {
                return name.to_owned();
            }
            return leaf;
        }
    }
    class.to_owned()
}

#[cfg(all(test, feature = "fixtures"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fixture tree: per-pid argv (`(pid, ppid, argv, cwd)`), and launchers.
    struct World(PathBuf);

    impl World {
        fn new(
            tag: &str,
            procs: &[(u32, u32, &[&str], Option<&str>)],
            apps: &[(&str, &str)],
        ) -> Self {
            let root =
                std::env::temp_dir().join(format!("relaunch-resolve-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("cmd")).unwrap();
            std::fs::create_dir_all(root.join("applications")).unwrap();
            for (pid, ppid, argv, cwd) in procs {
                let mut bytes = Vec::new();
                for a in *argv {
                    bytes.extend_from_slice(a.as_bytes());
                    bytes.push(0);
                }
                std::fs::write(root.join(format!("cmd/{pid}")), bytes).unwrap();
                std::fs::write(root.join(format!("cmd/{pid}.ppid")), format!("{ppid}\n")).unwrap();
                if let Some(c) = cwd {
                    std::fs::write(root.join(format!("cmd/{pid}.cwd")), format!("{c}\n")).unwrap();
                }
            }
            for (file, body) in apps {
                std::fs::write(
                    root.join("applications").join(file),
                    format!("[Desktop Entry]\nType=Application\n{body}\n"),
                )
                .unwrap();
            }
            World(root)
        }
        fn file(&self, name: &str, body: &str) {
            std::fs::write(self.0.join("cmd").join(name), body).unwrap();
        }
        fn capture(&self, class: &str, pid: u32, shares: bool) -> Capture {
            let proc = Proc::fixture(self.0.join("cmd"));
            let index = DesktopIndex::build(
                &[self.0.join("applications").to_string_lossy().into_owned()],
                crate::index::Launcher::None,
            );
            capture_resolve(&proc, &index, class, pid, Some(false), shares)
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn got(c: &Capture) -> (Option<ExecSource>, &str, Option<&str>) {
        (c.source, c.exec.as_str(), c.cwd.as_deref())
    }

    #[test]
    fn an_honest_command_line_is_kept_whole_with_its_directory() {
        let w = World::new(
            "honest",
            &[
                (10, 1, &["mpv", "--mute", "/v/a b.mkv"], Some("/home/u")),
                (
                    11,
                    1,
                    &["foot", "--app-id=TUI.float", "-e", "bash", "-c", "dua i /"],
                    Some("/"),
                ),
            ],
            &[
                (
                    "mpv.desktop",
                    "StartupWMClass=mpv\nExec=mpv --player-operation-mode=pseudo-gui -- %U",
                ),
                ("foot.desktop", "StartupWMClass=TUI.float\nExec=foot"),
            ],
        );
        assert_eq!(
            got(&w.capture("mpv", 10, false)),
            (
                Some(ExecSource::Cmdline),
                "mpv --mute /v/a\\ b.mkv",
                Some("/home/u")
            ),
            "same program as its launcher: the window's own command, flags and positionals"
        );
        assert_eq!(
            got(&w.capture("TUI.float", 11, false)),
            (
                Some(ExecSource::Cmdline),
                "foot --app-id=TUI.float -e bash -c dua\\ i\\ /",
                Some("/")
            )
        );
        let c = w.capture("Unknown", 10, false);
        assert_eq!(
            c.source,
            Some(ExecSource::Cmdline),
            "no launcher: its own command"
        );
        assert_eq!(c.provenance, Provenance::Host);
    }

    #[test]
    fn a_launcher_running_another_program_wins_without_the_processes_directory() {
        let w = World::new(
            "other-program",
            &[(
                10,
                1,
                &["/usr/lib/libreoffice/program/soffice.bin", "--calc"],
                Some("/home/u"),
            )],
            &[(
                "calc.desktop",
                "StartupWMClass=libreoffice-calc\nExec=libreoffice --calc %U",
            )],
        );
        assert_eq!(
            got(&w.capture("libreoffice-calc", 10, false)),
            (Some(ExecSource::DesktopFile), "libreoffice --calc", None)
        );
    }

    #[test]
    fn a_title_smashed_command_line_is_never_split_back() {
        let w = World::new(
            "smashed",
            &[
                (
                    10,
                    1,
                    &["/usr/lib/electron43/electron /usr/share/proton-mail/app.asar"],
                    Some("/"),
                ),
                (11, 1, &["/usr/bin/foot"], Some("/")),
            ],
            &[(
                "proton-mail.desktop",
                "StartupWMClass=proton-mail\nExec=proton-mail %U",
            )],
        );
        assert_eq!(
            got(&w.capture("proton-mail", 10, false)),
            (Some(ExecSource::DesktopFile), "proton-mail", None)
        );
        let c = w.capture("Nameless", 10, false);
        assert_eq!(c.source, None, "smashed and no launcher: an unsigned guess");
        assert_eq!(c.to_json()["reason"], "no-command");
        assert_eq!(
            got(&w.capture("foot", 11, false)),
            (Some(ExecSource::Cmdline), "/usr/bin/foot", Some("/")),
            "one field without a space is an honest one-word command"
        );
    }

    #[test]
    fn a_process_that_also_owns_another_classes_window_is_not_this_windows_command() {
        let w = World::new(
            "shared",
            &[(10, 1, &["/opt/browser/browser", "--profile-directory=Default"], Some("/"))],
            &[("pwa.desktop", "StartupWMClass=crx_x\nExec=/opt/browser/browser --profile-directory=Default --app-id=abcd")],
        );
        let shared = w.capture("browser-abcd-Default", 10, true);
        assert_eq!(
            got(&shared),
            (
                Some(ExecSource::DesktopFile),
                "/opt/browser/browser --profile-directory=Default --app-id=abcd",
                None
            ),
            "the launcher, matched by the generic app-id identity"
        );
        assert_eq!(w.capture("Nameless", 10, true).source, None);
    }

    #[test]
    fn a_subprocess_is_walked_past_to_its_parent() {
        let w = World::new(
            "helper",
            &[
                (20, 10, &["/opt/app/app", "--type=renderer"], None),
                (10, 1, &["/opt/app/app", "--x"], Some("/w")),
            ],
            &[],
        );
        assert_eq!(
            got(&w.capture("App", 20, false)),
            (Some(ExecSource::Cmdline), "/opt/app/app --x", Some("/w")),
            "any --type= value, and the parent's directory"
        );
    }

    const FOOT: (&str, &str) = (
        "foot.desktop",
        "Name=Foot\nExec=foot\nCategories=System;TerminalEmulator;\nX-TerminalArgExec=-e",
    );

    /// A terminal (pid 10, argv `term`) whose shell (pid 11) has `tpgid` in
    /// the foreground; pid 12 is a command started from that shell.
    fn terminal(tag: &str, term: &[&str], tpgid: u32, apps: &[(&str, &str)]) -> World {
        let w = World::new(
            tag,
            &[
                (10, 1, term, Some("/")),
                (11, 10, &["/usr/bin/bash"], Some("/home/u")),
                (
                    12,
                    11,
                    &["herdr", "--session", "main"],
                    Some("/home/u/work"),
                ),
            ],
            apps,
        );
        w.file("11.tpgid", &format!("{tpgid}\n"));
        w
    }

    #[test]
    fn a_bare_terminal_running_a_command_is_captured_running_it() {
        let w = terminal("fg-command", &["/usr/bin/foot"], 12, &[FOOT]);
        let c = w.capture("foot", 10, false);
        assert_eq!(
            got(&c),
            (
                Some(ExecSource::Cmdline),
                "/usr/bin/foot -e herdr --session main",
                Some("/home/u/work")
            ),
            "the terminal's own command, its option, the foreground command whole, in its directory"
        );
        assert_eq!(c.provenance, Provenance::Host);
        assert_eq!(c.class, "foot", "the window keeps the terminal's class");
        let index = DesktopIndex::build(
            &[w.0.join("applications").to_string_lossy().into_owned()],
            crate::index::Launcher::None,
        );
        assert_eq!(capture_label(&index, &c), "herdr", "named for what it runs");
    }

    #[test]
    fn a_terminal_showing_only_its_shell_is_the_bare_terminal_with_its_options() {
        let w = terminal("fg-shell", &["foot", "--app-id=TUI.float"], 11, &[FOOT]);
        let c = w.capture("TUI.float", 10, false);
        assert_eq!(
            got(&c),
            (
                Some(ExecSource::Cmdline),
                "foot --app-id=TUI.float",
                Some("/home/u")
            ),
            "nothing started in it: the terminal as it is, app id kept, where its shell is"
        );
        assert_eq!(c.hosted, None);
        let w = terminal("fg-appid", &["foot", "--app-id=TUI.float"], 12, &[FOOT]);
        assert_eq!(
            w.capture("TUI.float", 10, false).exec,
            "foot --app-id=TUI.float -e herdr --session main",
            "a command started in it keeps the terminal's options too"
        );
    }

    #[test]
    fn a_bare_terminal_comes_back_where_its_shell_is_not_where_it_was_started() {
        // The case on hardware: foot started in `/`, its shell cd'd to a project.
        let procs: &[(u32, u32, &[&str], Option<&str>)] = &[
            (4941, 1, &["/usr/bin/foot"], Some("/")),
            (
                5015,
                4941,
                &["/usr/bin/bash"],
                Some("/home/laytonf/Projects"),
            ),
        ];
        let w = World::new("fg-shell-cwd", procs, &[FOOT]);
        w.file("5015.tpgid", "5015\n");
        assert_eq!(
            got(&w.capture("foot", 4941, false)),
            (
                Some(ExecSource::Cmdline),
                "/usr/bin/foot",
                Some("/home/laytonf/Projects")
            )
        );
        w.file("5015.cwd", "/\n");
        assert_eq!(
            w.capture("foot", 4941, false).cwd.as_deref(),
            Some("/"),
            "a shell that is in / is captured in / (no other default)"
        );
        let plain = World::new(
            "fg-shell-cwd-plain",
            procs,
            &[("foot.desktop", "Exec=foot\nCategories=System;")],
        );
        plain.file("5015.tpgid", "5015\n");
        assert_eq!(
            plain.capture("foot", 4941, false).cwd.as_deref(),
            Some("/"),
            "not a terminal by its launcher: its own process's directory"
        );
    }

    #[test]
    fn a_terminal_that_already_names_its_command_is_its_own_command_line() {
        let w = terminal("fg-named", &["foot", "-e", "htop"], 12, &[FOOT]);
        let c = w.capture("foot", 10, false);
        assert_eq!(
            got(&c),
            (
                Some(ExecSource::Cmdline),
                "foot -e htop",
                Some("/home/u/work")
            ),
            "never a second option or a second command; in its foreground's directory"
        );
        assert_eq!(
            c.hosted.as_deref(),
            Some("htop"),
            "named for the command it names"
        );
        let w = terminal(
            "fg-named-env",
            &["foot", "--app-id=herdr", "-e", "env", "A=1", "herdr"],
            11,
            &[FOOT],
        );
        assert_eq!(
            w.capture("herdr", 10, false).hosted.as_deref(),
            Some("herdr")
        );
    }

    #[test]
    fn only_a_terminal_by_its_launchers_categories_is_looked_inside() {
        let not_terminal = ("foot.desktop", "Exec=foot\nCategories=System;");
        let w = terminal("fg-not-term", &["foot"], 12, &[not_terminal]);
        assert_eq!(w.capture("foot", 10, false).exec, "foot");
        let w = terminal(
            "fg-not-term-named",
            &["foot", "-e", "htop"],
            12,
            &[not_terminal],
        );
        assert_eq!(
            w.capture("foot", 10, false).hosted,
            None,
            "not a terminal: no hosted name"
        );
        let w = terminal("fg-no-launcher", &["foot"], 12, &[]);
        assert_eq!(w.capture("foot", 10, false).exec, "foot");
    }

    #[test]
    fn a_foreground_off_the_host_or_an_untrusted_window_is_not_captured() {
        let w = terminal("fg-foreign", &["foot"], 12, &[FOOT]);
        w.file("12.ns", "foreign\n");
        assert_eq!(
            w.capture("foot", 10, false).exec,
            "foot",
            "a non-host leader is never read"
        );
        let w = terminal("fg-untrusted", &["foot"], 12, &[FOOT]);
        let proc = Proc::fixture(w.0.join("cmd"));
        let index = DesktopIndex::build(
            &[w.0.join("applications").to_string_lossy().into_owned()],
            crate::index::Launcher::None,
        );
        let c = capture_resolve(&proc, &index, "foot", 10, Some(true), false);
        assert_eq!(
            c.source, None,
            "§9: an XWayland window's foreground is not eligible"
        );
        assert!(!c.exec.contains("herdr"));
    }

    #[test]
    fn a_terminal_taking_its_command_without_an_option() {
        let kterm = (
            "kterm.desktop",
            "Exec=kterm\nCategories=TerminalEmulator;\nX-TerminalArgExec=",
        );
        let w = terminal("fg-no-option", &["kterm", "--single"], 12, &[kterm]);
        assert_eq!(
            w.capture("kterm", 10, false).exec,
            "kterm --single herdr --session main"
        );
        let w = terminal("fg-no-option-named", &["kterm", "vim"], 12, &[kterm]);
        assert_eq!(
            w.capture("kterm", 10, false).exec,
            "kterm vim",
            "already names one"
        );
    }
}
