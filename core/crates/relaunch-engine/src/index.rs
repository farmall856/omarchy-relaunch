//! The desktop index: every `.desktop` launcher in the XDG application dirs,
//! keyed for window-class lookup (a port of the bash engine's index, with the
//! ONE shared Desktop Entry parser, INV-2).
//!
//! Files are visited in `find -L DIR -maxdepth 2 -type f -name '*.desktop'`
//! order: dirs in XDG order, entries in raw directory order, depth-first. Every
//! map is first-wins, so this order is part of the contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use relaunch_sys::fs::{read_regular_capped, MAX_FILE_BYTES};
use serde_json::{Map, Value};

use crate::argv::{basename, is_wrapper_cmd, leaf_for_exec};
use crate::key::nkey;
use crate::shell::Exec;
use crate::webapp;

/// How a `.desktop` file is started by the legacy launch strings: whichever of
/// `gio` and `gtk-launch` is on `PATH`, checked once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launcher {
    /// `gio launch <file>`.
    Gio,
    /// `gtk-launch <desktop id>`.
    GtkLaunch,
    /// Neither: no launch string, so no exec-map entries.
    None,
}

impl Launcher {
    /// Pick the launcher available on `path` (a `PATH`-style list).
    #[must_use]
    pub fn detect(path: &str) -> Self {
        if on_path("gio", path) {
            Self::Gio
        } else if on_path("gtk-launch", path) {
            Self::GtkLaunch
        } else {
            Self::None
        }
    }

    fn command(self, file: &str) -> Option<Exec> {
        match self {
            Self::Gio => Some(Exec::from_argv(&["gio", "launch", file])),
            Self::GtkLaunch => Some(Exec::from_argv(&["gtk-launch", desktop_stem(file)])),
            Self::None => None,
        }
    }
}

