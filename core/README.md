# relaunch core (security kernel)

Rust workspace for the authorization-gated redesign. Design:
`relaunch-authz-spec-final.md` (archived under `relauncher-design-archive/`).

Crates (built in reviewable chunks):

- `relaunch-core` — the signed launch document, its canonical byte encoding,
  and Ed25519 sign/verify. No I/O, no D-Bus, no polkit. The service, the login
  anchor and the client all build on this so they agree byte-for-byte on what
  was authorized. **(chunk 1, done)**
- `relaunchd` — root D-Bus service: Prepare/Authorize/Reduce/Purge, polkit,
  per-uid key, lease state machine, durable `gen` commit. *(chunk 2)*
- `relaunch-anchor` — user-mode login verifier + boot + placement sampling.
  *(chunk 4)*
- `relaunchctl` — the client the bash engine drives, holding one bus
  connection across the human pause. *(chunk 2/5)*

## Notes / deviations from the spec text

- **Signature encoding.** The spec prose says `base64(ed25519)`; this
  implementation stores the 64-byte signature as lowercase hex in
  `config.json`, to avoid a base64 dependency in the security core. Functionally
  identical; noted so a reviewer comparing against the spec is not surprised.
- **Crypto KAT.** `ed25519-dalek` ships the RFC 8032 conformance vectors in its
  own test suite, so `golden_key_and_signature` pins *our* wiring (seed
  handling + canonical bytes + signing) against drift rather than re-testing
  Ed25519 itself.

## Build

```
cd core
cargo test
cargo clippy --all-targets
```

Dependencies are pinned to exact versions in the workspace `Cargo.toml` and will
be vendored for the package build.
