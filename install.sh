#!/usr/bin/env bash
# Installer for the Relaunch Omarchy plugin.
#
# Two parts:
#   1. Install the `relaunch` engine (bash + jq) onto your PATH. The QML bar
#      widget shells out to it.
#   2. Install the plugin folder into ~/.config/omarchy/plugins/ and wire
#      the hidden boot hook plus relaunch.lua pins (via `relaunch ensure-hooks`).
#
# Prefer `omarchy plugin add <repo-url> --enable` for the QML side once this is
# published; this script is for local development / manual installs.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ID="io.github.laytonf.relaunch"
PLUGIN_DST="$HOME/.config/omarchy/plugins/$PLUGIN_ID"
BIN_DIR="${PREFIX:-$HOME/.local}/bin"

echo "==> installing relaunch engine"
if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq not found. It ships with Omarchy; install jq and retry." >&2
  exit 1
fi
mkdir -p "$BIN_DIR"
install -m 0755 "$REPO_DIR/relaunch" "$BIN_DIR/relaunch"
echo "    installed $BIN_DIR/relaunch"
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
install -m 0755 "$REPO_DIR/relaunch"      "$PLUGIN_DST/relaunch"
echo "    installed $PLUGIN_DST"

echo "==> wiring hidden boot hook (not shown in the Relaunch list)"
"$BIN_DIR/relaunch" ensure-hooks
echo "    autostart.lua -> relaunch boot"
echo "    hyprland.lua  -> dofile relaunch.lua"

echo "==> discovering plugin in the shell"
if command -v omarchy-shell >/dev/null 2>&1; then
  omarchy-shell shell rescanPlugins || true
fi

cat <<EOF

Done.

  1. Make sure $BIN_DIR is on PATH.
  2. Add the widget to your bar if it isn't auto-placed:
       omarchy bar move $PLUGIN_ID --section right
  3. Arrange your apps across workspaces, click the Relaunch bar icon,
     and hit "Save Startup App Workspaces".
  4. Reboot to restore.

Remove with:
  relaunch uninstall --yes
  omarchy plugin remove $PLUGIN_ID   # (if installed via omarchy)
  rm -rf $PLUGIN_DST                  # (manual install)
EOF
