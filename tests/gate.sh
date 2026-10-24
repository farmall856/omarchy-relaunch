#!/usr/bin/env bash
# The full test gate. Every step must pass; the first failure stops the run.
#
#   rustfmt + clippy (-D warnings, all targets; all features, then the release
#   default features)  — rust-lint-policy.md
#   THIRD-PARTY-NOTICES.md lists exactly the direct dependencies
#   cargo test (whole workspace)
#   the fixture suite against the bash oracle AND the Rust engine (retired
#   sections run against the oracle only, each naming its Rust coverage)
#   the differential: oracle self-check, Rust vs goldens, LIVE bash vs Rust
#   the anchor stays environment-free (source and release binary)
#   the §11 restore check: oracle vs engine by identity + placement (fixture,
#   and LIVE on this desktop, sandboxed)
#   install.sh over the release core, black-box (tests/install-test.sh)
#   the AUR package owns files only under /usr (tests/pkgbuild-test.sh)
#   the polkit rule, executed (authorize and purge: AUTH_SELF or NO)
#   qmllint on the panel QML
#   the live key store (/var/lib/omarchy-relaunch/<uid>) is unchanged by it all
#   lint (shellcheck) STRICT on the shell that ships (the `relaunch` shim,
#   install.sh) and the test tooling; REPORT-ONLY on the bash oracle and the
#   suite.
set -euo pipefail
# No inherited override reaches a step: each suite names its own stand-ins, so
# a stray RELAUNCH_RELAUNCHCTL (or any other) cannot point a test at the
# installed service.
while IFS= read -r v; do unset "$v"; done < <(compgen -e | grep -E '^(RELAUNCH_|HYPRCTL$)')

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE="$ROOT/core"
# The bash engine, kept as the differential oracle until it is retired.
ORACLE="$ROOT/tests/oracle/relaunch"

# Shell that must stay shellcheck-clean.
SHELLCHECK_STRICT=(relaunch install.sh git-init.sh tests/gate.sh tests/check-notices.sh tests/capture-test.sh tests/hooks-test.sh tests/install-test.sh tests/anchor-lua-test.sh tests/live-list.sh tests/differential.sh tests/differential/world.sh tests/restore-parity.sh tests/pkgbuild-test.sh)
# Reported, not enforced (see header).
SHELLCHECK_REPORT=(tests/oracle/relaunch tests/relaunch-test.sh)

# Differential cases the Rust engine already implements (M1: install-signed,
# the boot flags in text mode, the shell/key probes; M2: the desktop index;
# M3: identity, trust, resolve, capture, labels; M4: propose; M5: the read side;
# M6: ensure-hooks, uninstall, the boot flags; M8: the suite's resolution
# sections as `suite:` scenarios).
# LIVE cases (this desktop's real XDG dirs and windows) run when hyprctl works.
# Live captures, the live index and the legacy resolver differ from bash by
# design (DLM: the generic resolver and web-app index, no terminal list);
# restore-parity.sh --live checks capture instead.
RUST_LIVE_CASES='^live:lookup:'
RUST_PORTED_CASES='^(install-signed|boot-|ensure-hooks|uninstall|probe-|index-dump|lookup:|capture:|resolve:|trust-|label-|propose$|list|snapshot|reload|last-boot|last-session|suite:)'

step() { printf '\n==> %s\n' "$1"; }

# The live key store must come through the gate untouched: no test may reach
# the installed authority service (an un-stubbed `relaunchctl purge` deletes
# this user's signing key, and the next login launches nothing).
LIVE_STORE="/var/lib/omarchy-relaunch/$(id -u)"
live_store() { # the store's public state and the key file's identity, or "absent"
  if [[ -e "$LIVE_STORE/public.json" ]]; then
    stat -c '%i %s %Y' "$LIVE_STORE/public.json"
    sha256sum <"$LIVE_STORE/public.json"
    # Unreadable (0600 root), but its inode, size and mtime are not.
    stat -c 'signing.key %i %s %Y' "$LIVE_STORE/signing.key" 2>/dev/null || printf 'signing.key absent\n'
  else
    printf 'absent\n'
  fi
}
LIVE_STORE_BEFORE="$(live_store)"

step "cargo fmt --check"
(cd "$CORE" && cargo fmt --all --check)

step "cargo clippy -D warnings"
(cd "$CORE" && cargo clippy --workspace --all-targets --all-features -- -D warnings)
(cd "$CORE" && cargo clippy --workspace --all-targets -- -D warnings)

step "third-party notices"
"$ROOT/tests/check-notices.sh"

step "cargo test (default features, then the engine's fixtures build)"
(cd "$CORE" && cargo test --workspace --quiet)
(cd "$CORE" && cargo test -p relaunch-engine --features fixtures --quiet)

step "fixture suite against the bash oracle (+ TxnLifecycle.js)"
RELAUNCH_ENGINE="$ORACLE" "$ROOT/tests/relaunch-test.sh"

