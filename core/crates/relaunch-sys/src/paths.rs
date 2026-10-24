//! Where Relaunch's per-user files live. One definition for every binary, so
//! the engine and the anchor can never disagree about a flag file's name.

use std::path::PathBuf;

/// The signed launch document.
pub const CONFIG_FILE: &str = "config.json";
/// Present: boot launches nothing until removed (advisory, spec §10).
pub const DISABLED_FLAG: &str = "disabled";
/// Present: the next boot launches nothing, then removes it.
pub const SKIP_ONCE_FLAG: &str = "skip-once";
/// The anchor's machine-readable record of the last boot.
pub const LAST_BOOT_JSON: &str = "last-boot.json";
/// The boot record for a person, beside [`LAST_BOOT_JSON`].
pub const LAST_BOOT_LOG: &str = "last-boot.log";

/// The compositor client every binary runs: a fixed path, never `PATH`.
pub const HYPRCTL: &str = "/usr/bin/hyprctl";
/// The clock every binary formats timestamps with: a fixed path, never `PATH`.
pub const DATE: &str = "/usr/bin/date";
/// The launch wrapper boot runs every pin under, as Omarchy's own launchers
/// do (a scope in the session's app slice): a fixed path, never `PATH`.
pub const UWSM_APP: &str = "/usr/bin/uwsm-app";

/// The XDG base directories of this process (the `xdg` crate: spec-correct, so a
/// relative value is ignored as if unset, and a missing `HOME` falls back to
/// the passwd entry).
#[must_use]
pub fn base_dirs() -> xdg::BaseDirectories {
    xdg::BaseDirectories::new()
}

/// `$XDG_CONFIG_HOME/omarchy-relaunch` (default `~/.config/omarchy-relaunch`).
/// The anchor uses exactly this; only the user-run engine also honours
/// `RELAUNCH_CONFIG_DIR`. With no home directory at all it is
/// `/.config/omarchy-relaunch`, which does not exist, so nothing launches.
#[must_use]
pub fn default_config_dir() -> PathBuf {
    base_dirs()
        .config_home
        .unwrap_or_else(|| PathBuf::from("/.config"))
        .join("omarchy-relaunch")
}
