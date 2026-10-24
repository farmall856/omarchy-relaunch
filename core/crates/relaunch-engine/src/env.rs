//! The ONE place the engine reads its environment.
//!
//! Production reads only `RELAUNCH_CONFIG_DIR` (it redirects the user's own
//! files; CLAUDE.md's live-probe workflow depends on it) and the XDG base
//! directories (via the `xdg` crate, which reads the real environment itself). Every other override exists for the test fixtures and is
//! compiled in only with the `fixtures` feature (D-P3): the release binary
//! cannot be pointed at fake process state, fake data dirs or a fake `hyprctl`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::host::Proc;

/// Everything the engine takes from its environment, resolved once.
#[derive(Clone, Debug)]
pub struct Env {
    /// Relaunch's private per-user directory.
    pub config_dir: PathBuf,
    /// Whether `RELAUNCH_CONFIG_DIR` named `config_dir` (a sandbox), rather
    /// than it defaulting to the user's own.
    pub config_dir_named: bool,
    /// Process state: the live system, or (fixtures) a fixture tree.
    pub proc: Proc,
    /// The XDG application directories, in search order, as path strings:
    /// `$XDG_DATA_HOME/applications` (always listed), then each existing
    /// `$XDG_DATA_DIRS/*/applications`.
    pub app_dirs: Vec<String>,
    /// `PATH`, for finding `gio` / `gtk-launch` (`command -v`).
    pub path: String,
    /// The `hyprctl` to run: the fixed system one (never `PATH`), or
    /// (fixtures) `HYPRCTL`.
    pub hyprctl: OsString,
    /// Omarchy's `autostart.lua` (`$HOME/.config/hypr/autostart.lua`, or
    /// (fixtures) `RELAUNCH_AUTOSTART`); `None` without a home directory.
    pub autostart: Option<PathBuf>,
    /// The user's `hyprland.lua`, which carries the one bootstrap line
    /// (`$HOME/.config/hypr/hyprland.lua`; fixtures `RELAUNCH_HYPRLAND_LUA`).
    pub hyprland_lua: Option<PathBuf>,
    /// The user's legacy `hyprland.conf` (`$HOME/.config/hypr/hyprland.conf`;
    /// fixtures `RELAUNCH_HYPR_CONF`).
    pub hypr_conf: Option<PathBuf>,
    /// The bar plugin's directory (fixtures `RELAUNCH_PLUGIN_DIR`).
    pub plugin_dir: Option<PathBuf>,
    /// The `relaunch` on the user's PATH, which uninstall removes when it is
    /// ours: `$HOME/.local/bin/relaunch` (fixtures: `$PREFIX/bin/relaunch`).
    pub engine_bin: Option<PathBuf>,
    /// The root-owned login verifier (fixtures `RELAUNCH_ANCHOR_BIN`).
    pub anchor_bin: PathBuf,
    /// The root-owned Lua loader the anchor bootstrap line runs (fixtures
    /// `RELAUNCH_ANCHOR_LUA`).
    pub anchor_lua: PathBuf,
    /// The client for the authority service (fixtures `RELAUNCH_RELAUNCHCTL`).
    pub relaunchctl: PathBuf,
}

/// Where the system package installs Relaunch's root-owned binaries.
pub const SYSTEM_LIB_DIR: &str = "/usr/lib/omarchy-relaunch";
/// Where the system package installs the anchor loader.
pub const ANCHOR_LUA: &str = "/usr/share/omarchy-relaunch/anchor.lua";

