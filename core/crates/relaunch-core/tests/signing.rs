//! Chunk-1 tests: Ed25519 known-answer, canonical determinism, the validation
//! allowlist, and the parse/verify identity binding.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed unwrap/index is a failed assertion"
)]

use ed25519_dalek::VerifyingKey;
use relaunch_core::*;

fn seed(byte: u8) -> [u8; SEED_LEN] {
    [byte; SEED_LEN]
}

const ID: &str = "relauncher-00000000000000a1";

fn launcher_for(class: &str) -> Launcher {
    Launcher {
        path: format!("/home/laytonf/.config/omarchy-relaunch/launchers/{class}.desktop"),
        sha256: "ab".repeat(32),
    }
}

fn base_entry(class: &str) -> Entry {
    Entry {
        class: class.to_string(),
        window_class: "brave-browser".to_string(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: "brave-browser".to_string(),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: launcher_for(class),
        cwd: None,
        enabled: true,
    }
}

fn base_config() -> SignedConfig {
    SignedConfig {
        v: SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "laytonf".to_string(),
        gen: 1,
        pre_launch_ms: 0,
        post_launch_ms: 500,
        entries: vec![base_entry(ID)],
    }
}

// ---- Ed25519 ----

#[test]
fn rfc8032_section_7_1_test1() {
    // RFC 8032 §7.1 TEST 1, through our seed->key wiring: seed derives the
    // standard public key, and signing the empty message reproduces the
    // standard signature. Guards against a curve/encoding/param regression in
    // our code path that dalek's own suite would not exercise.
    use ed25519_dalek::Signer;
    let seed =
        hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
    let s: [u8; SEED_LEN] = seed.try_into().unwrap();
    let key = signing_key_from_seed(&s);
    assert_eq!(
        hex::encode(key.verifying_key().to_bytes()),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );
    assert_eq!(
        hex::encode(key.sign(b"").to_bytes()),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
         5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    );
}

#[test]
fn golden_key_and_signature() {
    // Golden vector pinning our wiring (seed derivation + canonical bytes +
    // Ed25519) against drift. ed25519-dalek carries the RFC 8032 conformance
    // vectors in its own suite, so this guards OUR encoding and key handling,
    // which is where a regression could hide. Seed = bytes 0..31.
    let s: [u8; SEED_LEN] = std::array::from_fn(|i| u8::try_from(i).unwrap());
    let key = signing_key_from_seed(&s);
    assert_eq!(
        hex::encode(key.verifying_key().to_bytes()),
        "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8"
    );
    let cfg = base_config();
    assert_eq!(
        hex::encode(sign(&key, &cfg)),
        "5c811a49ff4299abb3663dfdcf9b519bf424426ce0403fe318ca0b502ff7f512\
         952e636fcd508cc9bec1318baa3f85fc50ede87f4b608c8f36ceabd4ecd8b40e"
    );
}

#[test]
fn sign_then_verify_roundtrips() {
    let cfg = base_config();
    cfg.validate().unwrap();
    let key = signing_key_from_seed(&seed(7));
    let sig = sign(&key, &cfg);
    assert!(verify(&key.verifying_key(), &cfg, &sig));
}

#[test]
fn tampered_exec_fails_verification() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let sig = sign(&key, &cfg);
    let mut evil = cfg.clone();
    evil.entries[0].exec = "evil".to_string();
    assert!(!verify(&key.verifying_key(), &evil, &sig));
}

#[test]
fn tampered_enabled_fails_verification() {
    let mut cfg = base_config();
    cfg.entries[0].enabled = false;
    let key = signing_key_from_seed(&seed(7));
    let sig = sign(&key, &cfg);
    let mut evil = cfg.clone();
    evil.entries[0].enabled = true;
    assert!(!verify(&key.verifying_key(), &evil, &sig));
}

#[test]
fn wrong_key_fails_verification() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let sig = sign(&key, &cfg);
    let other = signing_key_from_seed(&seed(8));
    assert!(!verify(&other.verifying_key(), &cfg, &sig));
}

// ---- Canonical encoding ----

#[test]
fn canonical_is_deterministic_and_domain_prefixed() {
    let cfg = base_config();
    let a = canonical_bytes(&cfg);
    let b = canonical_bytes(&cfg);
    assert_eq!(a, b);
    assert!(a.starts_with(DOMAIN));
}