/// `command -v NAME`: an executable regular file in a `PATH` entry (an empty
/// entry is the current directory).
fn on_path(name: &str, path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.split(':').any(|dir| {
        let dir = if dir.is_empty() { "." } else { dir };
        std::fs::metadata(format!("{dir}/{name}"))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// The file's basename without `.desktop`.
fn desktop_stem(file: &str) -> &str {
    let b = basename(file);
    b.strip_suffix(".desktop").unwrap_or(b)
}

/// The index maps (names match the bash engine's, for the differential dump).
#[derive(Debug, Default)]
pub struct DesktopIndex {
    exec_by_class: BTreeMap<String, String>,
    exec_by_id: BTreeMap<String, String>,
    exec_by_name: BTreeMap<String, String>,
    class_by_exec: BTreeMap<String, String>,
    path_by_class: BTreeMap<String, String>,
    path_by_id: BTreeMap<String, String>,
    path_by_name: BTreeMap<String, String>,
    name_by_leaf: BTreeMap<String, String>,
    webapp_launch_by_identity: BTreeMap<String, String>,
    webapp_path_by_identity: BTreeMap<String, String>,
    webapp_id_by_identity: BTreeMap<String, String>,
    webapp_ambiguous: BTreeMap<String, String>,
    webapp_id_claimed: BTreeMap<String, String>,
    /// A web app's host could not be canonicalized: it may name any identity,
    /// so none resolves.
    webapp_uncanonical: bool,
    /// Terminal emulators (`Categories=…TerminalEmulator…`) by the program
    /// their `Exec` runs: the option that precedes a command to run in one.
    terminal_arg_by_program: BTreeMap<String, String>,
}

/// The option a terminal takes before a command when its `.desktop` names
/// none (`X-TerminalArgExec`'s default).
const DEFAULT_TERMINAL_ARG_EXEC: &str = "-e";

fn set_if_empty(map: &mut BTreeMap<String, String>, key: &str, value: &str) {
    if !key.is_empty() && !value.is_empty() && !map.contains_key(key) {
        map.insert(key.to_owned(), value.to_owned());
    }
}

/// The `.desktop` files under `dir`, in `find -L -maxdepth 2` order.
fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(md) = std::fs::metadata(&path) else {
                continue; // a dangling link is not a file (find -L -type f)
            };
            if md.is_dir() {
                if depth < 2 {
                    walk(&path, depth + 1, out);
                }
            } else if md.is_file() && entry.file_name().to_string_lossy().ends_with(".desktop") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 1, &mut out);
    out
}

impl DesktopIndex {
    /// Index every launcher under `app_dirs`, in order.
    #[must_use]
    pub fn build(app_dirs: &[String], launcher: Launcher) -> Self {
        let mut idx = Self::default();
        for dir in app_dirs {
            let root = Path::new(dir);
            if !root.is_dir() {
                continue;
            }
            for file in desktop_files(root) {
                idx.add_file(&file.to_string_lossy(), dir, launcher);
            }
        }
        idx
    }

    fn add_file(&mut self, file: &str, root: &str, launcher: Launcher) {
        // An unreadable file contributes no keys but still claims its id.
        let text = read_regular_capped(Path::new(file), MAX_FILE_BYTES)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let keys = relaunch_desktop::index_keys(&text);
        let raw_exec = keys.exec.unwrap_or_default();
        let name = keys.name.unwrap_or_default();
        let wmclass = keys.startup_wm_class.unwrap_or_default();

        // Web apps: XDG desktop-id masking, this map only. The claim comes
        // before the Hidden check, so a Hidden=true override masks the system
        // entry instead of letting it register.
        let dfid = desktop_file_id(file, root);
        if !self.webapp_id_claimed.contains_key(&dfid) {
            self.webapp_id_claimed.insert(dfid.clone(), "1".into());
            if !keys.hidden {
                match webapp::identity_for_exec(&raw_exec) {
                    Some(webapp::Identity::Uncanonical) => self.webapp_uncanonical = true,
                    Some(webapp::Identity::Known(ident)) => {
                        match self.webapp_id_by_identity.get(&ident) {
                            Some(owner) if *owner != dfid => {
                                // Two surviving distinct ids, one identity: refuse to guess.
                                self.webapp_ambiguous.insert(ident, "1".into());
                            }
                            _ => {
                                self.webapp_id_by_identity.insert(ident.clone(), dfid);
                                self.webapp_path_by_identity
                                    .insert(ident.clone(), file.to_owned());
                                if let Some(cmd) = launcher.command(file) {
                                    self.webapp_launch_by_identity
                                        .insert(ident, cmd.into_string());
                                }
                            }
                        }
                    }
                    None => {}
                }
            }
        }

        if keys.hidden || raw_exec.is_empty() {
            return;
        }
        if keys.terminal_emulator {
            if let Some(prog) = crate::argv::program(&crate::shell::split_stored(&raw_exec)) {
                let arg = keys
                    .terminal_arg_exec
                    .as_deref()
                    .unwrap_or(DEFAULT_TERMINAL_ARG_EXEC);
                // First wins, and an empty option is a real value.
                self.terminal_arg_by_program
                    .entry(prog.to_owned())
                    .or_insert_with(|| arg.to_owned());
            }
        }
        let id = desktop_stem(file);
        let (wmkey, idkey, namekey) = (nkey(&wmclass), nkey(id), nkey(&name));
        // Name is a last-resort key: a PWA named "Proton Mail" with a crx_
        // StartupWMClass must not steal the official app's class.
        let by_name = wmclass.is_empty() || wmkey == namekey;
        // The launcher paths (what capture binds an entry to) need no launch
        // helper; only the legacy launch strings do.
        set_if_empty(&mut self.path_by_class, &wmkey, file);
        set_if_empty(&mut self.path_by_id, &idkey, file);
        if by_name {
            set_if_empty(&mut self.path_by_name, &namekey, file);
        }
        let Some(launch) = launcher.command(file) else {
            return;
        };
        let launch = launch.into_string();
        set_if_empty(&mut self.exec_by_class, &wmkey, &launch);
        set_if_empty(&mut self.exec_by_id, &idkey, &launch);
        if by_name {
            set_if_empty(&mut self.exec_by_name, &namekey, &launch);
        }
        let owner = if wmclass.is_empty() {
            id
        } else {
            wmclass.as_str()
        };
        let first = first_word(&raw_exec);
        set_if_empty(&mut self.class_by_exec, &nkey(&raw_exec), owner);
        set_if_empty(&mut self.class_by_exec, &nkey(first), owner);
        if !name.is_empty()
            && (is_wrapper_cmd(first) || matches!(basename(first), "bash" | "sh" | "zsh" | "dash"))
        {
            // Only a wrapper or shell yields a leaf that differs from the
            // first word; a plain Exec would let any app claim a bare command.
            if let Some(leaf) = leaf_for_exec(&raw_exec).filter(|l| !l.is_empty() && l != first) {
                set_if_empty(&mut self.name_by_leaf, &nkey(&leaf), &name);
            }
        }
    }

    /// The legacy launch string for a window class: `StartupWMClass` or
    /// desktop-file id, then a web-app identity, then `Name`.
    #[must_use]
    pub fn exec_for_class(&self, class: &str) -> Option<&str> {
        let k = nkey(class);
        self.exec_by_class
            .get(&k)
            .or_else(|| self.exec_by_id.get(&k))
            .or_else(|| self.webapp(class, &self.webapp_launch_by_identity))
            .or_else(|| self.exec_by_name.get(&k))
            .map(String::as_str)
    }

    /// The `.desktop` file a class resolves to, in the same tier order.
    #[must_use]
    pub fn launcher_for_class(&self, class: &str) -> Option<&str> {
        let k = nkey(class);
        self.path_by_class
            .get(&k)
            .or_else(|| self.path_by_id.get(&k))
            .or_else(|| self.webapp(class, &self.webapp_path_by_identity))
            .or_else(|| self.path_by_name.get(&k))
            .map(String::as_str)
    }

    /// A Chromium web-app window class, `<browser>-<identity>-Default`, for
    /// whichever browser opened it: the first word is the browser's, the
    /// rest the identity. An ambiguous identity resolves to nothing, and a
    /// class that is not a web app's matches no identity.
    fn webapp<'a>(&self, class: &str, map: &'a BTreeMap<String, String>) -> Option<&'a String> {
        let (_browser, ident) = class.strip_suffix("-Default")?.split_once('-')?;
        if ident.is_empty() || self.webapp_uncanonical || self.webapp_ambiguous.contains_key(ident)
        {
            return None;
        }
        map.get(ident)
    }

    /// The option that precedes a command to run in the terminal emulator
    /// whose program is `program` (a basename), when it is one.
    #[must_use]
    pub fn terminal_arg_exec(&self, program: &str) -> Option<&str> {
        self.terminal_arg_by_program
            .get(program)
            .map(String::as_str)
    }

    /// The class a launch command belongs to, by its whole text or first word.
    #[must_use]
    pub fn class_for_exec(&self, exec: &str) -> Option<&str> {
        self.class_by_exec
            .get(&nkey(exec))
            .or_else(|| self.class_by_exec.get(&nkey(first_word(exec))))
            .map(String::as_str)
    }

    /// The `Name` of a terminal-wrapped launcher whose leaf command is `leaf`
    /// (display labels only; never used to resolve a launch).
    #[must_use]
    pub fn name_for_leaf(&self, leaf: &str) -> Option<&str> {
        self.name_by_leaf.get(&nkey(leaf)).map(String::as_str)
    }

    /// Every map, for the differential oracle (`__index-dump`).
    #[must_use]
    pub fn dump(&self) -> Value {
        let maps: [(&str, &BTreeMap<String, String>); 13] = [
            ("DESKTOP_EXEC_BY_CLASS", &self.exec_by_class),
            ("DESKTOP_EXEC_BY_ID", &self.exec_by_id),
            ("DESKTOP_EXEC_BY_NAME", &self.exec_by_name),
            ("DESKTOP_CLASS_BY_EXEC", &self.class_by_exec),
            ("DESKTOP_PATH_BY_CLASS", &self.path_by_class),
            ("DESKTOP_PATH_BY_ID", &self.path_by_id),
            ("DESKTOP_PATH_BY_NAME", &self.path_by_name),
            ("DESKTOP_NAME_BY_LEAF", &self.name_by_leaf),
            ("WEBAPP_LAUNCH_BY_IDENTITY", &self.webapp_launch_by_identity),
            ("WEBAPP_PATH_BY_IDENTITY", &self.webapp_path_by_identity),
            ("WEBAPP_ID_BY_IDENTITY", &self.webapp_id_by_identity),
            ("WEBAPP_AMBIGUOUS", &self.webapp_ambiguous),
            ("WEBAPP_ID_CLAIMED", &self.webapp_id_claimed),
        ];
        let mut out = Map::new();
        for (name, map) in maps {
            let inner: Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                .collect();
            out.insert(name.to_owned(), Value::Object(inner));
        }
        Value::Object(out)
    }
}