impl Env {
    /// Resolve from the process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self::from_vars(
            |name| std::env::var_os(name),
            &relaunch_sys::paths::base_dirs(),
        )
    }

    /// Resolve the `RELAUNCH_*` variables through `get` (testable without
    /// mutating the process environment) and the XDG dirs from `xdg`.
    pub fn from_vars(get: impl Fn(&str) -> Option<OsString>, xdg: &xdg::BaseDirectories) -> Self {
        let nonempty = |name: &str| get(name).filter(|v| !v.is_empty());
        let config_dir_named = nonempty("RELAUNCH_CONFIG_DIR").is_some();
        let config_dir = nonempty("RELAUNCH_CONFIG_DIR").map_or_else(
            || {
                xdg.config_home
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/.config"))
                    .join("omarchy-relaunch")
            },
            PathBuf::from,
        );
        let proc = proc_source(fixture(&nonempty, "RELAUNCH_CMDLINE_DIR"));
        let app_dirs = match fixture(&nonempty, "RELAUNCH_DATA_DIRS") {
            Some(dirs) => fixture_app_dirs(&dirs.to_string_lossy()),
            None => xdg_app_dirs(xdg),
        };
        let path = get("PATH")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let hyprctl = fixture(&nonempty, "HYPRCTL")
            .unwrap_or_else(|| OsString::from(relaunch_sys::paths::HYPRCTL));
        let autostart = fixture(&nonempty, "RELAUNCH_AUTOSTART")
            .map(PathBuf::from)
            .or_else(|| {
                nonempty("HOME").map(|h| PathBuf::from(h).join(".config/hypr/autostart.lua"))
            });
        let home = nonempty("HOME").map(PathBuf::from);
        let under_home = |fixture_var: &str, rel: &str| {
            fixture(&nonempty, fixture_var)
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(rel)))
        };
        // A fixtures build is a test build: it reaches a system component only
        // through an override naming a stand-in the test made — a file this
        // user owns — never the installed one (root's), whatever the
        // environment says. So no test can drive the live authority service
        // (a `relaunchctl purge` would delete this user's signing key) or the
        // live anchor, not even with an inherited override.
        let system = |fixture_var: &str, path: &str| {
            let absent = || {
                Path::new(FIXTURE_SYSTEM_ROOT).join(Path::new(path).file_name().unwrap_or_default())
            };
            match fixture(&nonempty, fixture_var) {
                Some(stand_in) if is_own_file(Path::new(&stand_in)) => PathBuf::from(stand_in),
                Some(_) => absent(),
                None if cfg!(feature = "fixtures") => absent(),
                None => PathBuf::from(path),
            }
        };
        Self {
            config_dir,
            config_dir_named,
            proc,
            app_dirs,
            path,
            hyprctl,
            autostart,
            hyprland_lua: under_home("RELAUNCH_HYPRLAND_LUA", ".config/hypr/hyprland.lua"),
            hypr_conf: under_home("RELAUNCH_HYPR_CONF", ".config/hypr/hyprland.conf"),
            plugin_dir: under_home(
                "RELAUNCH_PLUGIN_DIR",
                ".config/omarchy/plugins/io.github.laytonf.relaunch",
            ),
            engine_bin: fixture(&nonempty, "PREFIX")
                .map(|p| PathBuf::from(p).join("bin/relaunch"))
                .or_else(|| home.as_ref().map(|h| h.join(".local/bin/relaunch"))),
            anchor_bin: system(
                "RELAUNCH_ANCHOR_BIN",
                &format!("{SYSTEM_LIB_DIR}/relaunch-anchor"),
            ),
            anchor_lua: system("RELAUNCH_ANCHOR_LUA", ANCHOR_LUA),
            relaunchctl: system(
                "RELAUNCH_RELAUNCHCTL",
                &format!("{SYSTEM_LIB_DIR}/relaunchctl"),
            ),
        }
    }
}

/// Where a fixtures build looks for a system component no usable override
/// names: a directory that does not exist, so the component is absent.
pub const FIXTURE_SYSTEM_ROOT: &str = "/nonexistent/relaunch-fixtures";

/// Whether `path` (followed through any link) is a file the current user owns:
/// a test's stand-in, never an installed system component.
fn is_own_file(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.uid() == relaunch_sys::current_uid())
}

/// `IFS=: read -r -a`: fields kept (empty ones too) except one trailing empty.
fn colon_fields(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split(':').collect();
    if v.last() == Some(&"") {
        v.pop();
    }
    v
}

fn is_dir(p: &str) -> bool {
    std::path::Path::new(p).is_dir()
}

/// `$XDG_DATA_HOME/applications` (always listed; a missing one is skipped when
/// indexing), then each `$XDG_DATA_DIRS` entry's existing `applications` dir.
fn xdg_app_dirs(xdg: &xdg::BaseDirectories) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(home) = &xdg.data_home {
        out.push(home.join("applications").to_string_lossy().into_owned());
    }
    for d in &xdg.data_dirs {
        let apps = d.join("applications");
        if apps.is_dir() {
            out.push(apps.to_string_lossy().into_owned());
        }
    }
    out
}

/// `RELAUNCH_DATA_DIRS` (fixtures): each entry's `applications` dir, and the
/// entry itself when it IS an `applications` dir.
#[cfg_attr(
    not(feature = "fixtures"),
    allow(dead_code, reason = "fixture-only path")
)]
fn fixture_app_dirs(dirs: &str) -> Vec<String> {
    let mut out = Vec::new();
    for d in colon_fields(dirs) {
        let apps = format!("{d}/applications");
        if is_dir(&apps) {
            out.push(apps);
        }
        if is_dir(d) && crate::argv::basename(d) == "applications" {
            out.push(d.to_owned());
        }
    }
    out
}

/// Process state source: a fixture tree only in a `fixtures` build.
#[cfg(feature = "fixtures")]
fn proc_source(dir: Option<OsString>) -> Proc {
    dir.map_or_else(Proc::live, |d| Proc::fixture(PathBuf::from(d)))
}

#[cfg(not(feature = "fixtures"))]
fn proc_source(_dir: Option<OsString>) -> Proc {
    Proc::live()
}

