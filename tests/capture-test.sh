#!/usr/bin/env bash
# Capture (§8/§9), install-signed and propose, black-box against any engine:
# RELAUNCH_ENGINE selects it (default: the bash engine). Checks of bash
# internals run only against bash; checks of behaviour the bash engine lacks
# (Sol #6 strict envelope, the M4 propose contract) run only against Rust.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELAUNCH="${RELAUNCH_ENGINE:-$ROOT/tests/oracle/relaunch}"
# Absolute: some sections run from another directory.
RELAUNCH="$(realpath "$RELAUNCH")"
ENGINE_IS_BASH=false
[[ "$(head -c 64 "$RELAUNCH" 2>/dev/null | tr -d '\0')" == '#!'*bash* ]] && ENGINE_IS_BASH=true
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

export HYPRCTL="$WORKDIR/hyprctl"
cat >"$HYPRCTL" <<'EOF'
#!/usr/bin/env bash
[[ "${1:-}" == clients ]] && cat "${FAKE_CLIENTS:?}"
EOF
chmod +x "$HYPRCTL"
export RELAUNCH_CONFIG_DIR="$WORKDIR/cfg"

# --- §8/§9 capture_resolve: trust gate + provenance + launcher shape ---
# A stub anchor exercises capture_resolve's BRANCHING only (the real Exec
# parser has its own Rust golden suite): canonical-exec prints the Exec value
# and refuses a DBusActivatable/Terminal launcher, like the real subcommand.
if $ENGINE_IS_BASH; then  # bash internals; probe library
CAPLIB="$WORKDIR/caplib.sh"
head -n -1 "$RELAUNCH" >"$CAPLIB"
fi
STUB_ANCHOR="$WORKDIR/stub-anchor"
cat >"$STUB_ANCHOR" <<'EOF'
#!/usr/bin/env bash
[[ "${1:-}" == canonical-exec ]] || exit 2
f="${2:?}"; [[ -r "$f" ]] || exit 1
grep -qiE '^(DBusActivatable|Terminal)[[:space:]]*=[[:space:]]*true' "$f" && exit 1
e="$(sed -n 's/^Exec=//p' "$f" | head -n1)"; [[ -n "$e" ]] || exit 1
printf '%s\n' "$e"
EOF
chmod +x "$STUB_ANCHOR"

CAPDATA="$WORKDIR/capdata/applications"
mkdir -p "$CAPDATA"
cat >"$CAPDATA/myapp.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=MyApp
StartupWMClass=MyApp
Exec=myapp --flag
EOF
cat >"$CAPDATA/dbusapp.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=DbusApp
StartupWMClass=DbusApp
Exec=dbusapp
DBusActivatable=true
EOF

CAPNS="$WORKDIR/capns"
mkdir -p "$CAPNS"
printf 'foot\0--app-id=herdr\0-e\0herdr\0' >"$CAPNS/300"   # terminal wrapper
printf 'myapp\0--flag\0'                   >"$CAPNS/301"   # plain argv window
printf 'foreign\n'                         >"$CAPNS/400.ns" # a sandboxed pid

if $ENGINE_IS_BASH; then  # bash internals; Rust: capture unit tests; propose below stays black-box
cap() {
  # cap <class> <pid> <xwayland> -> capture_resolve JSON on stdout
  RELAUNCH_ANCHOR_BIN="$STUB_ANCHOR" RELAUNCH_CMDLINE_DIR="$CAPNS" \
  RELAUNCH_DATA_DIRS="$WORKDIR/capdata" \
    bash -c 'source "$1"; capture_resolve "$2" "$3" "$4"' _ "$CAPLIB" "$1" "$2" "$3"
}
capget() { jq -r "$2" <<<"$(cap "$1" "$3" "$4")"; }

# Trusted (host fixture) terminal host: class becomes the wrapper's app-id.
[[ "$(cap herdr 300 false | jq -r .execSource)" == terminal ]] \
  || fail "trusted terminal must resolve execSource=terminal: $(cap herdr 300 false)"
[[ "$(cap herdr 300 false | jq -r .provenance)" == host ]] \
  || fail "terminal provenance must be host"

