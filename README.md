# Relaunch

An Omarchy Quattro bar widget that saves your current app-to-workspace layout,
then relaunches those apps into the same workspaces after a reboot or crash.
Each app restores its own context; this makes sure it comes back up in the
right place.

Click the bar icon → **Save Startup App Workspaces**. That's it. On the next
boot, herdr is back on workspace 1, Brave on 2, your terminal on 6 — wherever
you had them.

## How it works

No daemon, no live snapshotting. When you save, the widget inventories running
windows (`hyprctl clients -j`), records one `class → workspace` mapping per app,
and generates a plain Hyprland snippet of `windowrulev2` pinning rules plus
`exec-once` launches. Hyprland sources that snippet at startup. Because it's
declarative config rather than live state, it survives a crash for free.

- **Bar widget (QML):** the clickable UI in the Omarchy bar.
- **Engine (`relaunch`, a small Go binary):** does the `hyprctl` inventory and
  writes the config and Hyprland snippet. The widget shells out to it.

## Install

Via Omarchy (recommended once published):

```sh
omarchy plugin add https://github.com/farmall856/omarchy-relaunch.git --enable
```

The engine binary still needs to be built and on your PATH:

```sh
git clone https://github.com/farmall856/omarchy-relaunch.git
cd omarchy-relaunch
./install.sh
```

`install.sh` builds `relaunch` into `~/.local/bin`, installs the plugin folder,
and wires the Hyprland `source =` line.

### Dependencies

- **Go** (build-time only) — to compile the `relaunch` engine.
- **Hyprland / Omarchy Quattro** — the runtime.
- A Nerd Font in the bar (Omarchy default) for the widget glyph.

The engine runs `hyprctl` and, on demand, `hyprctl reload`. No elevated
privileges; everything runs as your user, like any Hyprland command.

## Usage

1. Arrange your apps across workspaces the way you want them on boot.
2. Click the Relaunch icon in the bar.
3. **Save Startup App Workspaces** — captures the layout and writes the snippet.
4. **Reload Hyprland** to apply now, or just reboot.

## Configure

Fine-tune by editing `~/.config/omarchy-relaunch/config.json`:

```json
{
  "staggerSeconds": 0,
  "entries": [
    { "class": "herdr",         "workspace": 1, "exec": "herdr",     "enabled": true },
    { "class": "brave-browser", "workspace": 2, "exec": "brave",     "enabled": true },
    { "class": "Alacritty",     "workspace": 6, "exec": "alacritty", "enabled": true }
  ]
}
```

- `class` matches a window's `initialClass` (stable across an app's post-launch
  class changes — Brave/Electron do this).
- `exec` overrides the guessed launch command; leave blank to use the guess.
- `enabled` keeps an entry on file but out of the generated snippet.
- `float` (optional) adds a float rule for that app.
- `staggerSeconds` inserts `sleep N &&` before each launch if apps race the
  rules on startup.

After editing, click **Regenerate** (or run `relaunch generate`).

## Remove

```sh
omarchy plugin remove io.github.laytonf.relaunch
```

Then delete the snippet source line from `hyprland.conf` and, if you like,
`rm -rf ~/.config/omarchy-relaunch`.

## Notes / limits

- One instance per app: a `class:^(brave-browser)$` rule sends every Brave
  window to its workspace, which is correct for a one-pinned-instance-per-app
  setup.
- Launch commands come from a small curated table because `/proc/<pid>/cmdline`
  is unreliable for Electron/flatpak/wrapped apps. Unknown apps fall back to a
  lowercased class; check `exec` in the config if one doesn't relaunch.
- Terminals relaunch empty; lean on the app's own restore (tmux-resurrect, etc.)
  for in-terminal context.

## License

MIT. See [LICENSE](LICENSE).
