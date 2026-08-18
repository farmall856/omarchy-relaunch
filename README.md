# Relaunch

An Omarchy Quattro bar widget that saves your current app-to-workspace layout,
then relaunches those apps into the same workspaces after a reboot or crash.
Each app restores its own context; this makes sure it comes back up in the
right place.

Click the bar icon → **Save Current Workspaces**. That's it. On the next
boot, herdr is back on workspace 1, Brave on 2, your terminal on 6 — wherever
you had them.

## How it works

No daemon. Nothing runs in the background unless you opt in to the session
snapshot below. When you save, the widget inventories running
windows (`hyprctl clients -j`) and records one `class → workspace` mapping per
app. A hidden `relaunch boot` hook in `~/.config/hypr/autostart.lua` launches
the list. Workspace pins live in generated `~/.config/omarchy-relaunch/relaunch.lua`
(`o.window` rules) and are loaded from `hyprland.lua`. Because that is
declarative config rather than live state, it survives a crash for free.

- **Bar widget (`BarWidget.qml` / `Panel.qml`):** Save, list edits, and boot
  policy, grouped into bordered sections. The last-boot log is a second plugin
  kind — a fullscreen overlay (`Overlay.qml`), not the cramped bar popover.
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
3. **Save Current Workspaces** — captures the layout and writes the pins. The
   status line reports how many entries were added and updated.

The panel is titled **relaunch**, with a **?** icon opposite it on the same
line that opens this README on GitHub. Below the save button the panel is
bordered boxes, each with its title sitting on the top border rather than
inside it. Every icon names itself on hover.

### Workspace boxes

Under a **Relaunch list (N)** heading — or **No apps on the relaunch list
yet** — comes one box per workspace, titled **Workspace 1**, **Workspace 2**,
and so on, holding the apps pinned to that workspace. Each app is a single
line: its name, then two icons.

- **Rocket** — show the launch command. It opens an editable field with a
  **Save** button, so the same icon both views and edits it. `Esc` closes the
  field. For an app that is also a Hyprland startup app, a **Delete startup
  config** button appears here too.
- **Trash can** — remove that app from the relaunch list.

An app whose launch command cannot be found is drawn in the urgent colour and
opens its editor by itself, since there is nothing else on a one-line row to
fix it with. Type the command that starts it and press **Save**.

### NOT IN RELAUNCH

One box for everything not on the relaunch list — running windows, Hyprland
startup apps you have not imported, and startup apps you told Relaunch to
leave alone. One line each.

- **+** — add that app to the relaunch list. A running window is added on the
  workspace it is already on. A startup app has no window to read a workspace
  from, so it is added on workspace 1; launch it, put it where you want it,
  and save again to move it.
- **Crossed-out eye** — leave a startup app alone. It stops being offered for
  import but stays listed, dimmed, so you can see the choice was deliberate.
- **Eye** — stop leaving it alone, putting it back in the list above.

### RELAUNCH ON BOOT

Boot policy and the last-boot report, in one box.

- **Skip next boot** — one-shot; the next `relaunch boot` does nothing, then
  clears itself. The chip becomes **Enable on boot** until then.
- **Disable until re-enabled** — stays off across reboots until you enable it.
- **View last boot log** — opens a fullscreen overlay with the last
  `relaunch boot` diagnostic: what launched, and where it landed. The line
  above it summarises the last boot at a glance.

### Remove Relaunch permanently

Centred on its own at the very bottom of the panel, in no section. Two-click
confirm; removes the hooks, the config, and the plugin folder.

### Session snapshot (optional, diagnostic)

Off by default. It answers "what did I have before, and what actually came
back?" — nothing else reads it, and no part of save, restore or boot depends
on it.

```sh
relaunch snapshot-hook --enable    # install the opt-in hook
relaunch snapshot                  # or capture the layout right now
relaunch last-session --diff       # compare that snapshot with the last boot
relaunch snapshot-hook --disable   # remove it
```

`--enable` installs a systemd **user** unit that runs `relaunch snapshot` when
the graphical session stops, writing `~/.config/omarchy-relaunch/last-session.json`
with every window's class, label, workspace, float state, monitor, pid and
title. It is ordered so its `ExecStop` runs while Hyprland is still up, and
carries a short `TimeoutStopSec` so a hung compositor socket cannot stall
logout. A oneshot unit is not a daemon: nothing is resident between runs.

`last-session --diff` reports each class as `ok`, `MISSING` (fewer windows
came back than you had) or `MOVED` (a window came back on the wrong workspace
or with the wrong float/tile state).

`relaunch uninstall --yes` removes the hook along with everything else.

## Configure

Fine-tune by editing `~/.config/omarchy-relaunch/config.json`:

