# Relaunch — AUR helper package (`omarchy-relaunch`)

This directory packages the Relaunch **system core** ("authorized boot tools")
for the AUR. It is the helper package the shell plugin installs on the user's
behalf. The user-space bar widget is installed separately via the Omarchy plugin
marketplace and is NOT in this package.

## What it installs (all root-owned)

- `/usr/lib/omarchy-relaunch/{relaunchd,relaunch-anchor,relaunch,relaunchctl}`
- `/usr/share/polkit-1/actions/…relaunch.policy` + `rules.d/00-…relaunch.rules`
- `/usr/share/dbus-1/system-services/…Relaunch1.service` + `system.d/…Relaunch1.conf`
- `/usr/share/omarchy-relaunch/anchor.lua`
- `/usr/lib/systemd/system/relaunchd.service`
- `/usr/share/licenses/omarchy-relaunch/…`

`relaunchd` is D-Bus activated (`Type=dbus`); nothing is enabled or started at
install time. pacman's own alpm hooks reload systemd/D-Bus/polkit.

## Prerequisite before any AUR build

AUR builds from a **published git ref**. The 2.0.0 code (Rust rewrite + hardening
+ capture-keying + the short polkit prompt + the boot-tools plugin buttons) is
newer than the latest pushed tag (`v1.0.4`) and is currently uncommitted. Before
building for the real AUR, tag and push the version in `PKGBUILD` (`pkgver`).
These are the maintainer's own steps (for LOCAL testing without the AUR, use
`packaging/aur/test-on-postmarchy.sh` instead — no tag or push needed):

    git add -A && git commit -m "Relaunch 2.0.0"
    git tag -a v2.0.0 -m "Relaunch 2.0.0"
    git push origin HEAD --tags

Keep `pkgver`, the git tag, and `manifest.json`'s `version` in lockstep. Bump
`pkgrel` for packaging-only changes. Regenerate `.SRCINFO` after any edit:

    cd packaging/aur && makepkg --printsrcinfo > .SRCINFO

## Testing on an Arch box (e.g. the postmarchy server)

The server is headless, so this only validates that the package **builds and
installs** — the widget and login-restore need a Hyprland desktop to function.

Build + install from a checkout of this repo (after the tag is pushed):

    # on the build host, in a scratch dir
    git clone https://github.com/farmall856/omarchy-relaunch
    cp -r omarchy-relaunch/packaging/aur ./aur && cd aur
    makepkg -si            # builds the Rust core, installs with pacman (sudo)

Verify:

    pacman -Ql omarchy-relaunch | grep -E 'relaunchd|anchor.lua|policy'
    /usr/lib/omarchy-relaunch/relaunch list   # engine runs (no pins yet)

Remove:

    sudo pacman -Rns omarchy-relaunch

To test the eventual AUR path itself, publish to the AUR, then
`omarchy pkg aur add omarchy-relaunch` (what the plugin button runs).

## How the plugin bridges to this package

The marketplace installs only the QML widget. When it detects the core is
missing, it offers an **"Install boot tools"** button that runs:

    omarchy pkg aur add omarchy-relaunch

which clones from the AUR, builds, and installs with a polkit/sudo prompt. The
plugin then finishes per-user setup (wires the login anchor into `hyprland.lua`).

## Marketplace / README disclosure (required by the submission checklist)

The submission checklist requires the repo to document install/removal and any
external dependency. Put this (or similar) in the top-level `README.md` and in
the submission's "maintainer notes" field:

> **Helper package required.** Relaunch restores apps at login through a small
> root-owned system service installed from the AUR as `omarchy-relaunch` (the
> "boot tools"): a polkit-gated signing authority, a login-time verifier that
> runs only a signature-verified launch list, a D-Bus activation file, and a
> systemd unit. The plugin will prompt to install it (polkit/sudo) on first use,
> or install it yourself with `omarchy pkg aur add omarchy-relaunch` and then
> `relaunch ensure-hooks --anchor` (which wires the login step into
> `hyprland.lua`). Remove it with `sudo pacman -Rns omarchy-relaunch`. The plugin never changes your config
> without an explicit Authorize.

Suggested listing: **category** Productivity (or System); **tags** Workspaces,
Launcher (optionally Security).
