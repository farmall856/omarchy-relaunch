//! `config.json`: the strict signed-envelope check and the ONE writer (INV-3).
//!
//! The engine writes `config.json` in exactly one way: installing a signed
//! document that `relaunchd` produced and the panel relayed. There is no legacy
//! (unsigned) writer in the Rust engine (D-P4), so a signed document can never
//! be downgraded by a later write. Anything that is not exactly one well-typed
//! signed envelope is refused, and the existing file is left byte-unchanged.

use relaunch_core::{ConfigFile, FILE_FORMAT_VERSION, MAX_DOCUMENT_BYTES, SIGNED_SCHEMA_VERSION};

/// What an existing `config.json` holds.
#[derive(Debug)]
pub enum Kind {
    /// A strict signed envelope (structure only; the anchor verifies the
    /// signature).
    Signed(Box<ConfigFile>),
    /// A JSON object with neither `signed` nor `signature`: the pre-signing
    /// format, migration input only.
    Legacy,
    /// A signed envelope of an older signed schema (its version): nothing in
    /// it can be carried into the current schema; the next Save replaces it.
    Superseded(u32),
    /// Anything else, including a malformed signed-looking document.
    Invalid(String),
}

/// Classify a `config.json` body.
#[must_use]
pub fn classify(bytes: &[u8]) -> Kind {
    match parse_signed(bytes) {
        Ok(cfg) => Kind::Signed(Box::new(cfg)),
        Err(e) => match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(serde_json::Value::Object(m))
                if !m.contains_key("signed") && !m.contains_key("signature") =>
            {
                Kind::Legacy
            }
            Ok(serde_json::Value::Object(m)) => {
                let older = m
                    .get("signed")
                    .and_then(|s| s.get("v"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|v| (1..SIGNED_SCHEMA_VERSION).contains(v));
                match older {
                    Some(v) => Kind::Superseded(v),
                    None => Kind::Invalid(e),
                }
            }
            _ => Kind::Invalid(e),
        },
    }
}

/// Parse exactly one strict signed envelope (Sol #6): one JSON document, no
/// unknown or duplicate keys, supported envelope and schema versions, every
/// field typed, a 128-hex-digit signature, and a structurally valid signed
/// document. This is a shape check, not cryptographic verification.
///
/// # Errors
///
/// A message naming the first failed check.
pub fn parse_signed(bytes: &[u8]) -> Result<ConfigFile, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("document too large".into());
    }
    let cfg: ConfigFile =
        serde_json::from_slice(bytes).map_err(|e| format!("not a signed envelope: {e}"))?;
    if cfg.v != FILE_FORMAT_VERSION {
        return Err(format!("unsupported envelope version {}", cfg.v));
    }
    if cfg.signed.v != SIGNED_SCHEMA_VERSION {
        return Err(format!("unsupported signed schema {}", cfg.signed.v));
    }
    if cfg.signature.len() != 128
        || !cfg
            .signature
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("signature is not 128 lowercase hex digits".into());
    }
    cfg.signed
        .validate()
        .map_err(|e| format!("signed document invalid: {e:?}"))?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig() -> String {
        "0".repeat(128)
    }
    fn doc(v: u32, sv: u32, signature: &str) -> String {
        format!(
            r#"{{"v":{v},"signed":{{"v":{sv},"uid":1000,"user":"u","gen":1,"entries":[]}},"signature":"{signature}"}}"#
        )
    }

    #[test]
    fn a_well_formed_envelope_is_signed() {
        assert!(matches!(
            classify(doc(2, 2, &sig()).as_bytes()),
            Kind::Signed(_)
        ));
    }

    #[test]
    fn sol6_null_fields_and_empty_signature_are_refused() {
        assert!(parse_signed(br#"{"v":null,"signed":null,"signature":""}"#).is_err());
        assert!(parse_signed(doc(2, 2, "").as_bytes()).is_err());
        assert!(parse_signed(doc(2, 2, &"A".repeat(128)).as_bytes()).is_err());
    }

    #[test]
    fn sol6_a_multi_document_stream_is_refused() {
        let two = format!("{}\n{}", r#"{"x":1}"#, doc(2, 2, &sig()));
        assert!(parse_signed(two.as_bytes()).is_err());
        let trailing = format!("{} {{}}", doc(2, 2, &sig()));
        assert!(parse_signed(trailing.as_bytes()).is_err());
    }

    #[test]
    fn unsupported_versions_and_unknown_keys_are_refused() {
        assert!(parse_signed(doc(3, 1, &sig()).as_bytes()).is_err());
        assert!(parse_signed(doc(2, 3, &sig()).as_bytes()).is_err());
        assert!(parse_signed(doc(2, 1, &sig()).as_bytes()).is_err());
        let extra = doc(2, 2, &sig()).replacen('{', r#"{"surprise":true,"#, 1);
        assert!(parse_signed(extra.as_bytes()).is_err());
        let dup = doc(2, 2, &sig()).replacen('{', r#"{"v":2,"#, 1);
        assert!(parse_signed(dup.as_bytes()).is_err());
    }

    #[test]
    fn legacy_and_invalid_are_told_apart() {
        assert!(matches!(
            classify(br#"{"staggerSeconds":0,"entries":[]}"#),
            Kind::Legacy
        ));
        assert!(matches!(
            classify(br#"{"signed":{},"signature":"ab"}"#),
            Kind::Invalid(_)
        ));
        assert!(matches!(
            classify(doc(2, 1, &sig()).as_bytes()),
            Kind::Superseded(1)
        ));
        assert!(matches!(
            classify(doc(2, 3, &sig()).as_bytes()),
            Kind::Invalid(_)
        ));
        assert!(matches!(
            classify(doc(2, 0, &sig()).as_bytes()),
            Kind::Invalid(_)
        ));
        assert!(matches!(classify(b"not json"), Kind::Invalid(_)));
        assert!(matches!(classify(b"[]"), Kind::Invalid(_)));
    }
}