# Trusted desktop-file (non-root-prefix fixture path => sha256 present, none prov).
MY="$(cap MyApp 301 false)"
[[ "$(jq -r .execSource <<<"$MY")" == desktop-file ]] || fail "MyApp must be desktop-file: $MY"
[[ "$(jq -r .provenance <<<"$MY")" == none ]]         || fail "desktop-file provenance must be none: $MY"
[[ "$(jq -r .launcher.path <<<"$MY")" == "$CAPDATA/myapp.desktop" ]] || fail "launcher.path wrong: $MY"
[[ "$(jq -r '.launcher.sha256|length' <<<"$MY")" == 64 ]] || fail "non-root launcher needs a 64-hex sha256: $MY"
[[ "$(jq -r .exec <<<"$MY")" == "myapp --flag" ]] || fail "desktop-file exec must be the canonical Exec: $MY"

# Trusted window whose .desktop is DBusActivatable: desktop-file refused, falls
# through to the argv (cmdline) host source.
DB="$(cap DbusApp 301 false)"
[[ "$(jq -r .execSource <<<"$DB")" == cmdline ]] || fail "DBusActivatable must fall through to cmdline: $DB"
[[ "$(jq -r .provenance <<<"$DB")" == host ]]    || fail "fall-through provenance must be host: $DB"

# Trusted window, class unknown to the index, no argv: guess + host, one word.
GUESS="$(RELAUNCH_ANCHOR_BIN="$STUB_ANCHOR" RELAUNCH_CMDLINE_DIR="$CAPNS" RELAUNCH_DATA_DIRS="$WORKDIR/capdata" \
  bash -c 'source "$1"; capture_resolve "Unknown.Thing" 302 false' _ "$CAPLIB")"
[[ "$(jq -r .execSource <<<"$GUESS")" == guess ]]   || fail "unknown trusted class must guess: $GUESS"
[[ "$(jq -r .provenance <<<"$GUESS")" == host ]]    || fail "guess provenance must be host: $GUESS"
[[ "$(jq -r .exec <<<"$GUESS")" == "unknown.thing" ]] || fail "guess exec must be the quoted lowercase class: $GUESS"

# Untrusted (foreign namespace) window, class unknown: pending/unverified, not eligible.
PEND="$(cap Unknown.Thing 400 false)"
[[ "$(jq -r .eligible <<<"$PEND")" == false ]]       || fail "untrusted unknown must be ineligible: $PEND"
[[ "$(jq -r .execSource <<<"$PEND")" == unverified ]] || fail "untrusted pending execSource=unverified: $PEND"
[[ "$(jq -r .provenance <<<"$PEND")" == none ]]       || fail "pending provenance must be none: $PEND"

# Untrusted (XWayland) window: same pending result even on a host pid.
PENDX="$(cap Unknown.Thing 301 true)"
[[ "$(jq -r .eligible <<<"$PENDX")" == false ]] || fail "XWayland unknown must be ineligible: $PENDX"

fi
# --- install-signed: accepts a signed envelope, refuses everything else ---
ISDIR="$WORKDIR/signed-cfg"
SIG="$(printf '0%.0s' {1..128})"
SIGNED_DOC='{"v":2,"signed":{"v":2,"uid":1000,"user":"u","gen":1,"preLaunchMs":0,"postLaunchMs":500,"entries":[]},"signature":"'"$SIG"'"}'
printf '%s' "$SIGNED_DOC" | RELAUNCH_CONFIG_DIR="$ISDIR" "$RELAUNCH" install-signed >/dev/null \
  || fail "install-signed must accept a signed-config envelope"
[[ "$(cat "$ISDIR/config.json")" == "$SIGNED_DOC" ]] || fail "install-signed must persist the document verbatim"
# A legacy unsigned config must NOT clobber an installed signed one.
if printf '{"staggerSeconds":0,"entries":[]}' \
    | RELAUNCH_CONFIG_DIR="$ISDIR" "$RELAUNCH" install-signed >/dev/null 2>&1; then
  fail "install-signed must refuse an unsigned document"
fi
[[ "$(cat "$ISDIR/config.json")" == "$SIGNED_DOC" ]] || fail "a refused install-signed must leave config.json untouched"
printf 'garbage' | RELAUNCH_CONFIG_DIR="$ISDIR" "$RELAUNCH" install-signed >/dev/null 2>&1 \
  && fail "install-signed must refuse non-JSON"

