//! The stdio line protocol between the Relaunch panel and `relaunchctl`.
//!
//! The panel spawns `relaunchctl` once per transaction (spec §5.1) and talks to
//! it over the child's stdio. Every message in either direction is exactly one
//! line of compact JSON terminated by `\n`; multi-line content (the rendered
//! diff, a signed config document) rides inside JSON string values. The panel's
//! QML uses `SplitParser { splitMarker: "\n" }` on the child's stdout.
//!
//! The grammar is strict (Sol #13): a frame is complete only at its newline (an
//! unterminated tail at EOF is not a frame, so EOF is never a decision), no type
//! here accepts unknown fields, and serde's derive rejects a duplicate key. The
//! proposal is kept as its exact bytes and forwarded unparsed, so a duplicate
//! key inside it reaches the service's strict parse instead of being collapsed
//! by a client-side `Value`.
//!
//! **§5.1 / §12 invariant, structural:** the transaction id never crosses this
//! boundary. There is no `txid` field in any type here; the id lives only inside
//! the running `relaunchctl` process and on the D-Bus wire, never on stdout,
//! never on argv, never in a log line. The panel authorizes by sending a bare
//! `authorize` decision on the *same* child, which drives `Authorize(txid)` on
//! the held connection — it never names the id.

use relaunch_core::{json_error, MAX_REQUEST_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// Largest Prepare/Reduce frame, excluding its newline. The base travels as a
/// JSON string: JSON-escaping at most doubles a byte of the compact signed base
/// (`"` → `\"`, `\` → `\\`; it holds no raw control bytes). The frame also
/// carries the raw proposal (or the ops) and the envelope keys.
pub const MAX_REQUEST_FRAME: usize = 3 * MAX_REQUEST_BYTES + 4096;

/// Largest decision frame. A decision is one tiny fixed object.
pub const MAX_DECISION_FRAME: usize = 256;

/// First line the panel sends to `relaunchctl commit`: the verified signed base
/// (the user's current `config.json` text, empty string if none) and the
/// proposed new launch set. The base is a string because it is opaque signed
/// JSON the client must not reinterpret; the proposal is kept as raw bytes and
/// forwarded to the service, which validates and canonicalizes it (§6).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    /// The current signed `config.json` text, or empty.
    #[serde(default)]
    pub base: String,
    /// The proposed launch set, exactly as the panel sent it.
    pub proposal: Box<RawValue>,
}

/// Second line the panel sends to `relaunchctl commit`, after the user has read
/// the review on the panel.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Decision {
    /// Run the credential check and commit.
    Authorize,
    /// Withdraw the transaction.
    Cancel,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionFrame {
    decision: String,
}

/// The single line `relaunchctl reduce` reads: the verified signed base and the
/// reducing ops (`disable`/`drop` a class). Reductions are promptless (§5.4);
/// the service verifies `base` against the bus-caller uid and applies the ops as
/// a subset only.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReduceRequest {
    /// The current signed `config.json` text, or empty.
    #[serde(default)]
    pub base: String,
    /// The reducing operations to apply.
    pub ops: Vec<ReduceOp>,
}

/// One reducing operation on one class.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct ReduceOp {
    /// What to do.
    pub op: ReduceKind,
    /// The entry's class.
    pub class: String,
}

/// A reducing operation: it can only stop launches.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ReduceKind {
    /// Keep the entry but stop launching it.
    Disable,
    /// Remove the entry.
    Drop,
}

impl ReduceKind {
    /// The wire token the service's `Reduce` expects in each `(kind, class)` op.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ReduceKind::Disable => "disable",
            ReduceKind::Drop => "drop",
        }
    }
}