```json
{
  "staggerSeconds": 0,
  "ignored": [],
  "skipOnce": false,
  "entries": [
    { "class": "herdr",         "workspace": 1, "exec": "xdg-terminal-exec --app-id=herdr -e herdr", "enabled": true },
    { "class": "brave-browser", "workspace": 2, "exec": "brave",                                 "enabled": true },
    { "class": "foot",          "workspace": 6, "exec": "foot",                                  "enabled": true }
  ]
}
```

- `class` matches a window's `initialClass` (stable across an app's post-launch
  class changes — Brave/Electron do this).
- `exec` overrides the guessed launch command; leave blank to use the guess.
- `enabled` keeps an entry on file but out of the generated pins and boot list.
- `float` is captured from the live window and refreshed on every save:
  `true` pins the app floating, `false` pins it tiled. Omitting it entirely
  leaves float alone, which lets Omarchy's own rules decide.
- `skipOnce` is set by **Skip next boot**. Save keeps it; `relaunch boot`
  consumes it.
- `ignored` is startup-app ids you chose to leave alone.
- `staggerSeconds` waits N seconds between launches if apps race the pins.

After a manual edit, run `relaunch generate` (or Save again from the panel).

Runtime files (not in git):

- `~/.config/omarchy-relaunch/config.json` — entries and skip/ignore state
- `~/.config/omarchy-relaunch/overrides.json` — class → exec exceptions you set
- `~/.config/omarchy-relaunch/relaunch.lua` — generated `o.window` pins
- `~/.config/omarchy-relaunch/disabled` / `skip-once` — boot flags
- `~/.config/omarchy-relaunch/last-boot.log` / `last-boot.json` — last `relaunch boot` diagnostic
- `~/.config/omarchy-relaunch/last-session.json` — pre-shutdown window snapshot, only if you enabled the hook

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

## Limits

### One workspace per app

Relaunch keeps **one entry per window class**, and the rule it generates is a
*standing* Hyprland rule, not a one-shot placement at boot:

```lua
o.window({ class = "^(brave-browser)$" }, { workspace = "2 silent" })
```

That rule has no expiry. It applies to **every** window of that class, for the
whole session — not just the ones `relaunch boot` starts. Verified on the
development machine: with Brave pinned to workspace 2, launching Brave from
workspace 7 opened it on workspace 2. The same happens with `foot`.

So two windows of one app on two different workspaces can be neither restored
nor kept:

- Save records only the first one seen (the lowest workspace).
- Even if both were recorded, the standing rule would pull both to the same
  workspace as soon as they opened.

This is the ceiling of the approach, not a bug to be filed. Hyprland window
rules match on **class**, not on a particular window instance, so there is
nothing to attach a second, different placement to. Terminals are the usual
way to hit this: every plain `foot` window shares the class `foot`. A terminal
hosting a command is a way out, because it gets its own class — `foot -e herdr`
is captured as class `herdr`, separate from plain `foot`, and can hold its own
workspace.

### Multiple monitors

Relaunch restores apps to workspace **numbers**, not to screens. Which monitor
a given workspace lives on is Hyprland's business, and Relaunch does not
express an opinion about it.

Omarchy ships no workspace→monitor binding at all — only keybindings to move a
workspace to another monitor by hand (`SUPER + SHIFT + ALT + arrow`, see
`default/hypr/bindings/tiling.lua`). So:

- **With your own workspace→monitor rules**, restore follows them: your apps
  land on the workspaces they were saved on, and those workspaces land on the
  screens you assigned.
- **Without them**, workspace placement across screens is whatever Hyprland
  decides at the time. Your apps still come back on the right workspace
  *numbers*, but which physical screen shows them is not guaranteed to match
  what you had.

Only single-monitor use has actually been verified. Multi-monitor should
follow from the above, but it is untested — treat it as such.

## Notes

- Launch commands: terminal wrap, then `overrides.json` (your exceptions;
  starts empty), then `gio launch` of the matching `.desktop` (your
  `~/.local/share/applications` first, then the system dirs), then the
  process command line, then a lowercased class. The panel flags unverified
  guesses and lets you type a command when the binary is missing.
- A terminal hosting another command (`foot -e cmd`, `omarchy-launch-terminal cmd`)
  is identified by that command and relaunched with
  `xdg-terminal-exec --app-id=<cmd> -e <cmd...>` so Hyprland gets a distinct
  class. Argument boundaries are preserved, so `bash -c 'dua i /'` survives
  the round trip.
- Terminals relaunch empty; lean on the app's own restore (tmux, herdr, etc.)
  for in-terminal context.

## License

MIT. See [LICENSE](LICENSE).
