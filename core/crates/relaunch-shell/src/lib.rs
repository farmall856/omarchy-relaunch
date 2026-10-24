//! Shell words, shared by the engine (capture, the panel's warnings) and the
//! login anchor (boot's launch check), so the two can never judge a command
//! differently (INV-2, INV-8).
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

pub mod argv;
pub mod execcheck;
pub mod shell;