/// Every line `relaunchctl` writes to stdout. Serialized compact, one per line.
/// Note the absence of any transaction id: `Prepared` carries only the human
/// review surface (the service-rendered summary and diff), never the id the
/// panel would need to forge an `Authorize` out of band.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    /// Prepare succeeded; show `summary` and, under Details, `diff`, both
    /// verbatim (`PlainText`).
    Prepared {
        /// What the change means, in plain language.
        summary: String,
        /// The service-rendered review diff.
        diff: String,
    },
    /// Authorize/Reduce succeeded; `config` is the signed document text to persist.
    Committed {
        /// The signed `config.json` text (empty for a purge).
        config: String,
    },
    /// A status probe: an open transaction already exists for this uid (§4 —
    /// existence only, never another connection's code/diff/id).
    Pending {
        /// Whether a transaction is open for this uid.
        pending: bool,
    },
    /// The proposal's diff against the signed base was empty: nothing to
    /// authorize, no transaction minted (§3 step 2). Not a failure.
    Nothing,
    /// Any failure. `message` is safe to show; it never contains a code or id.
    Error {
        /// A human-readable reason.
        message: String,
    },
}

impl Event {
    /// Serialize as one stdout line (no embedded newline; multi-line content is
    /// JSON-escaped inside string values). Infallible: the payloads are plain
    /// strings/bools, so `serde_json` cannot fail here.
    #[must_use]
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"event":"error","message":"internal serialization error"}"#.to_string()
        })
    }
}

/// Parse one frame (without its newline) as a `PrepareRequest`.
///
/// # Errors
///
/// Not exactly one well-formed request object.
pub fn parse_prepare(frame: &[u8]) -> Result<PrepareRequest, String> {
    serde_json::from_slice(frame).map_err(|e| format!("bad prepare request: {}", json_error(&e)))
}

/// Parse one frame (without its newline) as a `Decision`.
///
/// # Errors
///
/// Not exactly one well-formed decision object, or an unknown decision.
pub fn parse_decision(frame: &[u8]) -> Result<Decision, String> {
    let f: DecisionFrame =
        serde_json::from_slice(frame).map_err(|e| format!("bad decision: {}", json_error(&e)))?;
    match f.decision.as_str() {
        "authorize" => Ok(Decision::Authorize),
        "cancel" => Ok(Decision::Cancel),
        _ => Err("bad decision: unknown decision".into()),
    }
}

