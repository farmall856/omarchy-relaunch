//! relaunchd — the root authority engine for the Relaunch authorization
//! redesign (spec: `relaunch-authz-spec-final.md`).
//!
//! This crate holds the *logic* behind the root D-Bus service: the per-uid key
//! store and durable generation commit (`store`), the frozen-transaction lease
//! (`txn`), the escaped review diff with its D7 caps (`diff`), and the engine
//! that ties them together under per-uid serialization (`engine`). It is pure
//! apart from the state directory it owns; the D-Bus transport, the caller-uid
//! lookup from bus credentials, and the polkit-backed `Authenticator` are a
//! thin adapter added in a later chunk. Keeping the security decisions here, off
//! the bus, is what makes them testable in isolation.

pub mod diff;
pub mod engine;
pub mod polkit;
pub mod store;
pub mod txn;
pub mod users;

pub use engine::{
    AuthRequest, Authenticator, Engine, EngineError, InvalidateHook, PrepareOk, Proposal, ReduceOp,
};
pub use polkit::{PolkitAuthenticator, PolkitCanceller};
pub use store::{enrollment_id_for, PublicState, Store, StoreError};
pub use txn::{Clock, Transaction, DEFAULT_TTL_SECS};
