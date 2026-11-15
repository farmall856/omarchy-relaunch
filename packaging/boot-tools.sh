#!/usr/bin/env bash
# Install, update or reinstall the Relaunch boot tools (the root-owned
# omarchy-relaunch system package) exactly as THIS checkout pins them in
# packaging/boot-tools.pin. The Relaunch panel runs it in a terminal window;
# running it by hand does the same.
#
#   boot-tools.sh install|update|reinstall [--source]
#   boot-tools.sh check     headless: "installed V", "pinned V", and one of
#                           newer | current | notpkg | badpin
#   boot-tools.sh lint      the pin and the PKGBUILD agree (the gate)
#
# x86_64: the release package the pin names is downloaded from this
# repository's GitHub release and installed only when its size and SHA-256 are
# the pinned ones (and, when gh is signed in, its build attestation names this
# repository's release workflow and the pinned commit). Replacing a release
# asset or moving a tag changes nothing: only what this checkout pins installs.
# Any other architecture, --source, or a pin with no release package: the
# package is built here from packaging/arch/PKGBUILD, whose source is the
# pinned commit. Either way pacman asks for the password; this script never
# elevates anything else.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PIN="$HERE/boot-tools.pin"
ARCH_DIR="$HERE/arch"
REPO=farmall856/omarchy-relaunch
PKG=omarchy-relaunch
WORKFLOW="$REPO/.github/workflows/release.yml"

die() { printf 'boot-tools: %s\n' "$*" >&2; exit 1; }

