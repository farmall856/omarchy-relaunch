#!/usr/bin/env bash
# packaging/lua/anchor.lua, EXECUTED under each available Lua: it evaluates
# only the verifier's output, as text, in an environment holding
# o.exec_on_start alone. The verifier is replaced by a fake that prints a
# chosen payload; the loader is otherwise byte-for-byte the shipped file.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }

LUAS=()
for l in lua5.4 luajit; do command -v "$l" >/dev/null 2>&1 && LUAS+=("$l"); done
((${#LUAS[@]})) || fail "no Lua interpreter"

ANCHOR_SRC="$ROOT/packaging/lua/anchor.lua"
grep -qF 'local ANCHOR = "/usr/lib/omarchy-relaunch/relaunch-anchor"' "$ANCHOR_SRC" \
  || fail "anchor.lua must name the root-owned verifier by its fixed path"
grep -q 'getenv' "$ANCHOR_SRC" && fail "anchor.lua must not read the environment"
# The one command it lets through is the one the verifier emits.
emitted="$(grep -oE 'o\.exec_on_start\(\\"[^\\]+\\"\)' "$ROOT/core/crates/relaunch-anchor/src/lua.rs" | sed -E 's/.*\(\\"(.*)\\"\)/\1/')"
grep -qxF "local BOOT = \"$emitted\"" "$ANCHOR_SRC" \
  || fail "anchor.lua's BOOT must be the emitter's boot registration ($emitted)"

cat >"$WORKDIR/harness.lua" <<'LUA'
local calls = {}
o = {
  window = function() calls[#calls + 1] = "LEAK window" end,
  exec_on_start = function(c) calls[#calls + 1] = "exec " .. c end,
  launch_on_start = function() calls[#calls + 1] = "LEAK launch_on_start" end,
}
local printed = {}
local real_print = print
print = function(m) printed[#printed + 1] = tostring(m) end
local ok, err = pcall(dofile, os.getenv("LOADER"))
print = real_print
if not ok then print("LOADER_ERROR " .. tostring(err)) end
for _, c in ipairs(calls) do print(c) end
for _, p in ipairs(printed) do print("PRINTED " .. p) end
if SIDE_EFFECT then print("SIDE_EFFECT " .. tostring(SIDE_EFFECT)) end
LUA

run() { # run <lua> <payload-file> -> harness output
  local lua="$1" payload="$2"
  # shellcheck disable=SC2016 # $1 belongs to the generated script
  printf '#!/bin/sh\n[ "$1" = emit-lua ] || exit 2\ncat %q\n' "$payload" >"$WORKDIR/fake-anchor"
  chmod +x "$WORKDIR/fake-anchor"
  sed "s#^local ANCHOR = .*#local ANCHOR = \"$WORKDIR/fake-anchor\"#" "$ANCHOR_SRC" >"$WORKDIR/anchor.lua"
  LOADER="$WORKDIR/anchor.lua" "$lua" "$WORKDIR/harness.lua" 2>&1
}
payload() { printf '%s\n' "$@" >"$WORKDIR/payload"; printf '%s' "$WORKDIR/payload"; }

for L in "${LUAS[@]}"; do
  out="$(run "$L" "$(payload '-- Generated' 'o.exec_on_start("/usr/lib/omarchy-relaunch/relaunch-anchor boot")')")"
  [[ "$out" == 'exec /usr/lib/omarchy-relaunch/relaunch-anchor boot' ]] \
    || fail "$L: a verified payload must register boot: $out"

  for bad in 'o.exec_on_start("rm -rf ~")' 'o.exec_on_start("/usr/lib/omarchy-relaunch/relaunch-anchor boot; x")'; do
    out="$(run "$L" "$(payload "$bad")")"
    ! grep -qE '^(exec|LEAK) ' <<<"$out" && [[ "$out" == *"PRINTED omarchy-relaunch: anchor output failed"*refused* ]] \
      || fail "$L: the wrapper must refuse what the verifier never emits ($bad): $out"
  done

  out="$(run "$L" "$(payload '-- relaunch-anchor: not launching')")"
  [[ -z "$out" ]] || fail "$L: a comment-only payload must do nothing: $out"

  for evil in "os.execute(\"touch $WORKDIR/pwned\")" "io.open(\"$WORKDIR/opened\", \"w\")" \
              'require("os")' 'SIDE_EFFECT = 1; _G.SIDE_EFFECT = 1' 'o.launch_on_start("x")' \
              'setmetatable(_ENV, {})' 'dofile("/etc/passwd")' \
              'o.window({ class = "^(x)$" }, { workspace = "1 silent" })'; do
    out="$(run "$L" "$(payload "$evil")")"
    [[ "$out" != *SIDE_EFFECT* && "$out" != *LEAK* && "$out" != *LOADER_ERROR* ]] \
      || fail "$L: the payload escaped its environment ($evil): $out"
    [[ "$out" == *"PRINTED omarchy-relaunch: anchor output failed"* ]] \
      || fail "$L: a payload reaching outside its environment must fail and be reported ($evil): $out"
  done
  [[ ! -e "$WORKDIR/pwned" && ! -e "$WORKDIR/opened" ]] || fail "$L: os.execute or io.open ran"

  "$L" -e 'io.write(string.dump(function() SIDE_EFFECT = 1 end))' >"$WORKDIR/bytecode"
  out="$(run "$L" "$WORKDIR/bytecode")"
  [[ "$out" == *"PRINTED omarchy-relaunch: anchor output did not load"* && "$out" != *SIDE_EFFECT* ]] \
    || fail "$L: bytecode must be refused: $out"

  out="$(run "$L" "$(payload 'this is not ) lua (')")"
  [[ "$out" == *"did not load"* ]] || fail "$L: a malformed payload must be reported: $out"

  out="$(o_missing=1 LOADER=/dev/null "$L" -e "o = nil; dofile('$WORKDIR/anchor.lua'); print('survived')" 2>&1)"
  [[ "$out" == survived ]] || fail "$L: without Omarchy's o the anchor must return quietly: $out"
done
printf 'ok %s (%s)\n' "$(basename "$0")" "${LUAS[*]}"
