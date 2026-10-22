//! relaunch-anchor — the root-owned, user-run login verifier for the Relaunch
//! authorization redesign (spec: `relaunch-authz-spec-final.md`).
//!
//! It verifies the signed `config.json` against the root-held public key at
//! login (`verify`), emits the Hyprland pins from the *verified* document only
//! (`lua`), and at boot launches each eligible entry by parsing the hashed
//! launcher buffer into a direct `execve` argv or running a reviewed shell
//! string (`desktop`, `launch`). On-disk `relaunch.lua`/`rules.lua` are never
//! executed. The binary resolves its paths from `getuid()`, `$HOME` and the
//! fixed `/var/lib/omarchy-relaunch` — it reads no `RELAUNCH_*` override.

pub mod desktop;
pub mod fsio;
pub mod launch;
pub mod lua;
pub mod verify;

pub use launch::{decide, Spawn};
pub use lua::{emit, BOOT_REGISTRATION};
pub use verify::{load_verified, trusted_chain};

/// The constant stdout comment printed when verification fails. The reason goes
/// to stderr only (reviewer I9), so no verifier error text — which could carry
/// newlines or shell-ish content — is ever interpolated into Hyprland-bound
/// stdout.
pub const NOT_LAUNCHING_COMMENT: &str = "-- relaunch-anchor: not launching\n";