declare -A P=()
PIN_ERR=""
# read_pin: the pin into P, or PIN_ERR and status 1. Five keys, each once,
# each value by its pattern; the release triple is all set or all empty.
read_pin() {
  P=()
  local line k v
  [[ -f "$PIN" ]] || { PIN_ERR="no pin file"; return 1; }
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" =~ ^([a-z0-9]+)=(.*)$ ]] || { PIN_ERR="bad line: $line"; return 1; }
    k="${BASH_REMATCH[1]}" v="${BASH_REMATCH[2]}"
    [[ -z "${P[$k]+x}" ]] || { PIN_ERR="$k twice"; return 1; }
    P[$k]="$v"
  done <"$PIN"
  for k in version commit file size sha256; do
    [[ -n "${P[$k]+x}" ]] || { PIN_ERR="no $k"; return 1; }
  done
  ((${#P[@]} == 5)) || { PIN_ERR="unknown key"; return 1; }
  [[ "${P[version]}" =~ ^[0-9]+(\.[0-9]+)*-[1-9][0-9]*$ ]] || { PIN_ERR="bad version"; return 1; }
  [[ "${P[commit]}" =~ ^[0-9a-f]{40}$ ]] || { PIN_ERR="bad commit"; return 1; }
  if [[ -n "${P[file]}${P[size]}${P[sha256]}" ]]; then
    [[ "${P[file]}" == "$PKG-${P[version]}-x86_64.pkg.tar.zst" ]] || { PIN_ERR="file is not the version's package"; return 1; }
    [[ "${P[size]}" =~ ^[1-9][0-9]{0,9}$ ]] || { PIN_ERR="bad size"; return 1; }
    [[ "${P[sha256]}" =~ ^[0-9a-f]{64}$ ]] || { PIN_ERR="bad sha256"; return 1; }
  fi
  return 0
}

# pkgbuild_value <name>: a top-level `name=value` line of the PKGBUILD.
pkgbuild_value() { sed -n "s/^$1=//p" "$ARCH_DIR/PKGBUILD" | head -n 1; }

# The PKGBUILD builds the pinned commit at the pinned version.
pkgbuild_matches() {
  [[ "$(pkgbuild_value _commit)" == "${P[commit]}" ]] || { PIN_ERR="PKGBUILD _commit is not the pinned commit"; return 1; }
  [[ "$(pkgbuild_value pkgver)-$(pkgbuild_value pkgrel)" == "${P[version]}" ]] || { PIN_ERR="PKGBUILD version is not the pinned version"; return 1; }
}

check() {
  if ! read_pin; then echo badpin; return 0; fi
  echo "pinned ${P[version]}"
  local v
  v="$(pacman -Q "$PKG" 2>/dev/null)" || { echo notpkg; return 0; }
  v="${v#* }"
  echo "installed $v"
  if (($(vercmp "${P[version]}" "$v") > 0)); then echo newer; else echo current; fi
}

lint() {
  read_pin || die "pin: $PIN_ERR"
  pkgbuild_matches || die "$PIN_ERR"
  echo "pin ok: ${P[version]} at ${P[commit]}${P[file]:+, release ${P[file]}}"
}

WORK=""
cleanup() { [[ -z "$WORK" ]] || rm -rf "$WORK"; }
trap cleanup EXIT
workdir() { WORK="$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/relaunch-boot-tools.XXXXXX")"; }

# from_release <pacman -U flags…>: download, check against the pin, install.
from_release() {
  workdir
  local f="$WORK/${P[file]}"
  local url="https://github.com/$REPO/releases/download/v${P[version]%-*}/${P[file]}"
  echo "Downloading ${P[file]} from the v${P[version]%-*} release…"
  curl -fL --proto '=https' --tlsv1.2 --max-filesize "${P[size]}" -o "$f" "$url" \
    || die "the download failed: nothing was installed"
  [[ "$(stat -c %s "$f")" == "${P[size]}" ]] || die "its size is not the pinned one: nothing was installed"
  printf '%s  %s\n' "${P[sha256]}" "$f" | sha256sum -c --status - \
    || die "its SHA-256 is not the pinned one: nothing was installed"
  echo "Checked against this plugin's pin: SHA-256 ${P[sha256]}."
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    gh attestation verify "$f" --repo "$REPO" --signer-workflow "$WORKFLOW" \
      --source-digest "${P[commit]}" --deny-self-hosted-runners >/dev/null \
      || die "its build attestation does not verify: nothing was installed"
    echo "Build attestation checked: built by $REPO's release workflow from ${P[commit]:0:12}."
  else
    echo "(gh is not signed in, so the build attestation was not checked; the pinned SHA-256 was.)"
  fi
  sudo pacman -U "$@" "$f"
}

# from_source <makepkg flags…>: build the pinned commit here, then install.
from_source() {
  pkgbuild_matches || die "$PIN_ERR"
  if ! command -v makepkg >/dev/null 2>&1 || ! command -v fakeroot >/dev/null 2>&1; then
    die "building from source needs base-devel: sudo pacman -S --needed base-devel"
  fi
  workdir
  cp "$ARCH_DIR/PKGBUILD" "$ARCH_DIR/$PKG.install" "$WORK/"
  echo "Building $PKG ${P[version]} from ${P[commit]:0:12} (this takes a few minutes)…"
  (cd "$WORK" && makepkg --syncdeps --install --rmdeps --clean "$@")
}

run() {
  local mode="$1" source=false
  shift
  case "${1:-}" in
    --source) source=true; shift ;;
    "") ;;
    *) die "unknown option: $1" ;;
  esac
  (($# == 0)) || die "too many arguments"
  read_pin || die "this plugin's pin is invalid ($PIN_ERR): reinstall the plugin"
  local needed=(--needed)
  [[ "$mode" == reinstall ]] && needed=()
  if ! $source && [[ "$(uname -m)" == x86_64 && -n "${P[file]}" ]]; then
    from_release "${needed[@]}"
  else
    if ! $source; then
      if [[ -z "${P[file]}" ]]; then echo "No release package is pinned yet, so the boot tools are built from source."
      else echo "The release package is x86_64 only, so the boot tools are built from source on $(uname -m)."; fi
    fi
    from_source "${needed[@]}"
  fi
}

case "${1:-}" in
  install | update | reinstall) run "$@" ;;
  check) (($# == 1)) || die "check takes no arguments"; check ;;
  lint) (($# == 1)) || die "lint takes no arguments"; lint ;;
  *) die "usage: boot-tools.sh install|update|reinstall [--source] | check | lint" ;;
esac
