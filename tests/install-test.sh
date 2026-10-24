#!/usr/bin/env bash
# The real install.sh, black-box, over the release binaries placed where the
# system package puts them. `unshare -rm` maps us to root in a private mount
# namespace with /usr/lib and /usr/share overlaid on scratch directories and a
# private tmpfs on each core directory (an installed package's own copy is not
# ours to replace), so the core is root-owned in there while the real /usr
# never changes; HOME is a sandbox. Proves what install.sh leaves on the login path: the anchor line,
# no legacy loader, rules.lua an inspection copy -- and that it refuses, writing
# nothing, while any part of the core is missing or not root-owned.
#
#   tests/install-test.sh [release-dir]   (default core/target/release)
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REL="$(realpath "${1:-$ROOT/core/target/release}")"
ANCHOR_LUA=/usr/share/omarchy-relaunch/anchor.lua
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
for b in relaunch relaunchctl relaunch-anchor; do
  [[ -x "$REL/$b" ]] || fail "$REL/$b is missing (cargo build --release)"
done
WORKDIR="$(mktemp -d)"
trap 'chmod -R u+rwx "$WORKDIR" 2>/dev/null; rm -rf "$WORKDIR"' EXIT

LEGACY_HOOK='local _c = os.getenv("XDG_CONFIG_HOME"); if _c == nil or _c == "" then _c = os.getenv("HOME") .. "/.config" end; local _rl = _c .. "/omarchy-relaunch/relaunch.lua"; local _f = io.open(_rl); if _f then _f:close(); dofile(_rl) end'

# Run inside the namespace: mount the overlays, install the core as the
# package would, apply one fault, run install.sh as a fresh login would.
cat >"$WORKDIR/inner.sh" <<'EOF'
set -euo pipefail
S="$1" REL="$2" ROOT="$3" FAULT="$4"
for d in lib share; do
  mkdir -p "$S/$d-up" "$S/$d-work"; chmod 0755 "$S/$d-up"
  mount -t overlay overlay -o "lowerdir=/usr/$d,upperdir=$S/$d-up,workdir=$S/$d-work" "/usr/$d"
  mkdir -p "/usr/$d/omarchy-relaunch"
  mount -t tmpfs -o mode=0755 tmpfs "/usr/$d/omarchy-relaunch"
done
for b in relaunch relaunchctl relaunch-anchor; do
  install -m 0755 "$REL/$b" "/usr/lib/omarchy-relaunch/$b"
done
install -m 0644 "$ROOT/packaging/lua/anchor.lua" /usr/share/omarchy-relaunch/anchor.lua
case "$FAULT" in
  none) ;;
  no-anchor-lua) rm /usr/share/omarchy-relaunch/anchor.lua ;;
  anchor-lua-writable) chmod 0664 /usr/share/omarchy-relaunch/anchor.lua ;;
  anchor-dir-writable) chmod 0775 /usr/share/omarchy-relaunch ;;
  anchor-lua-symlink) mv /usr/share/omarchy-relaunch/anchor.lua /usr/share/omarchy-relaunch/real.lua
    ln -s real.lua /usr/share/omarchy-relaunch/anchor.lua ;;
  no-relaunch-anchor) rm /usr/lib/omarchy-relaunch/relaunch-anchor ;;
  lib-dir-writable) chmod 0775 /usr/lib/omarchy-relaunch ;;
  *) echo "unknown fault $FAULT" >&2; exit 2 ;;
esac
rc=0
env -i HOME="$S/home" PATH=/usr/bin:/bin bash "$ROOT/install.sh" >"$S/out" 2>&1 || rc=$?
echo "$rc" >"$S/rc"
(cd /usr && find lib/omarchy-relaunch share/omarchy-relaunch -mindepth 1 | sort | tr '\n' ' ') >"$S/core"
EOF

