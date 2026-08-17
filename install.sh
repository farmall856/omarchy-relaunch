#!/usr/bin/env bash
# Installer for the Relaunch Omarchy plugin.
#
# Two parts:
#   1. Build the `relaunch` engine binary (Go) onto your PATH. The QML bar
#      widget shells out to it.
#   2. Install the plugin folder into ~/.config/omarchy/plugins/ and ensure the
#      generated Hyprland snippet is sourced.
#
# Prefer `omarchy plugin add <repo-url> --enable` for the QML side once this is
# published; this script is for local development / manual installs.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ID="io.github.laytonf.relaunch"
PLUGIN_DST="$HOME/.config/omarchy/plugins/$PLUGIN_ID"
BIN_DIR="${PREFIX:-$HOME/.local}/bin"
HYPR_CONF="$HOME/.config/hypr/hyprland.conf"
SNIPPET="$HOME/.config/omarchy-relaunch/relaunch.conf"
SOURCE_LINE="source = $SNIPPET"

echo "==> building relaunch engine"
if ! command -v go >/dev/null 2>&1; then
  echo "error: Go toolchain not found. Install go (pacman -S go) and retry." >&2
  exit 1
fi
mkdir -p "$BIN_DIR"
( cd "$REPO_DIR/engine" && go build -o "$BIN_DIR/relaunch" . )
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
echo "    installed $PLUGIN_DST"

echo "==> ensuring snippet is sourced from hyprland.conf"
mkdir -p "$(dirname "$SNIPPET")"
[[ -f "$SNIPPET" ]] || echo "# populated by: relaunch save" > "$SNIPPET"
if [[ -f "$HYPR_CONF" ]] && ! grep -qF "$SOURCE_LINE" "$HYPR_CONF"; then
  { echo ""; echo "# omarchy-relaunch"; echo "$SOURCE_LINE"; } >> "$HYPR_CONF"
  echo "    added source line to $HYPR_CONF"
else
  echo "    source line already present (or hyprland.conf missing)"
fi

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
  4. Reboot, or click "Reload Hyprland".

Remove with:
  omarchy plugin remove $PLUGIN_ID   # (if installed via omarchy)
  rm -rf $PLUGIN_DST                  # (manual install)
EOF
