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
windows (`hyprctl clients -j`) and records one `class → workspace` mapping per
app. A hidden `relaunch boot` hook in `~/.config/hypr/autostart.lua` launches
the list. Workspace pins live in generated `~/.config/omarchy-relaunch/relaunch.lua`
(`o.window` rules) and are loaded from `hyprland.lua`. Because that is
declarative config rather than live state, it survives a crash for free.

- **Bar widget (`BarWidget.qml` / `Panel.qml`):** Save, list edits, and boot
  policy. The last-boot log is a second plugin kind — a fullscreen overlay
  (`Overlay.qml`), not the cramped bar popover.
- **Engine (`relaunch`, a bash + jq script):** inventories windows and
  existing Hyprland startup apps, writes the Lua pins, and runs `relaunch boot`.
  The widget invokes the copy in the plugin folder.

## Install

Via Omarchy (recommended):

```sh
omarchy plugin add https://github.com/farmall856/omarchy-relaunch.git --enable
```

That clone is enough. Opening the widget (or just loading the bar) runs the
engine from the plugin folder and wires the hidden boot hook plus the
`relaunch.lua` source line. Optionally also:

```sh
git clone https://github.com/farmall856/omarchy-relaunch.git
cd omarchy-relaunch
./install.sh
```

`install.sh` copies `relaunch` into `~/.local/bin`, installs the plugin folder,
and wires the same hooks.

### Dependencies

- **bash** and **jq** — both ship with Omarchy Quattro. No compiler.
- **Hyprland / Omarchy Quattro** — the runtime.
- A Nerd Font in the bar (Omarchy default) for the widget glyph.

The engine runs `hyprctl`. No elevated privileges; everything runs as your
user, like any Hyprland command.

## Usage

1. Arrange your apps across workspaces the way you want them on boot.
2. Click the Relaunch icon in the bar.
3. **Save Startup App Workspaces** — captures the layout and writes the pins.
4. Existing Hyprland startup apps stay listed so you can add them to relaunch,
   delete a startup line, or leave one alone.
5. After a reboot, **View last boot log** opens a fullscreen overlay with the
   last `relaunch boot` diagnostic (what launched, where it landed).

Boot policy in the same panel:

- **Skip next boot** — one-shot; the next `relaunch boot` does nothing, then
  clears itself. The chip becomes **Enable on boot** until then.
- **Disable until re-enabled** — stays off across reboots until you enable it.
- **Remove Relaunch permanently** — two-click confirm; removes hooks, config,
  and the plugin folder.

## Configure

Fine-tune by editing `~/.config/omarchy-relaunch/config.json`:

```json
{
  "staggerSeconds": 0,
  "ignored": [],
  "skipOnce": false,
  "entries": [
    { "class": "herdr",         "workspace": 1, "exec": "xdg-terminal-exec --app-id=herdr herdr", "enabled": true },
    { "class": "brave-browser", "workspace": 2, "exec": "brave",                                 "enabled": true },
    { "class": "foot",          "workspace": 6, "exec": "foot",                                  "enabled": true }
  ]
}
```

- `class` matches a window's `initialClass` (stable across an app's post-launch
  class changes — Brave/Electron do this).
- `exec` overrides the guessed launch command; leave blank to use the guess.
- `enabled` keeps an entry on file but out of the generated pins and boot list.
- `float` (optional) floats that app.
- `skipOnce` is set by **Skip next boot**. Save keeps it; `relaunch boot`
  consumes it.
- `ignored` is startup-app ids you chose to leave alone.
- `staggerSeconds` waits N seconds between launches if apps race the pins.

After a manual edit, run `relaunch generate` (or Save again from the panel).

Runtime files (not in git):

- `~/.config/omarchy-relaunch/config.json` — entries and skip/ignore state
- `~/.config/omarchy-relaunch/relaunch.lua` — generated `o.window` pins
- `~/.config/omarchy-relaunch/disabled` / `skip-once` — boot flags
- `~/.config/omarchy-relaunch/last-boot.log` / `last-boot.json` — last `relaunch boot` diagnostic

## Remove

From the panel: **Remove Relaunch permanently** (click twice). That removes the
autostart hook, the `hyprland.lua` source line, `~/.config/omarchy-relaunch/`,
and the plugin folder.

Or from a terminal:

```sh
relaunch uninstall --yes
# or, if you only want the plugin checkout gone:
omarchy plugin remove io.github.laytonf.relaunch --yes
```

`omarchy plugin remove` does not unwind the Hyprland hooks or config dir;
use the panel action or `relaunch uninstall --yes` for a full teardown.

## Notes / limits

- One instance per app: an `o.window` class match sends every window of that
  class to its workspace, which is correct for a one-pinned-instance-per-app
  setup.
- Launch commands come from a small curated table because `/proc/<pid>/cmdline`
  is unreliable for Electron/flatpak/wrapped apps. Unknown apps fall back to a
  lowercased class; check `exec` in the config if one doesn't relaunch.
- A terminal hosting another command (`foot -e cmd`, `omarchy-launch-terminal cmd`)
  is identified by that command and relaunched with
  `xdg-terminal-exec --app-id=<cmd> <cmd...>` so Hyprland gets a distinct class.
- Terminals relaunch empty; lean on the app's own restore (tmux, herdr, etc.)
  for in-terminal context.

## License

MIT. See [LICENSE](LICENSE).
