//! relaunchctl — the user-run Relaunch client (spec §2, §5.1, §12).
//!
//! The panel drives every launch-list mutation through this binary:
//! - an **expanding** change (add / enable / edit-command / change-workspace /
//!   adopt) goes through `relaunchctl commit`, which holds one bus connection
//!   across the human pause — `Prepare` → show the service's summary and diff on
//!   the panel → `Authorize` on the *same* connection (§3, §5.1);
//! - a **reducing** change (disable / drop) goes through the promptless
//!   one-shot `relaunchctl reduce` (§5.3/§5.4);
//! - `relaunchctl purge` deletes the user's key + state after the user
//!   authenticates (polkit `io.github.laytonf.relaunch.purge`, §5.4);
//! - `relaunchctl status` reports only *that* a transaction is open (§4).
//!
//! The binary never writes `signed`/`signature` itself and never reinterprets
//! the signed base: it forwards the opaque base to the service, which verifies
//! it, and relays the signed result back to the panel to persist. The
//! transaction id lives only inside a running `commit` process and on the wire
//! (`protocol` enforces this structurally).
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

pub mod bus;
pub mod driver;
pub mod frame;
pub mod protocol;
