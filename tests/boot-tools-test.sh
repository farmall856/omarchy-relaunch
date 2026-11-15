#!/usr/bin/env bash
# The stubs are scripts: their $ expands when they run, not here.
# shellcheck disable=SC2016
# packaging/boot-tools.sh, black-box, in a sandbox: curl, sudo, pacman, makepkg,
# fakeroot, gh and uname are stand-ins on PATH that record what they were asked
# and install nothing. Each case copies packaging/ fresh and may rewrite its pin.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT

BIN="$W/bin"
mkdir -p "$BIN"
stub() { printf '#!/usr/bin/env bash\n%s\n' "$2" >"$BIN/$1"; chmod +x "$BIN/$1"; }
# curl -… -o FILE URL: logs the URL and writes the fixture asset to FILE.
stub curl 'out=""; while (($#)); do [[ "$1" == -o ]] && { out="$2"; shift; }; u="$1"; shift; done
printf "curl %s\n" "$u" >>"$LOG"; cp "$ASSET" "$out"'
stub sudo 'printf "sudo %s\n" "$*" >>"$LOG"'
stub pacman '[[ "$1" == -Q && -n "${INSTALLED:-}" ]] && { echo "omarchy-relaunch $INSTALLED"; exit 0; }; exit 1'
stub makepkg 'printf "makepkg %s [%s]\n" "$*" "$(LC_ALL=C ls -A | tr "\n" " ")" >>"$LOG"'
stub fakeroot 'exit 0'
stub uname 'echo "${MACHINE:-x86_64}"'
stub gh 'case "$1 ${2:-}" in
  "auth status") [[ -n "${GH:-}" ]] ;;
  "attestation verify") printf "gh %s\n" "$*" >>"$LOG"; [[ "$GH" == ok ]] ;;
  *) exit 2 ;; esac'
# vercmp is pacman's; the real one is used when present.
command -v vercmp >/dev/null || { echo "boot-tools-test: needs vercmp (pacman)" >&2; exit 1; }

ASSET="$W/asset.pkg.tar.zst"
head -c 4096 /dev/urandom >"$ASSET"
SIZE="$(stat -c %s "$ASSET")"
SUM="$(sha256sum "$ASSET" | cut -d' ' -f1)"
COMMIT="$(sed -n 's/^commit=//p' "$ROOT/packaging/boot-tools.pin")"
VER="$(sed -n 's/^version=//p' "$ROOT/packaging/boot-tools.pin")"
FILE="omarchy-relaunch-$VER-x86_64.pkg.tar.zst"

fail=0
n=0
# t <name> <expect-status> <log-pattern|-> <absent-pattern|-> <pin-lines|-> <args…>
t() {
  local name="$1" want="$2" has="$3" hasnt="$4" pin="$5"
  shift 5
  local c="$W/case$n" status=0
  n=$((n + 1))
  mkdir -p "$c"
  cp -r "$ROOT/packaging" "$c/"
  [[ "$pin" == - ]] || printf '%b' "$pin" >"$c/packaging/boot-tools.pin"
  : >"$c/log"
  (cd "$c" && env PATH="$BIN:$PATH" LOG="$c/log" ASSET="$ASSET" XDG_RUNTIME_DIR="$c" \
    INSTALLED="${INSTALLED:-}" MACHINE="${MACHINE:-}" GH="${GH:-}" \
    bash "$c/packaging/boot-tools.sh" "$@" >"$c/out" 2>&1) || status=$?
  local bad=""
  [[ "$status" == "$want" ]] || bad="exit $status, wanted $want"
  [[ "$has" == - ]] || grep -qE -- "$has" "$c/log" "$c/out" || bad+=" missing /$has/"
  [[ "$hasnt" == - ]] || ! grep -qE -- "$hasnt" "$c/log" || bad+=" unwanted /$hasnt/"
  if [[ -n "$bad" ]]; then
    printf 'FAIL %s:%s\n' "$name" "$bad"
    sed 's/^/    /' "$c/out" "$c/log"
    fail=1
  fi
}
released="version=$VER\ncommit=$COMMIT\nfile=$FILE\nsize=$SIZE\nsha256=$SUM\n"