#[test]
fn canonical_differs_when_a_field_changes() {
    let cfg = base_config();
    let mut other = cfg.clone();
    other.entries[0].workspace = 2;
    assert_ne!(canonical_bytes(&cfg), canonical_bytes(&other));
}

#[test]
fn canonical_length_framing_blocks_boundary_confusion() {
    // "ab","c" vs "a","bc": same concatenation, different framed bytes.
    let mut one = base_config();
    one.entries[0].window_class = "ab".to_string();
    one.entries[0].label = "c".to_string();
    let mut two = base_config();
    two.entries[0].window_class = "a".to_string();
    two.entries[0].label = "bc".to_string();
    assert_ne!(canonical_bytes(&one), canonical_bytes(&two));
}

// ---- Validation allowlist ----

#[test]
fn duplicate_class_rejected() {
    let mut cfg = base_config();
    cfg.entries.push(base_entry(ID));
    cfg.sort_entries();
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::DuplicateClass(ID.into()))
    );
}

#[test]
fn unsorted_entries_rejected() {
    let mut cfg = base_config();
    cfg.entries = vec![
        base_entry("relauncher-00000000000000b2"),
        base_entry("relauncher-00000000000000a1"),
    ];
    assert_eq!(cfg.validate(), Err(ValidationError::EntriesNotSorted));
}

#[test]
fn workspace_zero_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].workspace = 0;
    assert_eq!(cfg.validate(), Err(ValidationError::Workspace(ID.into())));
}

#[test]
fn empty_exec_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec = String::new();
    assert_eq!(cfg.validate(), Err(ValidationError::EmptyExec(ID.into())));
}

#[test]
fn argv_source_without_host_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::Cmdline;
    cfg.entries[0].provenance = Provenance::None;
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::ArgvSourceNeedsHost(ID.into()))
    );
}

#[test]
fn argv_source_with_host_accepted() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::Guess;
    cfg.entries[0].provenance = Provenance::Host;
    cfg.validate().unwrap();
}

#[test]
fn non_argv_source_with_host_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::OverridesTable;
    cfg.entries[0].provenance = Provenance::Host;
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::NonArgvSourceHasHost(ID.into()))
    );
}

#[test]
fn class_must_be_a_pin_id() {
    for bad in [
        "brave-browser",
        "relauncher-00000000000000A1",
        "relauncher-00000000000000a",
        "relauncher-00000000000000a1x",
    ] {
        let mut cfg = base_config();
        cfg.entries[0] = base_entry(bad);
        assert_eq!(
            cfg.validate(),
            Err(ValidationError::ClassNotLauncherId(bad.into())),
            "{bad}"
        );
    }
}

#[test]
fn window_class_is_required_and_clean() {
    for bad in ["", "a\nb", &"x".repeat(MAX_CLASS_LEN + 1)] {
        let mut cfg = base_config();
        cfg.entries[0].window_class = bad.to_string();
        assert_eq!(cfg.validate(), Err(ValidationError::WindowClass(ID.into())));
    }
}

#[test]
fn launcher_must_be_the_pins_own_file_in_a_launchers_dir() {
    for bad in [
        "/usr/share/applications/relauncher-00000000000000a1.desktop",
        "/home/laytonf/.config/omarchy-relaunch/launchers/relauncher-00000000000000b2.desktop",
        "/home/laytonf/.config/omarchy-relaunch/launchers/sub/relauncher-00000000000000a1.desktop",
        "/home/laytonf/.config/omarchy-relaunch/launchersx/relauncher-00000000000000a1.desktop",
        "/home/laytonf/.config/omarchy-relaunch/launchers/relauncher-00000000000000a1.desktop.bak",
        "/home/laytonf/launchers.d/relauncher-00000000000000a1.desktop",
    ] {
        let mut cfg = base_config();
        cfg.entries[0].launcher.path = bad.into();
        assert_eq!(
            cfg.validate(),
            Err(ValidationError::LauncherName(ID.into())),
            "{bad}"
        );
    }
}

#[test]
fn dotdot_in_launcher_path_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].launcher.path =
        "/tmp/../home/x/omarchy-relaunch/launchers/relauncher-00000000000000a1.desktop".into();
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::LauncherPath(ID.into()))
    );
}

