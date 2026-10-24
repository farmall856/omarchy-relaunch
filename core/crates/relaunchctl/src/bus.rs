//! The thin D-Bus layer: a generated async proxy for `io.github.laytonf.Relaunch1`
//! and a `Session` that holds exactly one system-bus connection for the lifetime
//! of a transaction (spec §5.1).
//!
//! The held connection is the whole point. `Authorize` binds the unique bus
//! sender of `Prepare`, so Prepare and Authorize must travel the *same*
//! connection; two `relaunchctl` invocations would be two senders and could not
//! satisfy that. `Session` therefore owns the `Connection` and the `Proxy` and
//! keeps them in scope across the human pause. When the process exits — on
//! cancel, on stdin EOF, on panel death — the connection drops and the service
//! sees a `NameOwnerChanged` for that sender, which cancels the transaction and
//! releases the one-per-uid slot (§5.2).
//!
//! The transaction id returned by `prepare` is held in a local and passed
//! straight back to `authorize`; it is never printed, logged, or placed on
//! argv. Live bus round-trips against a running service on the system bus are
//! exercised on hardware (chunk 7), not in unit tests.

use zbus::Connection;

/// The `io.github.laytonf.Relaunch1` interface, as the client calls it.
#[zbus::proxy(
    interface = "io.github.laytonf.Relaunch1",
    default_service = "io.github.laytonf.Relaunch1",
    default_path = "/io/github/laytonf/Relaunch1",
    gen_blocking = false
)]
pub trait Relaunch {
    /// Returns `(txid, code, summary, rendered_diff)`; an empty `txid` means the
    /// diff was empty and no transaction was minted.
    fn prepare(
        &self,
        base: Vec<u8>,
        proposal_json: Vec<u8>,
    ) -> zbus::Result<(Vec<u8>, String, String, String)>;
    /// Run the credential check for the prepared transaction; the signed document on success.
    fn authorize(&self, txid: Vec<u8>) -> zbus::Result<Vec<u8>>;
    /// Apply reducing ops to the verified base; the signed document.
    fn reduce(&self, base: Vec<u8>, ops: Vec<(String, String)>) -> zbus::Result<Vec<u8>>;
    /// Delete the caller's key and state.
    fn purge(&self) -> zbus::Result<()>;
    /// Whether a transaction is open for the caller's uid (existence only, §4).
    fn has_pending(&self) -> zbus::Result<bool>;
}

/// One held system-bus connection, for the length of a transaction. The proxy
/// owns the connection, so dropping the session drops the connection (§5.1).
pub struct Session {
    proxy: RelaunchProxy<'static>,
}

impl Session {
    /// Open the single connection used for this whole transaction.
    ///
    /// # Errors
    ///
    /// The system bus is unreachable or the proxy cannot be built.
    pub async fn connect() -> zbus::Result<Self> {
        let conn = Connection::system().await?;
        let proxy = RelaunchProxy::new(&conn).await?;
        Ok(Session { proxy })
    }

    /// The proxy on the held connection.
    #[must_use]
    pub fn proxy(&self) -> &RelaunchProxy<'static> {
        &self.proxy
    }
}

impl crate::driver::Service for Session {
    async fn prepare(
        &self,
        base: Vec<u8>,
        proposal: Vec<u8>,
    ) -> Result<(Vec<u8>, String, String, String), String> {
        self.proxy
            .prepare(base, proposal)
            .await
            .map_err(|e| e.to_string())
    }
    async fn authorize(&self, txid: Vec<u8>) -> Result<Vec<u8>, String> {
        self.proxy.authorize(txid).await.map_err(|e| e.to_string())
    }
    async fn reduce(&self, base: Vec<u8>, ops: Vec<(String, String)>) -> Result<Vec<u8>, String> {
        self.proxy
            .reduce(base, ops)
            .await
            .map_err(|e| e.to_string())
    }
}