t "lint: the checked-in pin agrees with the PKGBUILD" 0 "^pin ok" - - lint
t "check: no package installed" 0 "^notpkg" - - check
INSTALLED=1.0.0-1 t "check: an older package is installed" 0 "^newer" - - check
INSTALLED="$VER" t "check: the pinned package is installed" 0 "^current" - - check
INSTALLED=99.0.0-1 t "check: a newer package is never downgraded" 0 "^current" - - check
t "check: a broken pin is badpin" 0 "^badpin" - 'version=x\n' check

MACHINE=x86_64 t "x86_64: the pinned release, checked, installed with --needed" 0 \
  "^sudo pacman -U --needed .*/$FILE$" makepkg "$released" install
t "the download is this repo's release for the pinned tag" 0 \
  "^curl https://github.com/farmall856/omarchy-relaunch/releases/download/v${VER%-*}/$FILE$" - "$released" update
t "reinstall reinstalls (no --needed)" 0 "^sudo pacman -U /" "--needed" "$released" reinstall
t "a SHA-256 that is not the pin installs nothing" 1 "SHA-256 is not the pinned one" "^sudo" \
  "version=$VER\ncommit=$COMMIT\nfile=$FILE\nsize=$SIZE\nsha256=${SUM//?/0}\n" install
t "a size that is not the pin installs nothing" 1 "size is not the pinned one" "^sudo" \
  "version=$VER\ncommit=$COMMIT\nfile=$FILE\nsize=$((SIZE + 1))\nsha256=$SUM\n" install
GH=ok t "signed-in gh: the attestation must name the workflow and the pinned commit" 0 \
  "^gh attestation verify .* --repo farmall856/omarchy-relaunch --signer-workflow farmall856/omarchy-relaunch/.github/workflows/release.yml --source-digest $COMMIT --deny-self-hosted-runners" \
  - "$released" install
GH=ok t "signed-in gh: the install follows the attestation" 0 "^sudo pacman -U" - "$released" install
GH=bad t "an attestation that does not verify installs nothing" 1 "attestation does not verify" "^sudo" "$released" install

MACHINE=aarch64 t "aarch64: built from source, never downloaded" 0 \
  "^makepkg --syncdeps --install --rmdeps --clean --needed \[PKGBUILD omarchy-relaunch.install \]" "^curl|^sudo" "$released" install
t "--source on x86_64 builds from source" 0 "^makepkg " "^curl|^sudo" "$released" install --source
t "no release pinned yet: built from source" 0 "^makepkg .*--needed" "^curl" - install
t "a reinstall from source rebuilds without --needed" 0 "^makepkg --syncdeps --install --rmdeps --clean \[" "--needed" - reinstall --source
t "a PKGBUILD that builds another commit is refused" 1 "_commit is not the pinned commit" "^makepkg" \
  "version=$VER\ncommit=${COMMIT//?/a}\nfile=\nsize=\nsha256=\n" install
t "a PKGBUILD at another version is refused" 1 "version is not the pinned version" "^makepkg" \
  "version=9.9.9-1\ncommit=$COMMIT\nfile=\nsize=\nsha256=\n" install

for bad in "version=$VER\ncommit=$COMMIT\nfile=\nsize=\nsha256=\nextra=1\n" \
  "version=$VER\nversion=$VER\ncommit=$COMMIT\nfile=\nsize=\nsha256=\n" \
  "version=$VER\ncommit=$COMMIT\nfile=other.pkg.tar.zst\nsize=$SIZE\nsha256=$SUM\n" \
  "version=$VER\ncommit=$COMMIT\nfile=$FILE\nsize=\nsha256=$SUM\n" \
  "version=$VER\ncommit=$COMMIT\nfile=\nsize=\n" \
  "version=$VER\ncommit=$COMMIT\nfile=\nsize=\nsha256=\n junk\n"; do
  t "a malformed pin installs nothing: ${bad//\\n/ }" 1 "pin is invalid" "^curl|^sudo|^makepkg" "$bad" install
done
t "an unknown option is refused" 1 "unknown option" "^curl|^sudo|^makepkg" - install --force
t "no command is refused" 1 "usage" - - ""

((fail == 0)) || exit 1
echo "ok boot-tools-test.sh ($n cases)"
