#!/usr/bin/env bash
# The AUR package (packaging/aur/PKGBUILD), packaged from this tree into a
# scratch directory: it owns files only under /usr, and its install script has
# no removal hook. So removing the package (the panel's Uninstall:
# `omarchy pkg drop`, i.e. `pacman -Rns`) can never reach the signing key and
# signed state under /var/lib/omarchy-relaunch, which systemd's
# StateDirectory= creates and no package owns.
#
#   tests/pkgbuild-test.sh   (needs the release core in core/target/release)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
mkdir -p "$W/src" "$W/pkg"
ln -s "$ROOT" "$W/src/omarchy-relaunch"
(cd "$ROOT" && bash -c 'set -e; source packaging/aur/PKGBUILD; srcdir="$1"; pkgdir="$2"; package' _ "$W/src" "$W/pkg") \
  || fail "package() failed"
mapfile -t files < <(cd "$W/pkg" && find . \( -type f -o -type l \) | sed 's|^\.||' | sort)
(( ${#files[@]} > 0 )) || fail "package() installed nothing"
for f in "${files[@]}"; do
  [[ "$f" == /usr/* ]] || fail "the package owns $f, outside /usr"
done
[[ -x "$W/pkg/usr/lib/omarchy-relaunch/relaunchd" ]] || fail "the package lacks relaunchd"
if grep -qE '^[[:space:]]*(pre|post)_remove[[:space:]]*\(' "$ROOT/packaging/aur/omarchy-relaunch.install"; then
  fail "the install script has a removal hook"
fi
printf 'ok %s (%d files, all under /usr; no removal hook)\n' "$(basename "$0")" "${#files[@]}"
