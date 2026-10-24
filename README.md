# Relaunch

An Omarchy Quattro bar widget that saves your current app-to-workspace layout,
then relaunches those apps into the same workspaces after a reboot or crash.
Each app restores its own context; this makes sure it comes back up in the
right place.

Click the bar icon → **Capture open windows**. That's it. On the next
boot, herdr is back on workspace 1, Brave on 2, your terminal on 6 — wherever
you had them.

## How it works

No daemon. Nothing runs in the background unless you opt in to the session
snapshot below. When you save, the widget inventories running
windows (`hyprctl clients -j`) and records one `class → workspace` mapping per
app. The boot hook and the workspace pins live in two files Relaunch owns:
`~/.config/omarchy-relaunch/relaunch.lua` registers the hook, and
`rules.lua` holds the generated `o.window` pins. Both are reached through one
guarded line in `hyprland.lua`. **Relaunch never writes your
`autostart.lua`** — it reads it, to show you what already starts at login, and
that is all. Because the pins are declarative config rather than live state,
they survive a crash for free.

- **Bar widget (`BarWidget.qml` / `Panel.qml`):** Save, list edits, and boot
  policy, grouped into bordered sections. The last-boot log is a second plugin
  kind — a fullscreen overlay (`Overlay.qml`), not the cramped bar popover.
- **Engine (`relaunch`, a bash + jq script):** inventories windows and
  existing Hyprland startup apps, writes the Lua pins, and runs `relaunch boot`.
  The widget invokes the copy in the plugin folder.

## Install

Relaunch comes in two parts: the bar widget, from the Omarchy plugin
marketplace, and its **boot tools**, a small system package from the AUR.

```sh
omarchy plugin add https://github.com/farmall856/omarchy-relaunch.git --enable
```

Open the widget. Until the boot tools are installed it says so, explains what
they are, and offers **Install boot tools**.

> **Helper package required.** Relaunch restores apps at login through a small
> root-owned system service installed from the AUR as `omarchy-relaunch` (the
> "boot tools"): a polkit-gated signing authority, a login-time verifier that
> runs only a signature-verified launch list, a D-Bus activation file, and a
> systemd unit. The plugin will prompt to install it (polkit/sudo) on first use,
> or install it yourself with `omarchy pkg aur add omarchy-relaunch` and then
> `relaunch ensure-hooks --anchor` (which wires the login step into
> `hyprland.lua`). Remove it with `sudo pacman -Rns omarchy-relaunch`. The plugin never changes your config
> without an explicit Authorize.

The button runs exactly two fixed commands:

1. `omarchy pkg aur add omarchy-relaunch` in a terminal window, where the
   package builds and pacman asks for your password. The plugin itself never
   elevates.
2. Once the package's files are in place,
   `/usr/lib/omarchy-relaunch/relaunch ensure-hooks --anchor`, which adds the
   one login line to `~/.config/hypr/hyprland.lua`. Running it again changes
   nothing.

Nothing launches at login until you capture a layout and Authorize it.

Once they are installed, the panel's **Boot tools** section shows the
installed version and manages the package, each in a terminal window that
asks for your password:

- **Check for updates** asks the AUR for a newer version (no password, no
  terminal), and offers **Update** (`yay -S --needed omarchy-relaunch`) when
  there is one.
- **Reinstall** rebuilds the current version (`yay -S --rebuild
  omarchy-relaunch`), for damaged files or a half-finished update.
- **Uninstall** removes the package (`omarchy pkg drop omarchy-relaunch`).
  Your saved layout, your signing key and the widget are kept, so installing
  again brings everything back; until then nothing restores at login.

`omarchy pkg aur add` leaves an installed package as it is, so updates go to
yay, the AUR helper Omarchy uses, directly.

From a checkout instead, `./install.sh` installs the plugin folder and wires
the same login line (the boot tools package still comes from the AUR).

### Dependencies

- **Omarchy / Hyprland** — the runtime, and `omarchy pkg aur add` for the
  boot tools.
- **polkit**, **systemd**, **dbus** — pulled in by the `omarchy-relaunch`
  package.
- A Nerd Font in the bar (Omarchy default) for the widget glyph.

## Usage

1. Arrange your apps across workspaces the way you want them on boot.
2. Click the Relaunch icon in the bar.
3. **Capture open windows** — captures the layout as the boot list, replacing
   what was saved. If that would remove pinned apps that aren't open, the panel
   asks first. **Add open windows** instead adds the open apps and keeps every
   other pin — the one to use when only some of your apps are running. Either
   way you review the change and authorize it.

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
  field.
