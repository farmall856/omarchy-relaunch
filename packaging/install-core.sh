#!/usr/bin/env bash
# Install the Relaunch system core (step 2 of the chunk-6 install plan).
# Run with sudo-cached credentials: `sudo -v` first, then `./packaging/install-core.sh`.
# Aborts on the first error and installs nothing further.
set -euo pipefail

REPO=/home/laytonf/Projects/relauncher
REL="$REPO/core/target/release"

die() { echo "install-core: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run under sudo (e.g. sudo ./packaging/install-core.sh)"

# Preflight: every source file must exist before anything is installed.
srcs=(
  "$REL/relaunchd"
  "$REL/relaunch-anchor"
  "$REL/relaunch"
  "$REL/relaunchctl"
  "$REPO/packaging/polkit/io.github.laytonf.relaunch.policy"
  "$REPO/packaging/polkit/00-io.github.laytonf.relaunch.rules"
  "$REPO/packaging/dbus/io.github.laytonf.Relaunch1.service"
  "$REPO/packaging/dbus/io.github.laytonf.Relaunch1.conf"
  "$REPO/packaging/lua/anchor.lua"
  "$REPO/packaging/systemd/relaunchd.service"
  "$REPO/THIRD-PARTY-NOTICES.md"
  "$REPO/LICENSES/freedesktop-desktop-entry/LICENSE"
  "$REPO/LICENSE"
)
for f in "${srcs[@]}"; do
  [ -f "$f" ] || die "missing source: $f"
done

echo "== root-owned binary directory"
install -d -m 0755 /usr/lib/omarchy-relaunch

echo "== root authority service (D-Bus activated, runs as root)"
install -m 0755 "$REL/relaunchd"      /usr/lib/omarchy-relaunch/relaunchd

echo "== login verifier/boot binary (root-owned, runs as the user)"
install -m 0755 "$REL/relaunch-anchor" /usr/lib/omarchy-relaunch/relaunch-anchor

echo "== Rust engine the shim execs (user-run)"
install -m 0755 "$REL/relaunch"         /usr/lib/omarchy-relaunch/relaunch

echo "== client the panel drives (user-run)"
install -m 0755 "$REL/relaunchctl"     /usr/lib/omarchy-relaunch/relaunchctl

echo "== polkit action (allow_active = auth_self)"
install -m 0644 "$REPO/packaging/polkit/io.github.laytonf.relaunch.policy"  /usr/share/polkit-1/actions/io.github.laytonf.relaunch.policy

echo "== polkit rule (out-orders empower.rules; denies non-local/inactive)"
install -m 0644 "$REPO/packaging/polkit/00-io.github.laytonf.relaunch.rules" /usr/share/polkit-1/rules.d/00-io.github.laytonf.relaunch.rules

echo "== D-Bus activation file (starts relaunchd on demand, as root)"
install -m 0644 "$REPO/packaging/dbus/io.github.laytonf.Relaunch1.service" /usr/share/dbus-1/system-services/io.github.laytonf.Relaunch1.service

echo "== D-Bus policy (root owns the name; any user may call; polkit gates authority)"
install -m 0644 "$REPO/packaging/dbus/io.github.laytonf.Relaunch1.conf"    /usr/share/dbus-1/system.d/io.github.laytonf.Relaunch1.conf

echo "== root anchor loader hyprland.lua runs at login (root 0644)"
install -d -m 0755 /usr/share/omarchy-relaunch
install -m 0644 "$REPO/packaging/lua/anchor.lua" /usr/share/omarchy-relaunch/anchor.lua

echo "== hardened systemd unit"
install -m 0644 "$REPO/packaging/systemd/relaunchd.service" /usr/lib/systemd/system/relaunchd.service

echo "== third-party notices + retained license text"
install -Dm 0644 "$REPO/THIRD-PARTY-NOTICES.md" /usr/share/licenses/omarchy-relaunch/THIRD-PARTY-NOTICES.md
install -Dm 0644 "$REPO/LICENSES/freedesktop-desktop-entry/LICENSE" /usr/share/licenses/omarchy-relaunch/LICENSES/freedesktop-desktop-entry/LICENSE
install -Dm 0644 "$REPO/LICENSE" /usr/share/licenses/omarchy-relaunch/LICENSE

echo "== reload systemd / dbus / polkit"
systemctl daemon-reload
systemctl reload dbus-broker
systemctl restart polkit

echo "install-core: done (no enable/start — service is Type=dbus, activated on first call)"