#[test]
fn launcher_sha256_must_be_lowercase_hex() {
    for bad in ["", "AB".repeat(32).as_str(), &"a".repeat(63)] {
        let mut cfg = base_config();
        cfg.entries[0].launcher.sha256 = bad.into();
        assert_eq!(cfg.validate(), Err(ValidationError::BadSha256(ID.into())));
    }
}

#[test]
fn every_source_carries_a_launcher() {
    for (src, prov) in [
        (ExecSource::DesktopFile, Provenance::None),
        (ExecSource::OverridesTable, Provenance::None),
        (ExecSource::Terminal, Provenance::Host),
        (ExecSource::Cmdline, Provenance::Host),
        (ExecSource::Guess, Provenance::Host),
    ] {
        let mut cfg = base_config();
        cfg.entries[0].exec_source = src;
        cfg.entries[0].provenance = prov;
        cfg.validate().unwrap();
    }
    let no_launcher = r#"{"class":"relauncher-00000000000000a1","windowClass":"x","workspace":1,
        "exec":"x","execSource":"overrides-table","provenance":"none","enabled":true}"#;
    assert!(serde_json::from_str::<Entry>(no_launcher).is_err());
}

#[test]
fn delays_are_bounded() {
    let mut cfg = base_config();
    cfg.post_launch_ms = MAX_DELAY_MS;
    cfg.pre_launch_ms = MAX_DELAY_MS;
    cfg.validate().unwrap();
    cfg.pre_launch_ms = MAX_DELAY_MS + 1;
    assert_eq!(cfg.validate(), Err(ValidationError::DelayTooLong));
    cfg.pre_launch_ms = 0;
    cfg.post_launch_ms = MAX_DELAY_MS + 1;
    assert_eq!(cfg.validate(), Err(ValidationError::DelayTooLong));
}

#[test]
fn canonical_binds_the_window_class_and_both_delays() {
    let cfg = base_config();
    for change in [
        (|c: &mut SignedConfig| c.entries[0].window_class = "foot".into()) as fn(&mut SignedConfig),
        |c| c.pre_launch_ms = 1,
        |c| c.post_launch_ms = 501,
        |c| c.entries[0].launcher.sha256 = "cd".repeat(32),
    ] {
        let mut other = cfg.clone();
        change(&mut other);
        assert_ne!(canonical_bytes(&cfg), canonical_bytes(&other));
    }
    let mut swapped = cfg.clone();
    swapped.pre_launch_ms = 500;
    swapped.post_launch_ms = 0;
    assert_ne!(canonical_bytes(&cfg), canonical_bytes(&swapped));
}

#[test]
fn the_working_directory_is_signed_and_must_be_absolute() {
    let cfg = base_config();
    let mut with = cfg.clone();
    with.entries[0].cwd = Some("/home/u".into());
    with.validate().unwrap();
    assert_ne!(canonical_bytes(&cfg), canonical_bytes(&with));
    let mut other = with.clone();
    other.entries[0].cwd = Some("/home/v".into());
    assert_ne!(canonical_bytes(&with), canonical_bytes(&other));
    for bad in ["", "relative", "/a\nb"] {
        let mut c = cfg.clone();
        c.entries[0].cwd = Some(bad.into());
        assert_eq!(
            c.validate(),
            Err(ValidationError::Cwd(ID.into())),
            "{bad:?}"
        );
    }
}

#[test]
fn a_v1_document_is_refused() {
    let mut cfg = base_config();
    cfg.v = 1;
    assert_eq!(cfg.validate(), Err(ValidationError::SchemaVersion(1)));
}

#[test]
fn control_char_in_class_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].class = "relauncher-00000000000000a\u{202e}".to_string();
    cfg.sort_entries();
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::ControlInClass(_))
    ));
}

// ---- parse_and_verify identity binding ----

fn signed_json(cfg: &SignedConfig, key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    let sig = sign(key, cfg);
    let file = to_config_file(cfg, &sig, vec![]);
    serde_json::to_vec(&file).unwrap()
}

#[test]
fn parse_and_verify_accepts_matching_identity() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let out = parse_and_verify(&json, &key.verifying_key(), 1000, "laytonf", 1).unwrap();
    assert_eq!(out.entries.len(), 1);
}

#[test]
fn parse_and_verify_rejects_wrong_uid() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let r = parse_and_verify(&json, &key.verifying_key(), 1001, "laytonf", 1);
    assert!(matches!(r, Err(VerifyError::UidMismatch { .. })));
}

