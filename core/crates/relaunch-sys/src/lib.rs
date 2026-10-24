//! Safe OS primitives shared by every Relaunch binary: identity (uid, account
//! name), the config-dir paths, bounded reads (of files and of a system tool's
//! output) and the one private atomic writer.
//!
//! The ONE place the process uid and a uid's account name are read (INV: one
//! function, many callers). The service binds `signed.user` to the account name
//! and the anchor re-checks it at login, so both must resolve names identically:
//! they call this, never their own lookup. There is no `unsafe` here; `nix`
//! wraps `getuid(2)` and the reentrant `getpwuid_r(3)` (NSS-aware).
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

pub mod cmd;
pub mod fs;
pub mod paths;

use nix::unistd::{getuid, Uid, User};

/// Write one diagnostic line to stderr (the journal for the service, the
/// terminal for a user). The single channel every Relaunch binary logs through;
/// stdout is reserved for each binary's protocol. A failed write is ignored:
/// there is nowhere better to report it.
#[macro_export]
macro_rules! diag {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stderr().lock(), $($arg)*);
    }};
}

/// The calling process's real uid.
///
/// Correct only in user-mode code (the anchor, the engine). The root service
/// takes the caller's uid from bus credentials instead (spec §2).
#[must_use]
pub fn current_uid() -> u32 {
    getuid().as_raw()
}

/// The current login name for `uid` from the password database, or `None` if
/// the uid has no entry or the lookup fails.
///
/// A name that is not valid UTF-8 is `None`, not a lossy approximation: the
/// name is bound into signed documents, so a decoded-with-replacement string
/// would be a different identity.
#[must_use]
pub fn account_name(uid: u32) -> Option<String> {
    let user = User::from_uid(Uid::from_raw(uid)).ok()??;
    if user.name.contains(char::REPLACEMENT_CHARACTER) {
        return None;
    }
    Some(user.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_uid_resolves_to_a_name() {
        let name = account_name(current_uid());
        assert!(name.is_some_and(|n| !n.is_empty()));
    }

    #[test]
    fn nonexistent_uid_is_none() {
        assert!(account_name(4_294_967_000).is_none());
    }
}
