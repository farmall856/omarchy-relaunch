# Relaunch: the boot tools package (`omarchy-relaunch`)

This directory packages the Relaunch **system core** (the "boot tools") for
Arch. The shell plugin installs it on the user's behalf through
`packaging/boot-tools.sh`. The bar widget itself is installed through the
Omarchy plugin marketplace and is NOT in this package.

## What it installs (all root-owned)

- `/usr/lib/omarchy-relaunch/{relaunchd,relaunch-anchor,relaunch,relaunchctl}`
  and `/usr/bin/relaunch` (a symlink)
- `/usr/share/polkit-1/actions/…relaunch.policy` + `rules.d/00-…relaunch.rules`
- `/usr/share/dbus-1/system-services/…Relaunch1.service` + `system.d/…Relaunch1.conf`
- `/usr/share/omarchy-relaunch/anchor.lua`
- `/usr/lib/systemd/system/relaunchd.service`
- `/usr/share/licenses/omarchy-relaunch/…`

`relaunchd` is D-Bus activated (`Type=dbus`), so nothing is enabled or started
at install time. pacman's own alpm hooks reload systemd, D-Bus and polkit.

## How a version reaches users

The plugin installs only what `packaging/boot-tools.pin` names (see the
top-level README, "Where the boot tools come from"). A release is four steps:

1. Bump `pkgver` here and `version` in `manifest.json`, commit, and push a tag
   `v$pkgver`.
2. `.github/workflows/release.yml` builds the package from the tagged commit
   in an `archlinux` container, attests its build provenance (Sigstore,
   keyless), and attaches it to a **draft** release. Review the draft and
   publish it.
3. `packaging/pin-release.sh v$pkgver` downloads that package, requires its
   attestation to name the release workflow and the tag's commit, and writes
   the version, commit, file, size and SHA-256 into `packaging/boot-tools.pin`
   and the commit into this PKGBUILD's `_commit`.
4. Run `tests/gate.sh`, then commit and push the pin. That commit is what
   users install, and the marketplace's target commit.

Keep `pkgver`, the tag and `manifest.json`'s `version` in lockstep. For a
packaging-only change, bump `pkgrel`: the release workflow uploads the new
`-<pkgrel>` package to the same tag's release.

## Local builds

To build the pinned commit by hand (what `boot-tools.sh install --source`
does):

    cp packaging/arch/{PKGBUILD,omarchy-relaunch.install} "$(mktemp -d)" && cd "$_"
    makepkg --syncdeps --install --rmdeps --clean

`packaging/arch/test-on-postmarchy.sh` builds the WORKING TREE instead (no
fetch, so uncommitted changes are included) into a local pacman repo. It's for
testing on a headless Arch box: it checks that the package builds and
installs, not that login restore works.

Verify an install:

    pacman -Ql omarchy-relaunch | grep -E 'relaunchd|anchor.lua|policy'
    /usr/lib/omarchy-relaunch/relaunch list   # the engine runs (no pins yet)

Remove:

    sudo pacman -Rns omarchy-relaunch
