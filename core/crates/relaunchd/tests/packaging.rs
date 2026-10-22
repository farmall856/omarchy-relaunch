//! Guards on the shipped polkit data files so an accidental edit that would
//! weaken the authorization boundary fails the build. The live behaviour
//! (empower user still prompted, SSH denied) is hardware-tested later.

const RULE: &str = include_str!("../../../../packaging/polkit/00-io.github.laytonf.relaunch.rules");
const POLICY: &str = include_str!("../../../../packaging/polkit/io.github.laytonf.relaunch.policy");

#[test]
fn rule_denies_nonlocal_or_inactive_and_requires_auth_self() {
    assert!(RULE.contains("io.github.laytonf.relaunch.authorize"));
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

#[test]
fn policy_is_auth_self_active_only() {
    assert!(POLICY.contains("io.github.laytonf.relaunch.authorize"));
    assert!(POLICY.contains("<allow_any>no</allow_any>"));
    assert!(POLICY.contains("<allow_inactive>no</allow_inactive>"));
    assert!(POLICY.contains("<allow_active>auth_self</allow_active>"));
    // Never a *_keep variant: every change is a fresh authorization.
    assert!(!POLICY.contains("auth_self_keep"));
    assert!(!POLICY.contains("auth_admin_keep"));
}
