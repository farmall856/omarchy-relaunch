#!/usr/bin/env bash
# Pin a published release's boot tools package into this checkout (maintainer
# step, after .github/workflows/release.yml has built the tag):
#
#   packaging/pin-release.sh v2.0.4
#
# Downloads the x86_64 package from that release, requires its build
# attestation to name this repository's release workflow and the tag's commit,
# then writes its version, commit, file, size and SHA-256 to
# packaging/boot-tools.pin and the commit to packaging/arch/PKGBUILD. Commit
# the result: that commit is what the plugin installs, and the marketplace
# target. Needs git and a signed-in gh. Changes nothing unless every check
# passes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO=farmall856/omarchy-relaunch
WORKFLOW="$REPO/.github/workflows/release.yml"
PIN="$ROOT/packaging/boot-tools.pin"
PKGBUILD="$ROOT/packaging/arch/PKGBUILD"

die() { printf 'pin-release: %s\n' "$*" >&2; exit 1; }

tag="${1:-}"
[[ "$tag" =~ ^v[0-9]+(\.[0-9]+)*$ ]] || die "usage: pin-release.sh vX.Y.Z"
gh auth status >/dev/null 2>&1 || die "gh is not signed in"

pkgver="$(sed -n 's/^pkgver=//p' "$PKGBUILD" | head -n 1)"
pkgrel="$(sed -n 's/^pkgrel=//p' "$PKGBUILD" | head -n 1)"
[[ "v$pkgver" == "$tag" ]] || die "PKGBUILD pkgver is $pkgver, not ${tag#v}"

commit="$(git -C "$ROOT" rev-parse --verify "$tag^{commit}")" || die "no local tag $tag"
remote="$(git -C "$ROOT" ls-remote origin "refs/tags/$tag^{}" | cut -f1)"
[[ "$remote" == "$commit" ]] || die "origin's $tag is ${remote:-missing}, local is $commit"

file="omarchy-relaunch-$pkgver-$pkgrel-x86_64.pkg.tar.zst"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gh release download "$tag" --repo "$REPO" --pattern "$file" --dir "$work" \
  || die "release $tag has no $file"
gh attestation verify "$work/$file" --repo "$REPO" --signer-workflow "$WORKFLOW" \
  --source-digest "$commit" --deny-self-hosted-runners >/dev/null \
  || die "$file's build attestation does not name $WORKFLOW at $commit"

size="$(stat -c %s "$work/$file")"
sum="$(sha256sum "$work/$file" | cut -d' ' -f1)"
head -n "$(grep -c '^#' "$PIN")" "$PIN" >"$work/pin"
printf 'version=%s\ncommit=%s\nfile=%s\nsize=%s\nsha256=%s\n' \
  "$pkgver-$pkgrel" "$commit" "$file" "$size" "$sum" >>"$work/pin"
cp "$work/pin" "$PIN"
sed -i "s/^_commit=.*/_commit=$commit/" "$PKGBUILD"
"$ROOT/packaging/boot-tools.sh" lint
printf 'Pinned %s: %s (%s bytes, sha256 %s).\nRun tests/gate.sh, then commit packaging/boot-tools.pin and packaging/arch/PKGBUILD.\n' \
  "$tag" "$file" "$size" "$sum"
