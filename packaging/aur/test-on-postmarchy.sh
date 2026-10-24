#!/usr/bin/env bash
#
# Build the Relaunch boot-tools package from THIS working tree (committed or not)
# and serve it from a LOCAL pacman repo, so the plugin's "Install boot tools"
# button installs it via yay WITHOUT ever touching the real AUR. For testing the
# full find -> install -> update -> uninstall flow on an Arch box (e.g. the
# postmarchy server).
#
# Run it as your normal user from a checkout of the repo. It calls `sudo` only to
# add the local repo to /etc/pacman.conf and to sync. It installs NOTHING by
# itself -- the plugin's button (or a manual `yay -S omarchy-relaunch`) does that.
#
# Needs: base-devel (makepkg), rust/cargo. Re-run with PKGVER=2.0.1 (etc.) to
# publish a newer build into the repo and exercise "Check for Updates".
#
# Tear down when done:
#   omarchy pkg drop omarchy-relaunch
#   sudo sed -i '/^\[relaunch-test\]/,/^Server = file:/d' /etc/pacman.conf && sudo pacman -Sy
#   rm -rf ~/localrepo-relaunch
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"   # packaging/aur -> repo root
PKGVER="${PKGVER:-2.0.0}"
REPO_NAME="relaunch-test"
LOCALREPO="${LOCALREPO:-$HOME/localrepo-relaunch}"
BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/relaunch-pkgbuild.XXXXXX")"
trap 'rm -rf "$BUILD_DIR"' EXIT

die() { echo "test-on-postmarchy: $*" >&2; exit 1; }
[ "$(id -u)" -ne 0 ] || die "run as your normal user (it calls sudo itself); makepkg refuses root"
command -v makepkg >/dev/null || die "makepkg not found -- install base-devel"
command -v cargo   >/dev/null || die "cargo not found -- install rust"
command -v repo-add >/dev/null || die "repo-add not found -- install pacman/base-devel"

echo "== repo:        $REPO_DIR"
echo "== version:     $PKGVER"
echo "== local repo:  $LOCALREPO  (pacman repo [$REPO_NAME])"

# A self-contained PKGBUILD that builds THIS tree (no git fetch, so uncommitted
# changes ARE included) and installs exactly what packaging/aur/PKGBUILD does.
# Quoted heredoc = no expansion; the three @PLACEHOLDERS@ are filled by sed, so
# $pkgdir / $_repo stay literal for makepkg.
cat > "$BUILD_DIR/PKGBUILD" <<'PKGBUILD_EOF'
pkgname=omarchy-relaunch
pkgver=@PKGVER@
pkgrel=1
pkgdesc="Relaunch system core (LOCAL TEST build from working tree)"
arch=('@ARCH@')
url="https://github.com/farmall856/omarchy-relaunch"
license=('MIT')
depends=('polkit' 'systemd' 'dbus')
options=('!debug')
source=()

_repo="@REPO@"

build() {
  cd "$_repo/core"
  cargo build --release -p relaunchd -p relaunch-anchor -p relaunch-engine -p relaunchctl
}