# --- install-signed writes through links and is born private ---
LNK="$WORKDIR/links"
mkdir -p "$LNK/dotfiles" "$LNK/cfg"
ln -s "$LNK/dotfiles/config.json" "$LNK/cfg/config.json"
printf '%s' "$SIGNED_DOC" | RELAUNCH_CONFIG_DIR="$LNK/cfg" "$RELAUNCH" install-signed >/dev/null \
  || fail "install-signed failed through a dangling symlinked config.json"
[[ -L "$LNK/cfg/config.json" ]] || fail "install-signed replaced a symlinked config.json with a regular file"
[[ "$(cat "$LNK/dotfiles/config.json")" == "$SIGNED_DOC" ]] || fail "install-signed must write the link target"
ln -s "$LNK/dotfiles/config.json" "$LNK/mid.json"
mkdir -p "$LNK/cfg2"; ln -s "$LNK/mid.json" "$LNK/cfg2/config.json"
printf '%s' "$SIGNED_DOC" | RELAUNCH_CONFIG_DIR="$LNK/cfg2" "$RELAUNCH" install-signed >/dev/null
[[ -L "$LNK/cfg2/config.json" && -L "$LNK/mid.json" ]] || fail "a symlink chain must stay a chain"
[[ "$(stat -c '%a' "$LNK/dotfiles/config.json")" == 600 ]] || fail "the signed document must be 0600"
( umask 000; printf '%s' "$SIGNED_DOC" | RELAUNCH_CONFIG_DIR="$LNK/fresh" "$RELAUNCH" install-signed >/dev/null )
[[ "$(stat -c '%a' "$LNK/fresh")" == 700 ]] || fail "install-signed must create the config dir 0700"
[[ "$(stat -c '%a' "$LNK/fresh/config.json")" == 600 ]] || fail "install-signed must create config.json 0600"

# --- propose: classified candidates + base gate ---
PROPC="$WORKDIR/propose-clients.json"
cat >"$PROPC" <<'EOF'
[
  {"address":"0xaa","pid":301,"initialClass":"MyApp","workspace":{"id":2},"xwayland":false},
  {"address":"0xbb","pid":400,"initialClass":"Sandboxed.App","workspace":{"id":3},"xwayland":false}
]
EOF
PROPOUT="$(FAKE_CLIENTS="$PROPC" RELAUNCH_CONFIG_DIR="$WORKDIR/propose-empty" \
  RELAUNCH_ANCHOR_BIN="$STUB_ANCHOR" RELAUNCH_CMDLINE_DIR="$CAPNS" RELAUNCH_DATA_DIRS="$WORKDIR/capdata" \
  "$RELAUNCH" propose --json)"
[[ "$(jq -r '.candidates|length' <<<"$PROPOUT")" == 2 ]] || fail "propose must emit one candidate per window: $PROPOUT"
[[ "$(jq -r '.base' <<<"$PROPOUT")" == "" ]] || fail "propose base must be empty with no signed config: $PROPOUT"
# MyApp (pid 301 host, .desktop present). bash prefers the launcher; the Rust
# engine keeps the window's own honest command line, which runs the same
# program as the launcher (DLM).
if $ENGINE_IS_BASH; then want_src=desktop-file; else want_src=cmdline; fi
[[ "$(jq -r '.candidates[]|select(.class=="MyApp")|.execSource' <<<"$PROPOUT")" == "$want_src" ]] \
  || fail "propose MyApp must be $want_src: $PROPOUT"
[[ "$(jq -r '.candidates[]|select(.class=="MyApp")|.workspace' <<<"$PROPOUT")" == 2 ]] \
  || fail "propose must carry the live workspace: $PROPOUT"
# Sandboxed.App (pid 400 foreign ns) -> ineligible pending.
[[ "$(jq -r '.candidates[]|select(.class=="Sandboxed.App")|.eligible' <<<"$PROPOUT")" == false ]] \
  || fail "propose must mark an untrusted window ineligible: $PROPOUT"

# snapshot is born private too (it records window titles).
echo '[{"address":"0x1","pid":301,"initialClass":"MyApp","workspace":{"id":2},"title":"secret"}]' >"$WORKDIR/snap-clients.json"
( umask 022; FAKE_CLIENTS="$WORKDIR/snap-clients.json" RELAUNCH_CONFIG_DIR="$WORKDIR/snapcfg" "$RELAUNCH" snapshot >/dev/null ) \
  || fail "snapshot failed"
