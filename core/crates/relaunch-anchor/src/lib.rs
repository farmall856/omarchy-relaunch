//! relaunch-anchor — the root-owned, user-run login verifier for the Relaunch
//! authorization redesign (spec: `relaunch-authz-spec-final.md`).
//!
//! It verifies the signed `config.json` against the root-held public key at
//! login (`verify`), emits the Hyprland pins from the *verified* document only
//! (`lua`), and at boot launches each enabled entry by parsing its hashed
//! private launcher into a direct `execve` argv (`desktop`, `launch`), then
//! moves its window to its workspace (`boot`, `hypr`). On-disk `rules.lua` is never
//! executed. The binary resolves its paths from `getuid()`, `$HOME` and the
//! fixed `/var/lib/omarchy-relaunch` — it reads no `RELAUNCH_*` override.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code: a failed unwrap/index is a failed assertion"
    )
)]

/// The shared Desktop Entry parser (INV-2: one parser for capture and boot).
pub use relaunch_desktop as desktop;
pub mod boot;
pub mod hypr;
pub mod launch;
pub mod lua;
pub mod report;
pub mod verify;

pub use launch::{decide, launchable, launcher_path, read_launcher, Spawn};
pub use lua::{emit, BOOT_REGISTRATION};
pub use verify::{load_verified, trusted_chain};

/// The constant stdout comment printed when verification fails. The reason goes
/// to stderr only (reviewer I9), so no verifier error text — which could carry
/// newlines or shell-ish content — is ever interpolated into Hyprland-bound
/// stdout.
pub const NOT_LAUNCHING_COMMENT: &str = "-- relaunch-anchor: not launching\n";