# scenario <name> <fault>: a fresh namespace over a home holding a legacy install.
scenario() {
  S="$WORKDIR/$1"; H="$S/home"
  mkdir -p "$H/.config/hypr" "$H/.config/omarchy-relaunch"
  printf '%s\n' 'require("hypr.autostart")' '-- omarchy-relaunch' "$LEGACY_HOOK" >"$H/.config/hypr/hyprland.lua"
  printf 'o.exec_on_start("touch %s/pwned")\n' "$S" >"$H/.config/omarchy-relaunch/relaunch.lua"
  printf 'o.window("x", {})\n' >"$H/.config/omarchy-relaunch/rules.lua"
  (cd "$H" && find . -printf '%p %s\n' | sort) >"$S/home.before"
  cp "$H/.config/hypr/hyprland.lua" "$S/hyprland.before"
  unshare -rm bash "$WORKDIR/inner.sh" "$S" "$REL" "$ROOT" "$2" || fail "$1: the namespace run failed"
}
unchanged() { # the refused run wrote nothing in HOME
  (cd "$H" && find . -printf '%p %s\n' | sort) | cmp -s - "$S/home.before" \
    || fail "$1: a refused install.sh changed HOME: $(cd "$H" && find . | sort | diff "$S/home.before" - | head -5)"
  cmp -s "$S/hyprland.before" "$H/.config/hypr/hyprland.lua" || fail "$1: a refused install.sh changed hyprland.lua"
}

# --- a full install over a legacy one
scenario install none
[[ "$(cat "$S/rc")" == 0 ]] || fail "install.sh failed: $(cat "$S/out")"
LUA="$H/.config/hypr/hyprland.lua"
[[ "$(cat "$LUA")" == "require(\"hypr.autostart\")"$'\n'"-- omarchy-relaunch"$'\n'"pcall(dofile, \"$ANCHOR_LUA\")" ]] \
  || fail "hyprland.lua must carry exactly the anchor line: $(cat "$LUA")"
[[ "$(grep -cF "pcall(dofile, \"$ANCHOR_LUA\")" "$LUA")" == 1 ]] || fail "the anchor line must be present once"
grep -q 'relaunch.lua\|dofile(_rl)' "$LUA" && fail "the legacy bootstrap survived install.sh"
[[ ! -e "$H/.config/omarchy-relaunch/relaunch.lua" ]] || fail "install.sh must remove the legacy loader"
RULES="$H/.config/omarchy-relaunch/rules.lua"
[[ -f "$RULES" && ! -L "$RULES" ]] || fail "rules.lua must be a regular file"
grep -q 'Inspection copy only' "$RULES" || fail "rules.lua must be the inspection copy: $(cat "$RULES")"
grep -qE 'o\.|dofile|require' "$RULES" && fail "the inspection copy must hold no Lua: $(cat "$RULES")"
[[ "$(stat -c '%a' "$H/.config/omarchy-relaunch")" == 700 ]] || fail "the config dir must be 0700"
cmp -s "$ROOT/relaunch" "$H/.local/bin/relaunch" || fail "the relaunch shim must be on PATH"
PLUGIN="$H/.config/omarchy/plugins/io.github.laytonf.relaunch"
for f in manifest.json BarWidget.qml Panel.qml Overlay.qml TxnLifecycle.js CoreInstall.js relaunch; do
  cmp -s "$ROOT/$f" "$PLUGIN/$f" || fail "the plugin's $f must be installed"
done
[[ ! -e "$S/pwned" ]] || fail "install.sh ran the user's legacy loader"
grep -qF "$ANCHOR_LUA" "$S/out" || fail "install.sh must report the anchor wiring: $(cat "$S/out")"
extra="$(find "$S/lib-up" "$S/share-up" -mindepth 1 | sed "s#^$S/##" | { grep -vxE '(lib|share)-up/omarchy-relaunch' || true; } | sort | tr '\n' ' ')"
[[ -z "$extra" ]] || fail "install.sh wrote into the system directories: $extra"
[[ "$(cat "$S/core")" == 'lib/omarchy-relaunch/relaunch lib/omarchy-relaunch/relaunch-anchor lib/omarchy-relaunch/relaunchctl share/omarchy-relaunch/anchor.lua ' ]] \
  || fail "install.sh wrote into the core directories: $(cat "$S/core")"
printf '    install: anchor line once, no relaunch.lua, rules.lua an inspection copy, shim + plugin installed\n'

# --- refusals: any part of the core missing or not root-trusted
for fault in no-anchor-lua anchor-lua-writable anchor-dir-writable anchor-lua-symlink no-relaunch-anchor lib-dir-writable; do
  scenario "$fault" "$fault"
  [[ "$(cat "$S/rc")" != 0 ]] || fail "$fault: install.sh must refuse"
  grep -q 'Install the omarchy-relaunch system package first' "$S/out" || fail "$fault: install.sh must say why: $(cat "$S/out")"
  unchanged "$fault"
  printf '    refused, HOME unchanged: %s\n' "$fault"
done

printf 'ok %s\n' "$(basename "$0")"