[[ "$(stat -c '%a' "$WORKDIR/snapcfg")" == 700 ]] || fail "snapshot must create the config dir 0700"
[[ "$(stat -c '%a' "$WORKDIR/snapcfg/last-session.json")" == 600 ]] || fail "last-session.json must be 0600"
if ! $ENGINE_IS_BASH; then  # Sol S1: a diagnostic artifact replaces a planted link, never writes through it
  printf 'victim\n' >"$WORKDIR/snap-victim"
  rm "$WORKDIR/snapcfg/last-session.json"; ln -s "$WORKDIR/snap-victim" "$WORKDIR/snapcfg/last-session.json"
  FAKE_CLIENTS="$WORKDIR/snap-clients.json" RELAUNCH_CONFIG_DIR="$WORKDIR/snapcfg" "$RELAUNCH" snapshot >/dev/null \
    || fail "snapshot failed over a planted link"
  [[ "$(cat "$WORKDIR/snap-victim")" == victim && -f "$WORKDIR/snapcfg/last-session.json" && ! -L "$WORKDIR/snapcfg/last-session.json" ]] \
    || fail "snapshot must replace a planted last-session.json link, not write through it"
fi

# propose base reflects an installed signed config.
PROPOUT2="$(FAKE_CLIENTS="$PROPC" RELAUNCH_CONFIG_DIR="$ISDIR" \
  RELAUNCH_ANCHOR_BIN="$STUB_ANCHOR" RELAUNCH_CMDLINE_DIR="$CAPNS" RELAUNCH_DATA_DIRS="$WORKDIR/capdata" \
  "$RELAUNCH" propose --json)"
[[ "$(jq -r '.base' <<<"$PROPOUT2")" == "$SIGNED_DOC" ]] || fail "propose must pass the signed config as base: $PROPOUT2"


if ! $ENGINE_IS_BASH; then  # behaviour the bash engine lacks (deltas D6, DOK, D12, DWS)
# Sol #6: exactly one strict envelope; the existing file is left byte-unchanged.
for bad in '{"v":null,"signed":null,"signature":""}' \
           "$SIGNED_DOC"$'\n'"$SIGNED_DOC" \
           '{"v":2,"signed":{"v":2,"uid":1000,"user":"u","gen":1,"entries":[]},"signature":"ab"}' \
           '{"v":2,"signed":{"v":1,"uid":1000,"user":"u","gen":1,"entries":[]},"signature":"'"$SIG"'"}'; do
  if printf '%s' "$bad" | RELAUNCH_CONFIG_DIR="$ISDIR" "$RELAUNCH" install-signed >/dev/null 2>&1; then
    fail "install-signed must refuse ${bad@Q}"
  fi
  [[ "$(cat "$ISDIR/config.json")" == "$SIGNED_DOC" ]] || fail "a refused relay changed config.json: ${bad@Q}"
done

# The propose contract the panel consumes: ok, base and proposalJson as text.
[[ "$(jq -c 'keys' <<<"$PROPOUT")" == '["base","candidates","dropping","entries","kind","merged","ok","postLaunchMs","preLaunchMs","proposalJson"]' ]] \
  || fail "propose keys: $PROPOUT"
[[ "$(jq -r '.ok' <<<"$PROPOUT")" == true ]] || fail "propose must say ok:true: $PROPOUT"
jq -e '.proposalJson | fromjson | .entries | map(.windowClass) == ["MyApp"]' >/dev/null <<<"$PROPOUT" \
  || fail "the proposal must hold exactly the eligible window: $PROPOUT"

