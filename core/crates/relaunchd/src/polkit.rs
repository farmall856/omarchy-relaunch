//! The polkit-backed `Authenticator`: the trusted-path credential check.
//!
//! `Authorize` calls `org.freedesktop.PolicyKit1.Authority.CheckAuthorization`
//! with a `system-bus-name` subject naming the *caller's* unique connection, so
//! polkit resolves the subject to the calling process and runs AUTH_SELF for
//! that uid (fingerprint, then password — the installed polkit-1 PAM stack). The
//! `00-` rule (spec §3) denies non-local/inactive callers before the blanket
//! `empower` grant can apply.
//!
//! The call is synchronous (`zbus::blocking`) because the engine's
//! `Authenticator` trait is sync and is invoked off the per-uid lock, on a
//! blocking thread. The polkit dialog is NOT treated as a trusted display
//! surface (spec §1/D6); the only property we rely on here is that a real
//! credential was obtained for the caller's uid. The human cross-check is the
//! panel code, not this message.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Type, Value};

use crate::engine::Authenticator;

/// `(sa{sv})` — a polkit subject.
#[derive(Serialize, Type)]
struct Subject {
    kind: String,
    details: HashMap<String, OwnedValue>,
}

/// `(bba{ss})` — a polkit AuthorizationResult.
#[derive(Deserialize, Type)]
struct AuthzResult {
    is_authorized: bool,
    #[allow(dead_code)]
    is_challenge: bool,
    #[allow(dead_code)]
    details: HashMap<String, String>,
}

const AUTHORITY_DEST: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// CheckAuthorizationFlags::AllowUserInteraction.
const ALLOW_USER_INTERACTION: u32 = 0x1;

pub struct PolkitAuthenticator {
    bus: Connection,
    action_id: String,
}

impl PolkitAuthenticator {
    pub fn connect(action_id: &str) -> zbus::Result<Self> {
        Ok(PolkitAuthenticator {
            bus: Connection::system()?,
            action_id: action_id.to_string(),
        })
    }
}

impl Authenticator for PolkitAuthenticator {
    fn authorize(&self, _uid: u32, sender: &str, message: &str) -> Result<bool, String> {
        let name: OwnedValue = Value::new(sender.to_string())
            .try_into()
            .map_err(|e| format!("{e}"))?;
        let mut subject_details = HashMap::new();
        subject_details.insert("name".to_string(), name);
        let subject = Subject {
            kind: "system-bus-name".to_string(),
            details: subject_details,
        };

        // 3rd argument a{ss}: the message polkit may show. The panel is the
        // real review surface; this carries only the count + code (no exec).
        let mut details: HashMap<&str, &str> = HashMap::new();
        details.insert("polkit.message", message);

        let cancellation_id = "";
        let reply = self
            .bus
            .call_method(
                Some(AUTHORITY_DEST),
                AUTHORITY_PATH,
                Some(AUTHORITY_IFACE),
                "CheckAuthorization",
                &(
                    subject,
                    self.action_id.as_str(),
                    details,
                    ALLOW_USER_INTERACTION,
                    cancellation_id,
                ),
            )
            .map_err(|e| format!("polkit CheckAuthorization: {e}"))?;

        let result: AuthzResult = reply.body().deserialize().map_err(|e| format!("{e}"))?;
        Ok(result.is_authorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    #[test]
    fn subject_and_result_match_polkit_signatures() {
        // polkit Subject is (sa{sv}); AuthorizationResult is (bba{ss}).
        assert_eq!(Subject::SIGNATURE.to_string(), "(sa{sv})");
        assert_eq!(AuthzResult::SIGNATURE.to_string(), "(bba{ss})");
    }
}
