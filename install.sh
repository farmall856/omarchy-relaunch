#!/usr/bin/env bash
# Installer for the Relaunch Omarchy plugin (the per-user part).
#
# The engine itself is NOT installed here: it is the root-owned Rust binary the
# system package puts in /usr/lib/omarchy-relaunch, with relaunchctl and
# relaunch-anchor beside it, and the anchor loader in /usr/share/omarchy-relaunch.
# This script refuses to run, writing nothing, until all of that is in place:
# the panel it installs drives only that engine, and the only boot wiring it
# writes runs only that anchor.
#
# Three parts, in this order:
#   1. Check the system core is installed and root-owned, each file and its
#      directories.
#   2. Put the `relaunch` shim on PATH and in the plugin folder (it only hands
#      over to /usr/lib/omarchy-relaunch/relaunch), then install the plugin.
#   3. Wire the root anchor into hyprland.lua (`relaunch ensure-hooks
#      --anchor`), as this user. No Lua of the user's is loaded at login.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ID="io.github.laytonf.relaunch"
PLUGIN_DST="$HOME/.config/omarchy/plugins/$PLUGIN_ID"
BIN_DIR="${PREFIX:-$HOME/.local}/bin"
CORE_DIR="/usr/lib/omarchy-relaunch"
ANCHOR_LUA="/usr/share/omarchy-relaunch/anchor.lua"

# root_installed <path>: a regular file, and it, its directory and that
# directory's parent each root-owned, not a symlink, not group- or
# world-writable (the engine's own check, which ensure-hooks repeats).
root_installed() {
  local p="$1" _ mode
  [[ -f "$p" && ! -L "$p" ]] || return 1
  for _ in 1 2 3; do
    [[ ! -L "$p" && "$(stat -c '%u' "$p" 2>/dev/null)" == 0 ]] || return 1
    mode="$(stat -c '%a' "$p")" || return 1
    (( 8#$mode & 8#022 )) && return 1
    p="$(dirname "$p")"
  done
  return 0
}

echo "==> checking the system core"
for path in "$CORE_DIR/relaunch" "$CORE_DIR/relaunchctl" "$CORE_DIR/relaunch-anchor" "$ANCHOR_LUA"; do
  if ! root_installed "$path" || { [[ "$path" != "$ANCHOR_LUA" ]] && [[ ! -x "$path" ]]; }; then
    echo "error: $path is not installed root-owned (with its directories) and unwritable by others." >&2
    echo "       Install the omarchy-relaunch system package first." >&2
    exit 1
  fi
done
echo "    $CORE_DIR: relaunch, relaunchctl, relaunch-anchor; $ANCHOR_LUA"

echo "==> installing the relaunch shim"
mkdir -p "$BIN_DIR"
install -m 0755 "$REPO_DIR/relaunch" "$BIN_DIR/relaunch"
echo "    installed $BIN_DIR/relaunch -> $CORE_DIR/relaunch"
case ":$PATH:" in
  *":$BIN_DIR:"*) : ;;
  *) echo "    NOTE: add $BIN_DIR to PATH so the widget can find relaunch" ;;
esac

echo "==> installing plugin folder"
mkdir -p "$PLUGIN_DST"
install -m 0644 "$REPO_DIR/manifest.json" "$PLUGIN_DST/manifest.json"
install -m 0644 "$REPO_DIR/BarWidget.qml" "$PLUGIN_DST/BarWidget.qml"
install -m 0644 "$REPO_DIR/Panel.qml"     "$PLUGIN_DST/Panel.qml"
install -m 0644 "$REPO_DIR/Overlay.qml"   "$PLUGIN_DST/Overlay.qml"
install -m 0644 "$REPO_DIR/TxnLifecycle.js" "$PLUGIN_DST/TxnLifecycle.js"
install -m 0644 "$REPO_DIR/CoreInstall.js" "$PLUGIN_DST/CoreInstall.js"
install -m 0755 "$REPO_DIR/relaunch"      "$PLUGIN_DST/relaunch"
echo "    installed $PLUGIN_DST"

echo "==> wiring the root anchor into hyprland.lua"
"$BIN_DIR/relaunch" ensure-hooks --anchor
echo "    hyprland.lua  -> $ANCHOR_LUA (runs only a verified, signed launch list)"

echo "==> discovering plugin in the shell"
if command -v omarchy-shell >/dev/null 2>&1; then
  omarchy-shell shell rescanPlugins || true
fi

cat <<EOT

Done.

  1. Make sure $BIN_DIR is on PATH.
  2. Add the widget to your bar if it isn't auto-placed:
       omarchy bar move $PLUGIN_ID --section right
  3. Open your apps on their workspaces, click the Relaunch bar icon, and
     Save: review the change and Authorize it.
  4. Reboot to restore.

Remove with:
  relaunch uninstall --yes
  omarchy plugin remove $PLUGIN_ID   # (if installed via omarchy)
  rm -rf $PLUGIN_DST                  # (manual install)
EOT