/// Up to the first space (`${exec%% *}`), not an unquoted word.
fn first_word(exec: &str) -> &str {
    exec.split(' ').next().unwrap_or(exec)
}

/// The XDG desktop-file id: the path under its applications dir, `.desktop`
/// dropped, `/` as `-`. Case-sensitive, unlike an index key, so distinct
/// launchers cannot mask one another.
fn desktop_file_id(file: &str, root: &str) -> String {
    let rel = file
        .strip_prefix(root)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(file);
    rel.strip_suffix(".desktop")
        .unwrap_or(rel)
        .replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps(tag: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<String>) {
        let root =
            std::env::temp_dir().join(format!("relaunch-index-{tag}-{}", std::process::id()));
        let dir = root.join("applications");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, url) in files {
            std::fs::write(
                dir.join(format!("{name}.desktop")),
                format!("[Desktop Entry]\nType=Application\nName={name}\nExec=omarchy-launch-webapp {url}\n"),
            )
            .unwrap();
        }
        (root, vec![dir.to_string_lossy().into_owned()])
    }

    #[test]
    fn a_host_it_cannot_canonicalize_leaves_web_apps_unresolved() {
        // Chromium names both of these brave-xn--bcher-kva.example__-Default.
        let (root, dirs) = apps(
            "idn",
            &[
                ("Unicode", "https://b\u{fc}cher.example/"),
                ("Puny", "https://xn--bcher-kva.example/"),
            ],
        );
        let idx = DesktopIndex::build(&dirs, Launcher::Gio);
        assert_eq!(
            idx.launcher_for_class("brave-xn--bcher-kva.example__-Default"),
            None
        );
        assert_eq!(
            idx.exec_for_class("brave-xn--bcher-kva.example__-Default"),
            None
        );
        let _ = std::fs::remove_dir_all(root);
        let (root, dirs) = apps(
            "pct",
            &[
                ("Pct", "https://%65xample.com/"),
                ("Plain", "https://example.com/"),
            ],
        );
        let idx = DesktopIndex::build(&dirs, Launcher::Gio);
        assert_eq!(idx.launcher_for_class("brave-example.com__-Default"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_paths_do_not_depend_on_a_launch_helper() {
        let (root, dirs) = apps("nohelper", &[("Maps", "https://maps.example/")]);
        let idx = DesktopIndex::build(&dirs, Launcher::None);
        let want = format!("{}/Maps.desktop", dirs[0]);
        assert_eq!(
            idx.launcher_for_class("brave-maps.example__-Default"),
            Some(want.as_str())
        );
        assert_eq!(idx.launcher_for_class("Maps"), Some(want.as_str()));
        for browser in ["chrome", "chromium", "vivaldi", "msedge"] {
            assert_eq!(
                idx.launcher_for_class(&format!("{browser}-maps.example__-Default")),
                Some(want.as_str()),
                "{browser}: a web app resolves whichever browser opened it"
            );
        }
        assert_eq!(idx.launcher_for_class("maps.example__-Default"), None);
        assert_eq!(idx.launcher_for_class("foo-maps.example__"), None);
        assert_eq!(
            idx.exec_for_class("Maps"),
            None,
            "no helper, no legacy launch string"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
