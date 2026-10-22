//! Chunk-1 tests: Ed25519 known-answer, canonical determinism, the validation
//! allowlist, and the parse/verify identity binding.

use ed25519_dalek::VerifyingKey;
use relaunch_core::*;

fn seed(byte: u8) -> [u8; SEED_LEN] {
    [byte; SEED_LEN]
}

fn base_entry(class: &str) -> Entry {
    Entry {
        class: class.to_string(),
        workspace: 1,
        float: None,
        label: String::new(),
        exec: "brave-browser".to_string(),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: None,
        enabled: true,
    }
}

fn base_config() -> SignedConfig {
    SignedConfig {
        v: SIGNED_SCHEMA_VERSION,
        uid: 1000,
        user: "laytonf".to_string(),
        gen: 1,
        stagger_seconds: 0,
        entries: vec![base_entry("brave-browser")],
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
    let s: [u8; SEED_LEN] = std::array::from_fn(|i| i as u8);
    let key = signing_key_from_seed(&s);
    assert_eq!(
        hex::encode(key.verifying_key().to_bytes()),
        "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8"
    );
    let cfg = base_config();
    assert_eq!(
        hex::encode(sign(&key, &cfg)),
        "b33bdd286265f872a91cfe97c475ab582a656ba2f0883a27d07c2a25c472dd24\
         74bfd1e9672ba20a02bab8658a948a92ece2226ee7b043b70d2376a9f129ed00"
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
    one.entries[0].class = "ab".to_string();
    one.entries[0].label = "c".to_string();
    let mut two = base_config();
    two.entries[0].class = "a".to_string();
    two.entries[0].label = "bc".to_string();
    assert_ne!(canonical_bytes(&one), canonical_bytes(&two));
}

// ---- Validation allowlist ----

#[test]
fn duplicate_class_rejected() {
    let mut cfg = base_config();
    cfg.entries.push(base_entry("brave-browser"));
    cfg.sort_entries();
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::DuplicateClass("brave-browser".into()))
    );
}

#[test]
fn unsorted_entries_rejected() {
    let mut cfg = base_config();
    cfg.entries = vec![base_entry("zed"), base_entry("alacritty")];
    assert_eq!(cfg.validate(), Err(ValidationError::EntriesNotSorted));
}

#[test]
fn workspace_zero_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].workspace = 0;
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::Workspace("brave-browser".into()))
    );
}

#[test]
fn empty_exec_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec = String::new();
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::EmptyExec("brave-browser".into()))
    );
}

#[test]
fn argv_source_without_host_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::Cmdline;
    cfg.entries[0].provenance = Provenance::None;
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::ArgvSourceNeedsHost("brave-browser".into()))
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
        Err(ValidationError::NonArgvSourceHasHost(
            "brave-browser".into()
        ))
    );
}

#[test]
fn desktop_file_needs_launcher() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::DesktopFile;
    cfg.entries[0].launcher = None;
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::DesktopFileNeedsLauncher(
            "brave-browser".into()
        ))
    );
}

#[test]
fn root_prefix_launcher_needs_no_hash() {
    let mut cfg = base_config();
    cfg.entries[0].exec = "/usr/bin/brave".into();
    cfg.entries[0].exec_source = ExecSource::DesktopFile;
    cfg.entries[0].launcher = Some(Launcher {
        path: "/usr/share/applications/brave.desktop".into(),
        sha256: None,
    });
    cfg.validate().unwrap();
}

#[test]
fn user_launcher_without_hash_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::DesktopFile;
    cfg.entries[0].launcher = Some(Launcher {
        path: "/home/laytonf/.local/share/applications/x.desktop".into(),
        sha256: None,
    });
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::LauncherNeedsHashOrRootPrefix(
            "brave-browser".into()
        ))
    );
}

#[test]
fn dotdot_in_launcher_path_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::DesktopFile;
    cfg.entries[0].launcher = Some(Launcher {
        path: "/usr/share/applications/../../home/x.desktop".into(),
        sha256: None,
    });
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::LauncherPath("brave-browser".into()))
    );
}

#[test]
fn launcher_on_shell_source_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].exec_source = ExecSource::OverridesTable;
    cfg.entries[0].launcher = Some(Launcher {
        path: "/usr/share/applications/x.desktop".into(),
        sha256: None,
    });
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::LauncherOnShellSource(
            "brave-browser".into()
        ))
    );
}

#[test]
fn control_char_in_class_rejected() {
    let mut cfg = base_config();
    cfg.entries[0].class = "evil\u{202e}".to_string();
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
    s = s.replace("brave-browser", "evilcmdxxxxx1");
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