/// Parse one frame (without its newline) as a `ReduceRequest`.
///
/// # Errors
///
/// Not exactly one well-formed request object.
pub fn parse_reduce(frame: &[u8]) -> Result<ReduceRequest, String> {
    serde_json::from_slice(frame).map_err(|e| format!("bad reduce request: {}", json_error(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errors_carry_no_request_bytes() {
        let secret = "SECRET".repeat(1000);
        for err in [
            parse_decision(format!(r#"{{"decision":"{secret}"}}"#).as_bytes()).unwrap_err(),
            parse_decision(format!(r#"{{"decision":"cancel","{secret}":1}}"#).as_bytes())
                .unwrap_err(),
            parse_prepare(format!(r#"{{"{secret}":1}}"#).as_bytes()).unwrap_err(),
            parse_reduce(format!(r#"{{"base":"","ops":[["{secret}"]]}}"#).as_bytes()).unwrap_err(),
        ] {
            assert!(!err.contains("SECRET") && err.len() < 100, "{err}");
        }
    }

    #[test]
    fn prepare_request_keeps_base_and_exact_proposal_bytes() {
        let proposal = r#"{ "preLaunchMs": 3,  "entries": [] }"#;
        let line = format!(r#"{{"base":"{{\"v\":2}}","proposal":{proposal}}}"#);
        let req = parse_prepare(line.as_bytes()).expect("parse");
        assert_eq!(req.base, r#"{"v":2}"#);
        assert_eq!(
            req.proposal.get(),
            proposal,
            "proposal must be forwarded byte-exact"
        );
    }

    #[test]
    fn duplicate_key_inside_proposal_survives_to_the_service() {
        // A client-side Value would silently keep only the last "entries".
        let proposal = r#"{"entries":[],"entries":[{"x":1}]}"#;
        let line = format!(r#"{{"proposal":{proposal}}}"#);
        let req = parse_prepare(line.as_bytes()).expect("parse");
        assert_eq!(req.proposal.get(), proposal);
    }

    #[test]
    fn prepare_request_rejects_duplicate_top_level_key() {
        assert!(parse_prepare(br#"{"base":"a","base":"b","proposal":{}}"#).is_err());
    }

    #[test]
    fn prepare_request_defaults_empty_base() {
        let req = parse_prepare(br#"{"proposal":{"entries":[]}}"#).expect("parse");
        assert_eq!(req.base, "");
    }

    #[test]
    fn prepare_request_rejects_unknown_field() {
        assert!(parse_prepare(br#"{"proposal":{},"txid":"deadbeef"}"#).is_err());
    }

    #[test]
    fn prepare_request_rejects_two_documents_in_one_frame() {
        assert!(parse_prepare(br#"{"proposal":{}} {"proposal":{}}"#).is_err());
    }

    #[test]
    fn decision_parses_both_arms() {
        assert_eq!(
            parse_decision(br#"{"decision":"authorize"}"#).unwrap(),
            Decision::Authorize
        );
        assert_eq!(
            parse_decision(br#"{"decision":"cancel"}"#).unwrap(),
            Decision::Cancel
        );
    }

    #[test]
    fn decision_rejects_unknown_arm_extra_field_and_duplicate_key() {
        assert!(parse_decision(br#"{"decision":"sign"}"#).is_err());
        assert!(parse_decision(br#"{"decision":"authorize","also":1}"#).is_err());
        assert!(parse_decision(br#"{"decision":"cancel","decision":"authorize"}"#).is_err());
    }

    #[test]
    fn reduce_request_parses_ops() {
        let line = br#"{"base":"x","ops":[{"op":"disable","class":"org.foo"},{"op":"drop","class":"org.bar"}]}"#;
        let req = parse_reduce(line).expect("parse");
        assert_eq!(req.ops.len(), 2);
        assert_eq!(req.ops[0].op.as_str(), "disable");
        assert_eq!(req.ops[1].op.as_str(), "drop");
        assert_eq!(req.ops[1].class, "org.bar");
    }

    #[test]
    fn reduce_request_rejects_unknown_op() {
        assert!(parse_reduce(br#"{"ops":[{"op":"delete","class":"x"}]}"#).is_err());
    }

    #[test]
    fn events_serialize_as_single_lines() {
        let diff = "line one\nline two\twith tab";
        let line = Event::Prepared {
            summary: "Relaunch will open these 2 apps at login:\n  A".into(),
            diff: diff.into(),
        }
        .to_line();
        assert_eq!(line.matches('\n').count(), 0);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["event"], "prepared");
        assert_eq!(
            v["summary"],
            "Relaunch will open these 2 apps at login:\n  A"
        );
        assert_eq!(v["diff"], diff);
    }

    #[test]
    fn no_event_variant_can_carry_a_txid() {
        // Structural §5.1 guard: no serialized event may contain a "txid" key.
        for line in [
            Event::Prepared {
                summary: "s".into(),
                diff: "d".into(),
            }
            .to_line(),
            Event::Committed { config: "c".into() }.to_line(),
            Event::Pending { pending: false }.to_line(),
            Event::Nothing.to_line(),
            Event::Error {
                message: "m".into(),
            }
            .to_line(),
        ] {
            assert!(!line.contains("txid"), "event line leaked a txid: {line}");
        }
    }

    #[test]
    fn frame_limits_derive_from_the_shared_request_cap() {
        const _: () = assert!(MAX_REQUEST_FRAME > 2 * MAX_REQUEST_BYTES);
        const _: () = assert!(MAX_DECISION_FRAME < 1024);
    }
}
