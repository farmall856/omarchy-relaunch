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
pub mod launch;
pub mod lua;
pub mod verify;

pub use launch::{decide, Spawn};
pub use lua::{emit, BOOT_REGISTRATION};
pub use verify::{load_verified, trusted_chain};

/// stdout for `emit-lua`: the pins when the config verified, or a single
/// comment (no pins, no boot registration) when it did not — fail-closed.
pub fn emit_lua_output(result: &Result<relaunch_core::SignedConfig, String>) -> String {
    match result {
        Ok(cfg) => emit(cfg),
        Err(reason) => format!("-- relaunch-anchor: not launching ({reason})\n"),
    }
}