package() {
  cd "$_repo"
  install -Dm0755 core/target/release/relaunchd       "$pkgdir/usr/lib/omarchy-relaunch/relaunchd"
  install -Dm0755 core/target/release/relaunch-anchor "$pkgdir/usr/lib/omarchy-relaunch/relaunch-anchor"
  install -Dm0755 core/target/release/relaunch        "$pkgdir/usr/lib/omarchy-relaunch/relaunch"
  install -Dm0755 core/target/release/relaunchctl     "$pkgdir/usr/lib/omarchy-relaunch/relaunchctl"
  install -dm0755 "$pkgdir/usr/bin"
  ln -s /usr/lib/omarchy-relaunch/relaunch "$pkgdir/usr/bin/relaunch"
  install -Dm0644 packaging/polkit/io.github.laytonf.relaunch.policy   "$pkgdir/usr/share/polkit-1/actions/io.github.laytonf.relaunch.policy"
  install -Dm0644 packaging/polkit/00-io.github.laytonf.relaunch.rules "$pkgdir/usr/share/polkit-1/rules.d/00-io.github.laytonf.relaunch.rules"
  install -Dm0644 packaging/dbus/io.github.laytonf.Relaunch1.service   "$pkgdir/usr/share/dbus-1/system-services/io.github.laytonf.Relaunch1.service"
  install -Dm0644 packaging/dbus/io.github.laytonf.Relaunch1.conf      "$pkgdir/usr/share/dbus-1/system.d/io.github.laytonf.Relaunch1.conf"
  install -Dm0644 packaging/lua/anchor.lua                             "$pkgdir/usr/share/omarchy-relaunch/anchor.lua"
  install -Dm0644 packaging/systemd/relaunchd.service                  "$pkgdir/usr/lib/systemd/system/relaunchd.service"
  install -Dm0644 THIRD-PARTY-NOTICES.md "$pkgdir/usr/share/licenses/omarchy-relaunch/THIRD-PARTY-NOTICES.md"
  install -Dm0644 LICENSE                "$pkgdir/usr/share/licenses/omarchy-relaunch/LICENSE"
}
PKGBUILD_EOF
sed -i "s|@PKGVER@|$PKGVER|g; s|@ARCH@|$(uname -m)|g; s|@REPO@|$REPO_DIR|g" "$BUILD_DIR/PKGBUILD"

echo "== building the package (makepkg -f)…"
( cd "$BUILD_DIR" && makepkg -f ) || die "makepkg failed"
pkgfile="$(ls -1 "$BUILD_DIR"/omarchy-relaunch-"$PKGVER"-*.pkg.tar.* | head -1)"
[ -f "$pkgfile" ] || die "build produced no package"
echo "== built: $(basename "$pkgfile")"

echo "== publishing into the local repo…"
mkdir -p "$LOCALREPO"
cp -f "$pkgfile" "$LOCALREPO/"
repo-add "$LOCALREPO/$REPO_NAME.db.tar.gz" "$LOCALREPO/$(basename "$pkgfile")"

if ! grep -q "^\[$REPO_NAME\]" /etc/pacman.conf; then
  echo "== adding [$REPO_NAME] to /etc/pacman.conf (sudo)…"
  sudo tee -a /etc/pacman.conf >/dev/null <<CONF

[$REPO_NAME]
SigLevel = Optional TrustAll
Server = file://$LOCALREPO
CONF
else
  echo "== [$REPO_NAME] already in /etc/pacman.conf"
fi
echo "== sudo pacman -Sy…"
sudo pacman -Sy

cat <<NEXT

Done. 'yay -S omarchy-relaunch' now installs the LOCAL $PKGVER build from
[$REPO_NAME]; the real AUR is never consulted (a repo package beats the AUR).

Test the plugin flow on this box:
  1. Install the widget (faithful marketplace flow, no push). The plugin clone
     uses git, so COMMIT your changes first or it won't see them:
         git -C "$REPO_DIR" add -A && git -C "$REPO_DIR" commit -m "Relaunch $PKGVER (test)"
         omarchy plugin add "file://$REPO_DIR" --enable
     (Quick alternative that uses the working tree directly, skipping the
      omarchy-plugin-add path: run  "$REPO_DIR/install.sh"  AFTER the core is in.)
  2. Open the Relaunch widget -> "Install boot tools". It runs
     'omarchy pkg aur add omarchy-relaunch' -> yay installs from [$REPO_NAME] ->
     the panel wires the login anchor and goes to ready.
  3. Check for Updates: re-run this script as  PKGVER=$PKGVER.1 $0  to publish a
     newer build, then click Check for Updates -> "Update available".
  4. Reinstall, then Uninstall ('omarchy pkg drop omarchy-relaunch') -> back to
     the Install card.
  5. Reboot to confirm the login restore.

Tear down: see the header of this script.
NEXT