#[test]
fn parse_and_verify_rejects_wrong_user() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let r = parse_and_verify(&json, &key.verifying_key(), 1000, "mallory", 1);
    assert!(matches!(r, Err(VerifyError::UserMismatch { .. })));
}

#[test]
fn parse_and_verify_rejects_stale_gen() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let r = parse_and_verify(&json, &key.verifying_key(), 1000, "laytonf", 2);
    assert!(matches!(r, Err(VerifyError::GenMismatch { .. })));
}

#[test]
fn parse_and_verify_rejects_tampered_json_after_signing() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let mut s = String::from_utf8(json).unwrap();
    // Flip the signed command without re-signing.
    s = s.replace("\"exec\":\"brave-browser\"", "\"exec\":\"evilcmdxxxxx1\"");
    let r = parse_and_verify(s.as_bytes(), &key.verifying_key(), 1000, "laytonf", 1);
    // Either the class/exec change breaks the signature, or (if it hit only
    // one copy) validation still holds but the signature must fail.
    assert!(
        matches!(r, Err(VerifyError::BadSignature)) || matches!(r, Err(VerifyError::Validation(_)))
    );
}

#[test]
fn parse_and_verify_rejects_unknown_field() {
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let json = signed_json(&cfg, &key);
    let mut v: serde_json::Value = serde_json::from_slice(&json).unwrap();
    v["surprise"] = serde_json::json!(true);
    let bytes = serde_json::to_vec(&v).unwrap();
    let r = parse_and_verify(&bytes, &key.verifying_key(), 1000, "laytonf", 1);
    assert!(matches!(r, Err(VerifyError::Json(_))));
}

#[test]
fn verifying_key_from_public_bytes() {
    // The anchor loads a raw 32-byte public key from root-owned public.json.
    let key = signing_key_from_seed(&seed(3));
    let pk_bytes = key.verifying_key().to_bytes();
    assert_eq!(pk_bytes.len(), PUBLIC_KEY_LEN);
    let reparsed = VerifyingKey::from_bytes(&pk_bytes).unwrap();
    let cfg = base_config();
    let sig = sign(&key, &cfg);
    assert!(verify(&reparsed, &cfg, &sig));
}

#[test]
fn pending_is_ignored_during_verify() {
    // A large `pending` array is present in config.json but must not be parsed
    // into a Vec before (or after) the signature check (reviewer #5); the signed
    // entries still verify.
    let cfg = base_config();
    let key = signing_key_from_seed(&seed(7));
    let sig = sign(&key, &cfg);
    let file = to_config_file(&cfg, &sig, vec![]);
    let mut v: serde_json::Value = serde_json::to_value(&file).unwrap();
    v["pending"] = serde_json::json!((0..10_000).collect::<Vec<u32>>());
    let bytes = serde_json::to_vec(&v).unwrap();
    let out = parse_and_verify(&bytes, &key.verifying_key(), 1000, "laytonf", 1).unwrap();
    assert_eq!(out.entries.len(), 1);
}

#[test]
fn entry_parse_rejects_duplicate_keys() {
    // relaunchctl forwards the proposal bytes unparsed, so THIS strict parse is
    // what catches a duplicate key; it must never be collapsed to "last wins".
    let launcher = r#""launcher":{"path":"/h/omarchy-relaunch/launchers/relauncher-00000000000000a1.desktop","sha256":"x"}"#;
    let dup = format!(
        r#"{{"class":"relauncher-00000000000000a1","class":"relauncher-00000000000000b2",
             "windowClass":"x","workspace":1,"exec":"x","execSource":"guess",
             "provenance":"host","enabled":true,{launcher}}}"#
    );
    assert!(serde_json::from_str::<Entry>(&dup).is_err());
    let dup_nested = r#"{"class":"relauncher-00000000000000a1","windowClass":"x","workspace":1,
                         "exec":"x","execSource":"desktop-file","provenance":"none","enabled":true,
                         "launcher":{"path":"/h/omarchy-relaunch/launchers/relauncher-00000000000000a1.desktop",
                                     "path":"/tmp/evil.desktop","sha256":"x"}}"#;
    assert!(serde_json::from_str::<Entry>(dup_nested).is_err());
}