step "fixture suite against the Rust engine"
(cd "$CORE" && cargo build -p relaunch-engine --features fixtures --quiet)
RELAUNCH_ENGINE="$CORE/target/debug/relaunch" "$ROOT/tests/relaunch-test.sh"

step "differential: bash oracle vs its goldens"
# The port gate itself (tests/differential.sh <rust engine>) joins at M1+.
(cd "$CORE" && cargo build -p relaunch-anchor --quiet)
"$ROOT/tests/differential.sh" --oracle "$ORACLE"

step "differential: Rust engine vs goldens (ported commands)"
# Grows each milestone until it covers every case (M8). Listed deltas must
# have reviewed expected outputs, or the gate fails.
(cd "$CORE" && cargo build -p relaunch-engine --features fixtures --quiet)
"$ROOT/tests/differential.sh" --cases="$RUST_PORTED_CASES" "$CORE/target/debug/relaunch"

step "capture / install-signed / propose and boot-wiring suites against the Rust engine"
RELAUNCH_ENGINE="$CORE/target/debug/relaunch" "$ROOT/tests/capture-test.sh"
RELAUNCH_ENGINE="$CORE/target/debug/relaunch" "$ROOT/tests/hooks-test.sh"

step "restores as completely as the oracle (spec §11): identity + placement"
(cd "$CORE" && cargo build -p relaunch-anchor --quiet)
"$ROOT/tests/restore-parity.sh" "$CORE/target/debug/relaunch" "$CORE/target/debug/relaunch-anchor"

step "install.sh, black-box over the release core (user namespace + overlays)"
(cd "$CORE" && cargo build --release --quiet)
"$ROOT/tests/install-test.sh" "$CORE/target/release"

step "the AUR package owns only /usr (Uninstall keeps the key store)"
"$ROOT/tests/pkgbuild-test.sh"

step "the root anchor loader (packaging/lua/anchor.lua), executed"
"$ROOT/tests/anchor-lua-test.sh"

step "the polkit rule (packaging/polkit), executed for both actions"
node --test "$ROOT/tests/polkit-rule.test.js" >/dev/null

step "differential: LIVE, bash vs Rust (ported cases)"
if hyprctl clients -j >/dev/null 2>&1; then
  "$ROOT/tests/differential.sh" --live --cases="$RUST_LIVE_CASES" "$ORACLE" "$CORE/target/debug/relaunch"
  "$ROOT/tests/live-list.sh" "$CORE/target/debug/relaunch"
  "$ROOT/tests/restore-parity.sh" --live "$CORE/target/debug/relaunch" "$CORE/target/debug/relaunch-anchor"
else
  printf '    skipped: no Hyprland session reachable\n'
fi

step "the anchor reads no environment override (D-P3)"
# Its only environment read is PATH (the one an argv's bare program name is
# searched on), and the release binary names no RELAUNCH_ variable or HYPRCTL
# at all: it runs launchers' argv directly, never through a shell.
reads="$(grep -rhoE 'env::var(_os)?\("[A-Z_]+"\)' "$CORE/crates/relaunch-anchor/src" | sort -u | tr '\n' ' ')"
[[ "$reads" == 'env::var("PATH") ' ]] || { printf 'relaunch-anchor reads: %s\n' "$reads" >&2; exit 1; }
(cd "$CORE" && cargo build --release -p relaunch-anchor --quiet)
names="$(strings "$CORE/target/release/relaunch-anchor" | { grep -oE 'RELAUNCH_[A-Z_]+|HYPRCTL' || true; } | sort -u | tr '\n' ' ')"
[[ -z "$names" ]] || { printf 'relaunch-anchor binary names: %s\n' "$names" >&2; exit 1; }
printf '    env reads: %s; RELAUNCH_/HYPRCTL names in the binary: %s\n' "$reads" "$names"

step "qmllint"
qmllint "$ROOT/Panel.qml" "$ROOT/BarWidget.qml" "$ROOT/Overlay.qml"

step "shellcheck (strict)"
(cd "$ROOT" && shellcheck "${SHELLCHECK_STRICT[@]}")

step "shellcheck (report only)"
for f in "${SHELLCHECK_REPORT[@]}"; do
  n="$(cd "$ROOT" && shellcheck -f gcc "$f" 2>/dev/null | wc -l || true)"
  printf '    %-24s %s findings\n' "$f" "$n"
done

step "the live key store is untouched"
[[ "$(live_store)" == "$LIVE_STORE_BEFORE" ]] \
  || { printf 'the gate changed the key store %s: was\n%s\nnow\n%s\n' "$LIVE_STORE" "$LIVE_STORE_BEFORE" "$(live_store)" >&2; exit 1; }
printf '    public.json %s\n    %s\n' "$(head -n1 <<<"$LIVE_STORE_BEFORE")" "$(tail -n1 <<<"$LIVE_STORE_BEFORE")"

printf '\ngate: ok\n'
