//! The Relaunch engine (bash → Rust port, M1+): capture, resolution, proposal
//! assembly, `config.json` handling and the panel protocol. It runs as the
//! user and never signs anything; `relaunchd` does that after the panel's
//! reviewed Prepare/Authorize (spec §3, §12).
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

pub mod cli;
pub use relaunch_shell::{argv, execcheck, shell};
pub mod compositor;
pub mod config;
pub mod edit;
pub mod env;
pub mod hooks;
pub mod host;
pub mod index;
pub mod jq;
pub mod key;
pub mod launcher;
pub mod list;
pub mod propose;
pub mod resolve;
pub mod session;
pub mod startup;
pub mod stored;
pub mod webapp;