# Sol #12 / Sol #9 / DWS through the full propose path.
cat >"$PROPC" <<'EOF'
[
  {"address":"0x1","pid":901,"initialClass":"Back\\slash","workspace":{"id":2},"xwayland":false},
  {"address":"0x2","pid":902,"initialClass":"__proto__","workspace":{"id":3},"xwayland":false},
  {"address":"0x3","pid":903,"initialClass":"constructor","workspace":{"id":3},"xwayland":false},
  {"address":"0x4","pid":904,"initialClass":"toString","workspace":{"id":3},"xwayland":false},
  {"address":"0x5","pid":905,"initialClass":"Scratch","workspace":{"id":-98},"xwayland":false}
]
EOF
# Each window has a command line of its own: a window with neither that nor a
# launcher is an unsigned guess and never proposed.
for pid in 901 902 903 904 905; do printf '%s\0' "/opt/bin/app$pid" >"$CAPNS/$pid"; done
PROPOUT3="$(FAKE_CLIENTS="$PROPC" RELAUNCH_CONFIG_DIR="$WORKDIR/propose-empty" \
  RELAUNCH_CMDLINE_DIR="$CAPNS" RELAUNCH_DATA_DIRS="$WORKDIR/capdata" "$RELAUNCH" propose --json)"
jq -e '.proposalJson | fromjson | .entries | map(.windowClass) | sort == ["Back\\slash","__proto__","constructor","toString"]' \
  >/dev/null <<<"$PROPOUT3" || fail "class bytes must survive propose exactly: $PROPOUT3"
jq -e '.candidates[] | select(.class == "Scratch") | .proposed == false and .reason == "special-workspace"' \
  >/dev/null <<<"$PROPOUT3" || fail "a special-workspace window must be shown, not proposed: $PROPOUT3"

# Capture keys a pin by (window class, workspace, command): three foots on
# workspace 6 running different commands are three pins; two identical foots
# on workspace 2 are one, and the other is named as merged. Re-capturing the
# signed result proposes it unchanged.
cat >"$PROPC" <<'EOF'
[
  {"address":"0x10","pid":910,"initialClass":"foot","workspace":{"id":6},"xwayland":false},
  {"address":"0x11","pid":911,"initialClass":"foot","workspace":{"id":6},"xwayland":false},
  {"address":"0x12","pid":912,"initialClass":"foot","workspace":{"id":6},"xwayland":false},
  {"address":"0x13","pid":913,"initialClass":"foot","workspace":{"id":2},"xwayland":false},
  {"address":"0x14","pid":914,"initialClass":"foot","workspace":{"id":2},"xwayland":false}
]
EOF
printf '%s\0' /usr/bin/foot >"$CAPNS/910"
printf '%s\0' foot -e cliamp >"$CAPNS/911"
printf '%s\0' foot -e htop >"$CAPNS/912"
printf '%s\0' foot >"$CAPNS/913"
printf '%s\0' foot >"$CAPNS/914"
term_propose() {
  FAKE_CLIENTS="$PROPC" RELAUNCH_CONFIG_DIR="$WORKDIR/terminals" \
    RELAUNCH_CMDLINE_DIR="$CAPNS" RELAUNCH_DATA_DIRS="$WORKDIR/capdata" "$RELAUNCH" propose --json
}
TERMS="$(term_propose)"
jq -e '.proposalJson | fromjson | [.entries[] | select(.workspace == 6) | .exec] | sort
       == ["/usr/bin/foot", "foot -e cliamp", "foot -e htop"]' >/dev/null <<<"$TERMS" \
  || fail "each command on a shared workspace must be its own pin: $TERMS"
jq -e '[.proposalJson | fromjson | .entries[] | select(.workspace == 2)] | length == 1' >/dev/null <<<"$TERMS" \
  || fail "identical windows must be one pin: $TERMS"
jq -e '.merged == [{"label": "foot", "workspace": 2}]
       and ([.candidates[] | select(.reason == "duplicate-window") | .addr] == ["0x14"])' >/dev/null <<<"$TERMS" \
  || fail "the merged duplicate must be named: $TERMS"
jq -c '{v: 2, signed: ({v: 2, uid: 1000, user: "u", gen: 1} + (.proposalJson | fromjson)), signature: ("0" * 128)}' \
  <<<"$TERMS" | RELAUNCH_CONFIG_DIR="$WORKDIR/terminals" "$RELAUNCH" install-signed >/dev/null \
  || fail "install-signed refused the terminals proposal"
AGAIN="$(term_propose)"
[[ "$(jq -c '.proposalJson | fromjson' <<<"$AGAIN")" == "$(jq -c '.proposalJson | fromjson' <<<"$TERMS")" ]] \
  || fail "re-capturing unchanged terminals must propose the signed list unchanged: $AGAIN"
fi

printf 'ok %s\n' "$(basename "$0")"