- **Trash can** — remove that app from the relaunch list.

An app whose launch command cannot be found is drawn in the urgent colour and
opens its editor by itself, since there is nothing else on a one-line row to
fix it with. Type the command that starts it and press **Save**.

### WINDOWS NOT IN RELAUNCH

Windows open right now that the relaunch list does not cover, one line each,
each with the workspace it is on. Nothing in this box starts at login: close
the window and its line goes with it.

- **+** — add that window to the relaunch list, on the workspace it is
  already on.

The box is hidden when every open window is already on the list.

### STARTUP APPS

Hyprland startup apps — the entries in your `autostart.lua`. These *do* start
when you log in, which is why they are kept apart from the windows above.

The list is read-only, and the rows have no buttons. Relaunch makes your app
windows reappear in their workspaces after a reboot or a crash; it is not an
autostart manager, and it never writes `autostart.lua`. The box is here so
that when something you did not expect turns up at login, you can see where it
came from — then edit `autostart.lua` yourself, or use whatever put it there.

An app that is on both lists appears on both. The box is hidden when your
`autostart.lua` has no entries.

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

### Session snapshot (manual, diagnostic)

Answers "what did I have before, and what actually came back?" Nothing else
reads it, and no part of save, restore or boot depends on it.

```sh
relaunch snapshot              # capture the layout right now
relaunch last-session --diff   # compare that snapshot with the last boot
```

Capture is always something you ask for. Relaunch does not sample your
windows on a timer or at shutdown, and there is no background service — an
earlier pre-shutdown hook was removed because it recorded window titles on a
schedule you could not see or control. Everything Relaunch keeps on disk is
visible and editable in the panel.

## Configure

Fine-tune by editing `~/.config/omarchy-relaunch/config.json`:

```json
{
  "staggerSeconds": 0,
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
- `staggerSeconds` waits N seconds between launches if apps race the pins.

After a manual edit, run `relaunch generate` (or Save again from the panel).

Runtime files (not in git):

- `~/.config/omarchy-relaunch/config.json` — entries and boot-skip state
- `~/.config/omarchy-relaunch/overrides.json` — class → exec exceptions you set
- `~/.config/omarchy-relaunch/relaunch.lua` — owned loader; registers the boot hook
- `~/.config/omarchy-relaunch/rules.lua` — generated `o.window` pins
- `~/.config/omarchy-relaunch/disabled` / `skip-once` — boot flags
- `~/.config/omarchy-relaunch/last-boot.log` / `last-boot.json` — last `relaunch boot` diagnostic
- `~/.config/omarchy-relaunch/last-session.json` — window snapshot, written only when you run `relaunch snapshot`

## Remove

Only the boot tools, keeping your layout and key: **Uninstall** in the panel's
Boot tools section (see Install).

Everything: **Remove Relaunch permanently** (click twice, then confirm the
password prompt). That deletes your signing key and signed state, removes the
`hyprland.lua` source line, `~/.config/omarchy-relaunch/` (loader, rules and
all state), and the plugin folder. The boot tools package stays; remove it
with `sudo pacman -Rns omarchy-relaunch`. Your `autostart.lua` is not touched, because
Relaunch never wrote to it.

Complete removal is the one thing Relaunch promises unconditionally: every file
it writes is removable, and after an uninstall Hyprland is left as it was
found.

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

### Apps that restore themselves

Some applications reopen their own windows at startup, independently of
Relaunch. Browsers are the common case: after an unclean shutdown — which a
reboot usually is — Brave restores its previous session, and that includes any
web app or PWA window it had open. Relaunch launches the same web app from its
saved entry, and you end up with two.

Relaunch cannot see this coming. At the moment `relaunch boot` runs, the
browser has not restored anything yet, so there is no window to detect and no
way to tell "this app will bring itself back" from "this app needs launching".
Waiting and then closing the extra window is not an option either: Relaunch
must never close a window it did not open.

The fix is to stop launching that app through Relaunch — remove the entry and
let the application restore itself, which it was going to do anyway. Everything
else on the list is unaffected.

Web apps are the sharpest version of this, because the browser remembers them
individually. A PWA that a browser restores on its own is better left off the
list entirely.

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