/// A fixture override: honoured only in a `fixtures` build.
#[cfg(feature = "fixtures")]
fn fixture(nonempty: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<OsString> {
    nonempty(name)
}

/// A fixture override: ignored in a release build.
#[cfg(not(feature = "fixtures"))]
fn fixture(_nonempty: &impl Fn(&str) -> Option<OsString>, _name: &str) -> Option<OsString> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |n| {
            pairs
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_vars(vars(pairs), &xdg::BaseDirectories::new())
    }

    #[test]
    fn config_dir_override_is_honoured_and_empty_means_unset() {
        let e = env(&[("RELAUNCH_CONFIG_DIR", "/tmp/probe")]);
        assert_eq!(e.config_dir, PathBuf::from("/tmp/probe"));
        let e = env(&[("RELAUNCH_CONFIG_DIR", ""), ("XDG_CONFIG_HOME", "")]);
        assert_ne!(e.config_dir, PathBuf::from(""));
    }

    #[cfg(not(feature = "fixtures"))]
    #[test]
    fn release_build_ignores_fixture_overrides() {
        let e = env(&[
            ("RELAUNCH_CMDLINE_DIR", "/tmp/evil"),
            ("RELAUNCH_DATA_DIRS", "/tmp/evil"),
            ("HYPRCTL", "/tmp/evil/hyprctl"),
            ("RELAUNCH_AUTOSTART", "/tmp/evil/autostart.lua"),
            ("RELAUNCH_HYPRLAND_LUA", "/tmp/evil/h.lua"),
            ("RELAUNCH_ANCHOR_BIN", "/tmp/evil/anchor"),
            ("RELAUNCH_ANCHOR_LUA", "/tmp/evil/anchor.lua"),
            ("RELAUNCH_RELAUNCHCTL", "/tmp/evil/ctl"),
            ("PREFIX", "/tmp/evil"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(
            e.hyprland_lua,
            Some(PathBuf::from("/home/u/.config/hypr/hyprland.lua"))
        );
        assert_eq!(
            e.engine_bin,
            Some(PathBuf::from("/home/u/.local/bin/relaunch"))
        );
        assert_eq!(
            e.anchor_bin,
            PathBuf::from("/usr/lib/omarchy-relaunch/relaunch-anchor")
        );
        assert_eq!(e.anchor_lua, PathBuf::from(ANCHOR_LUA));
        assert_eq!(
            e.relaunchctl,
            PathBuf::from("/usr/lib/omarchy-relaunch/relaunchctl")
        );
        assert_eq!(
            e.autostart,
            Some(PathBuf::from("/home/u/.config/hypr/autostart.lua"))
        );
        assert!(!e.proc.is_fixture());
        assert_eq!(e.hyprctl, OsString::from("/usr/bin/hyprctl"));
        assert!(!e.app_dirs.iter().any(|d| d.starts_with("/tmp/evil")));
    }

    #[cfg(feature = "fixtures")]
    #[test]
    fn fixtures_build_honours_fixture_overrides() {
        let e = env(&[("RELAUNCH_CMDLINE_DIR", "/tmp/fixture")]);
        assert!(e.proc.is_fixture());
    }

    #[cfg(feature = "fixtures")]
    #[test]
    fn a_fixtures_build_never_reaches_an_installed_system_component() {
        let e = env(&[("HOME", "/home/u")]);
        for (what, path) in [
            ("relaunchctl", &e.relaunchctl),
            ("anchor", &e.anchor_bin),
            ("anchor.lua", &e.anchor_lua),
        ] {
            assert!(
                path.starts_with(FIXTURE_SYSTEM_ROOT),
                "{what}: {}",
                path.display()
            );
            assert!(!path.exists(), "{what}: {}", path.display());
        }
        // An override is honoured only for a stand-in this user owns.
        let dir = std::env::temp_dir().join(format!("relaunch-env-stub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("relaunchctl");
        std::fs::write(&stub, "#!/bin/sh\n").unwrap();
        let stub_s = stub.to_string_lossy().into_owned();
        let e = env(&[("RELAUNCH_RELAUNCHCTL", stub_s.as_str())]);
        assert_eq!(e.relaunchctl, stub, "a stand-in the test made is used");
        // An inherited override naming a file this user does not own — an
        // installed component, or a link to one — is never used.
        let link = dir.join("link");
        std::os::unix::fs::symlink("/usr/bin/env", &link).unwrap();
        let link_s = link.to_string_lossy().into_owned();
        for theirs in [
            "/usr/bin/env",
            link_s.as_str(),
            "/usr/lib/omarchy-relaunch/relaunchctl",
            "/nonexistent/x",
        ] {
            for var in [
                "RELAUNCH_RELAUNCHCTL",
                "RELAUNCH_ANCHOR_BIN",
                "RELAUNCH_ANCHOR_LUA",
            ] {
                let e = env(&[(var, theirs)]);
                for path in [&e.relaunchctl, &e.anchor_bin, &e.anchor_lua] {
                    assert!(
                        path.starts_with(FIXTURE_SYSTEM_ROOT),
                        "{var}={theirs}: {}",
                        path.display()
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
