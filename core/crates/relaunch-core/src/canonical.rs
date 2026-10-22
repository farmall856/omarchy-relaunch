//! Canonical byte encoding of a `SignedConfig`, the exact bytes that are
//! signed and verified.
//!
//! The encoding is explicit-length framing in a fixed field order, not JSON:
//! every string is preceded by its u64 length, so no value's contents can be
//! read as structure, and there is exactly one byte sequence per logical
//! document. A domain prefix carrying the schema version keeps these bytes from
//! ever being mistaken for another signing context.

use crate::model::{exec_source_tag, provenance_tag, Entry, SignedConfig};

/// Domain separation. The trailing newline and the explicit `/v1` pin both the
/// purpose and the schema version into the signed bytes.
pub const DOMAIN: &[u8] = b"io.github.laytonf.relaunch/config/v1\n";

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(DOMAIN);
        Writer { buf }
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn opt_bool(&mut self, v: Option<bool>) {
        match v {
            None => self.buf.push(0),
            Some(b) => {
                self.buf.push(1);
                self.bool(b);
            }
        }
    }
    fn entry(&mut self, e: &Entry) {
        self.str(&e.class);
        self.u32(e.workspace);
        self.opt_bool(e.float);
        self.str(&e.label);
        self.str(&e.exec);
        self.u32(exec_source_tag(e.exec_source));
        self.u32(provenance_tag(e.provenance));
        match &e.launcher {
            None => self.buf.push(0),
            Some(l) => {
                self.buf.push(1);
                self.str(&l.path);
                match &l.sha256 {
                    None => self.buf.push(0),
                    Some(h) => {
                        self.buf.push(1);
                        self.str(h);
                    }
                }
            }
        }
        self.bool(e.enabled);
    }
}

/// Serialize `cfg` to its canonical signed bytes. Entries are encoded in the
/// order they appear; callers sign/verify only after `validate`, which requires
/// them sorted and unique.
pub fn canonical_bytes(cfg: &SignedConfig) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(cfg.v);
    w.u32(cfg.uid);
    w.str(&cfg.user);
    w.u64(cfg.gen);
    w.u32(cfg.stagger_seconds);
    w.u64(cfg.entries.len() as u64);
    for e in &cfg.entries {
        w.entry(e);
    }
    w.buf
}
