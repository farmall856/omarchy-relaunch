//! Guards on the shipped polkit data files so an accidental edit that would
//! weaken the authorization boundary fails the build. The live behaviour
//! (empower user still prompted, SSH denied) is hardware-tested later.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

const RULE: &str = include_str!("../../../../packaging/polkit/00-io.github.laytonf.relaunch.rules");
const POLICY: &str = include_str!("../../../../packaging/polkit/io.github.laytonf.relaunch.policy");

#[test]
fn rule_denies_nonlocal_or_inactive_and_requires_auth_self() {
    // Executed for every action by tests/polkit-rule.test.js; here, the shape.
    assert!(RULE.contains("io.github.laytonf.relaunch.authorize"));
    assert!(RULE.contains("io.github.laytonf.relaunch.purge"));
    assert!(RULE.contains("!subject.local || !subject.active"));
    assert!(RULE.contains("polkit.Result.NO"));
    assert!(RULE.contains("polkit.Result.AUTH_SELF"));
}

#[test]
fn rule_filename_sorts_before_empower() {
    // polkit 128 processes rules.d in lexical order of basename; "00-..." sorts
    // before "empower.rules" and "50-default.rules", so our rule decides first.
    assert!("00-io.github.laytonf.relaunch.rules" < "50-default.rules");
    assert!("00-io.github.laytonf.relaunch.rules" < "empower.rules");
}

/// The `<action id="ID">…</action>` block of the policy.
fn policy_action(id: &str) -> &'static str {
    let open = format!("<action id=\"{id}\">");
    let start = POLICY
        .find(&open)
        .unwrap_or_else(|| panic!("policy lacks {id}"));
    let len = POLICY[start..]
        .find("</action>")
        .expect("unterminated action");
    &POLICY[start..start + len]
}

#[test]
fn every_action_is_auth_self_active_only() {
    for id in [
        "io.github.laytonf.relaunch.authorize",
        "io.github.laytonf.relaunch.purge",
    ] {
        let action = policy_action(id);
        assert!(action.contains("<allow_any>no</allow_any>"), "{id}");
        assert!(
            action.contains("<allow_inactive>no</allow_inactive>"),
            "{id}"
        );
        assert!(
            action.contains("<allow_active>auth_self</allow_active>"),
            "{id}"
        );
        // Never a *_keep variant: every change, and every purge, is a fresh
        // authorization.
        assert!(!action.contains("_keep</"), "{id}");
    }
    assert_eq!(
        POLICY.matches("<action id=").count(),
        2,
        "an action no test covers"
    );
}

const UNIT: &str = include_str!("../../../../packaging/systemd/relaunchd.service");
const SERVICE_MAIN: &str = include_str!("../src/main.rs");
const ANCHOR_MAIN: &str = include_str!("../../relaunch-anchor/src/main.rs");

/// The key store is one fixed path for the service that writes it and the
/// user-run anchor that reads it at every login: systemd keeps the unit's
/// state directory at `/var/lib/<name>` only without `DynamicUser` (which
/// would move it under `/var/lib/private`, out of the anchor's reach).
#[test]
fn the_service_and_the_anchor_share_one_fixed_key_store() {
    let root = "const STATE_ROOT: &str = \"/var/lib/omarchy-relaunch\";";
    assert!(SERVICE_MAIN.contains(root), "relaunchd's store root");
    assert!(ANCHOR_MAIN.contains(root), "the anchor's store root");
    let settings: Vec<&str> = UNIT
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect();
    assert!(settings.contains(&"StateDirectory=omarchy-relaunch"));
    assert!(
        settings.contains(&"StateDirectoryMode=0755"),
        "the anchor must traverse it"
    );
    assert!(
        !settings.iter().any(|l| l.starts_with("DynamicUser")
            || l.starts_with("PrivateUsers")
            || l.starts_with("User=")),
        "the store would move or change owner"
    );
}

/// The unit fixes the service's umask, so nothing it creates depends on how it
/// was started; the store also sets each mode it needs explicitly.
#[test]
fn the_service_runs_with_umask_0022() {
    let settings: Vec<&str> = UNIT.lines().map(str::trim).collect();
    assert!(
        settings.contains(&"UMask=0022"),
        "relaunchd.service needs UMask=0022"
    );
}
