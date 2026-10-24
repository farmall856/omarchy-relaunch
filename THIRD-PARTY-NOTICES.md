# Third-party notices

Relaunch's Rust binaries (`relaunchd`, `relaunch-anchor`, `relaunchctl`,
`relaunch`) are built with the crates below, each used unmodified under its own
license. This table lists every crate the workspace depends on directly; their
own dependencies are recorded in `core/Cargo.lock`. `tests/gate.sh` fails if a
direct dependency is missing here, listed at the wrong version or license, or
listed but no longer used.

| Crate | Version | License (SPDX) | Authors | Source |
|---|---|---|---|---|
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause | isis lovecruft, Tony Arcieri, and the dalek-cryptography contributors | https://github.com/dalek-cryptography/curve25519-dalek |
| `freedesktop-desktop-entry` | 0.8.4 | MPL-2.0 | Michael Aaron Murphy (System76 / pop-os) | https://github.com/pop-os/freedesktop-desktop-entry |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | The futures-rs contributors | https://github.com/rust-lang/futures-rs |
| `getrandom` | 0.3.3 | MIT OR Apache-2.0 | The Rand Project Developers | https://github.com/rust-random/getrandom |
| `hex` | 0.4.3 | MIT OR Apache-2.0 | KokaKiwi | https://github.com/KokaKiwi/rust-hex |
| `lexopt` | 0.3.2 | MIT | Jan Verbeek | https://github.com/blyxxyz/lexopt |
| `nix` | 0.31.3 | MIT | The nix-rust contributors | https://github.com/nix-rust/nix |
| `serde` | 1.0.228 | MIT OR Apache-2.0 | Erick Tryzelaar, David Tolnay | https://github.com/serde-rs/serde |
| `serde_json` | 1.0.145 | MIT OR Apache-2.0 | Erick Tryzelaar, David Tolnay | https://github.com/serde-rs/json |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | RustCrypto Developers | https://github.com/RustCrypto/hashes |
| `tokio` | 1.54.1 | MIT | Tokio Contributors | https://github.com/tokio-rs/tokio |
| `xdg` | 3.0.0 | Apache-2.0 OR MIT | Ben Longbons, whitequark | https://github.com/whitequark/rust-xdg |
| `zbus` | 5.19.0 | MIT | Zeeshan Ali Khan and the zbus contributors | https://github.com/z-galaxy/zbus |

## freedesktop-desktop-entry (MPL-2.0)

`relaunch-desktop` reads `.desktop` files with
[freedesktop-desktop-entry](https://github.com/pop-os/freedesktop-desktop-entry)
0.8.4 by System76, used unmodified. Its Source Code Form is available at that
repository and on crates.io (`freedesktop-desktop-entry` 0.8.4). The full
license text is retained in
[`LICENSES/freedesktop-desktop-entry/LICENSE`](LICENSES/freedesktop-desktop-entry/LICENSE).
