# Relaunch

An Omarchy Quattro bar widget that records which apps you have open on which
workspaces, and opens them again in the same places after a reboot or crash.
Each app restores its own context; Relaunch makes sure it comes back, in the
right place.

Click the bar icon, **Capture open windows**, review the list, **Authorize**.
On the next login herdr is back on workspace 1, Brave on 2, your terminal on 6
— wherever you had them.

## How it works

There is no daemon and nothing samples your windows. Relaunch acts only when
you click something in its panel, and once at login.

1. **Capture.** The panel's engine (a Rust binary from the boot tools package)
   reads the open windows from `hyprctl clients -j` and works out the command
   that opens each one (see [How a launch command is found](#how-a-launch-command-is-found)).
   The result is a *proposed* boot list: one pin per app per workspace.
2. **Review and Authorize.** The panel shows what will open at login. When
   you click **Authorize**, the system asks for your password or fingerprint
   (polkit), and the Relaunch service signs the list with a key only it can
   use. See [Authorization and security](#authorization-and-security).
3. **Login.** One line in `hyprland.lua` loads a root-owned loader, which runs
   the root-owned verifier, `relaunch-anchor`. It checks the list's signature
   and then starts each enabled pin. Each pin has its own private launcher
   file. The anchor reads that file once, checks that its SHA-256 is the
   signed one and that its command and working directory are exactly the
   signed ones, and starts that command directly — never through a shell,
   `gio` or `gtk-launch` — under `uwsm-app --`. That is how Omarchy's own
   launchers start apps, so restored apps land in the session's
   `app-graphical.slice` like anything else you open.
4. **Placement.** As each new window appears, the anchor recognises it (a
   window address that was not there before, with the pin's window class)
   and moves it to its workspace, tiled or floating as captured. Nothing
   stays behind as a window rule: windows you open later are not touched.

**Relaunch never writes your `autostart.lua`.** It reads it, to show you what
else starts at login, and that is all.

## Install

Relaunch comes in two parts: the bar widget, from the Omarchy plugin
marketplace, and its **boot tools**, a small system package from the AUR.

```sh
omarchy plugin add https://github.com/farmall856/omarchy-relaunch.git --enable
```

Open the widget. Until the boot tools are installed it says **Relaunch needs
its boot tools**, explains what they are, and offers **Install boot tools**.

> **Helper package required.** Relaunch restores apps at login through a small
> root-owned system package installed from the AUR as `omarchy-relaunch` (the
> "boot tools"): a polkit-gated signing authority, a login-time verifier that
> runs only a signature-verified launch list, a D-Bus activation file, and a
> systemd unit. The panel installs it for you, or install it yourself with
> `omarchy pkg aur add omarchy-relaunch` and then
> `relaunch ensure-hooks --anchor` (which wires the login step into
> `hyprland.lua`). The plugin never changes your config without an explicit
> Authorize.

**Install boot tools** runs exactly two fixed commands:

1. `omarchy pkg aur add omarchy-relaunch` in a terminal window, where the
   package builds and pacman asks for your password. The plugin itself never
   elevates. If the terminal outlives the install, click **I'm done**;
   **Check again** looks for the package again.
2. Once the package's files are in place,
   `/usr/lib/omarchy-relaunch/relaunch ensure-hooks --anchor`, which adds the
   one login line to `~/.config/hypr/hyprland.lua`. Running it again changes
   nothing. If this step fails the button reads **Finish setup** and retries
   it.

Nothing launches at login until you capture a layout and Authorize it.

Once they are installed, the panel's **Boot tools ·** line shows the installed
version (or **development install** for a build that pacman does not own,
which the panel cannot manage). Its buttons each run in a terminal window that
asks for your password, except the update check:

- **Check for updates** asks the AUR for a newer version (no password, no
  terminal), and offers **Update** (`yay -S --needed omarchy-relaunch`) when
  there is one.
- **Reinstall** rebuilds the current version (`yay -S --rebuild
  omarchy-relaunch`), for damaged files or a half-finished update. It asks
  first.
- **Uninstall** removes the package (`omarchy pkg drop omarchy-relaunch`) after
  asking **Remove the boot tools?**. Your saved layout, your signing key and
  the widget are kept, so installing again brings everything back; until then
  nothing restores at login.

`omarchy pkg aur add` leaves an installed package as it is, so updates go to
yay, the AUR helper Omarchy uses, directly.

From a checkout instead, `./install.sh` installs the plugin folder and wires
the same login line (the boot tools package still comes from the AUR).

### Dependencies

- **Omarchy / Hyprland**, with `uwsm` (Omarchy's session manager) and
  `omarchy pkg aur add` for the boot tools.
- **polkit**, **systemd**, **dbus**: pulled in by the `omarchy-relaunch`
  package.
- A Nerd Font in the bar (Omarchy default) for the widget glyph.

## Usage

1. Arrange your apps across workspaces the way you want them at login.
2. Click the Relaunch icon in the bar.
3. Click one of the two capture buttons:
   - **Capture open windows**: replaces the boot list with the apps open now.
     If that would remove pinned apps that aren't open, the panel asks first
     (**Replace** / **Keep them**).
   - **Add open windows**: adds the apps open now and keeps every other pin.
     Use it when only some of your apps are running.
4. The **Authorize launch changes** card shows what will open at login
   ("Relaunch will open these N apps at login:", each with its workspace,
   marked *(new)* or *(changed)*), with the exact change under **Details**.
   Click **Authorize** and confirm the system prompt, or **Cancel**.

The panel is titled **relaunch**, with a **?** icon on the same line that
opens this README. Below the capture buttons the panel is a series of bordered
boxes, each titled on its top border. Every icon names itself on hover.

### The boot list

Under a **Boot list (N)** heading (or **No apps on the boot list yet**) comes
one box per workspace, titled **Workspace 1**, **Workspace 2**, and so on,
holding the apps pinned there. Each app is one line: its name, then two icons.
A name followed by **(command line)** was captured from the window's own
command line rather than from an installed app launcher. That is normal for a
terminal or a script, not a warning.

- **Rocket** (**View or edit launch command**): opens the row. It shows the
  window class the pin matches, and an editable launch-command field with a
  **Save** button. Hover text becomes **Hide launch command**; `Esc` closes
  the row and discards the edit. Saving a new command is reviewed and
  authorized like a capture. See [Change an app's launch command](#change-an-apps-launch-command).
- **Trash can** (**Remove**): removes that app from the boot list. Removing
  only stops a launch, so it needs no password.

An app whose launch command cannot be found on your system is drawn in the
urgent colour and opens its row by itself, with **Launch command not found.
Type the command that starts this app.**

### WINDOWS NOT IN RELAUNCH

Windows open right now that the boot list does not cover, one line each, with
the workspace each is on (`ws 3 · Files`). Nothing in this box starts at
login: close the window and its line goes with it.

- **+** (**Add to relaunch on workspace N**): adds that window to the boot
  list where it is, after the same review and Authorize.

The box is hidden when every open window is already on the list.

### STARTUP APPS

The entries in your `autostart.lua`. These *do* start at login, which is why
they are kept apart from the windows above. The list is read-only: Relaunch is
not an autostart manager and never writes `autostart.lua`. The box is there so
that when something you did not expect turns up at login, you can see where it
came from. An app on both lists appears on both; the box is hidden when
`autostart.lua` has no entries.

### RELAUNCH ON BOOT

Boot policy, launch timing and the last-boot report. The first line says
**Runs on boot.**, **Will skip the next boot only.** or **Disabled until you
re-enable it.**

- **Skip next boot**: the next login launches nothing, then clears itself.
  While it is set the chip reads **Enable on boot**.
- **Disable until re-enabled**: stays off across reboots until you click
  **Enable on boot**.
- **Wait before (ms)** / **Wait after (ms)**: delays around each launch of
  apps that could race each other (a browser and its web apps, or two windows
  of one terminal). Different apps start side by side. Change a value, then
  **Apply delays**, which is reviewed and authorized.
- **Last boot log · …**: a one-line summary of the last login (for example
  `· 5 of 6 placed · 2026-10-24 09:31:02`).
- **View last boot log**: opens the full report in a fullscreen overlay. See
  [Troubleshooting](#troubleshooting-an-app-didnt-come-back).

None of these flags needs a password: they can only launch less, never more.

### Boot tools

The installed version and its **Check for updates** / **Update**,
**Reinstall** and **Uninstall** buttons. See [Install](#install).

### Remove relaunch permanently

Centred on its own at the bottom. Click once and it reads **Confirm removing
relaunch**; click again and confirm the password prompt. See [Remove](#remove).

### Session snapshot (manual, diagnostic)

Answers "what did I have before, and what actually came back?" Nothing else
reads it, and nothing in capture, authorization or boot depends on it.

```sh
relaunch snapshot              # record the open windows right now
relaunch last-session --diff   # compare that snapshot with the last boot
```

It is only ever written when you run it.

## Authorization and security

Relaunch starts programs as you at every login, so it treats its boot list as
something only you can approve.

- **Nothing unsigned launches.** A capture only *proposes* a boot list. The
  list that runs at login is the one you reviewed and authorized: it is signed
  with a per-user key, and the anchor verifies that signature before it
  starts anything. If the signature does not verify, nothing launches.
- **You approve every expansion.** Capturing, adding a window, changing a
  launch command and changing the delays each open the **Authorize launch
  changes** card. **Authorize** then shows the system authentication prompt
  (password or fingerprint) with one line: *Approve this change to your
  Relaunch login config.* The card is the record of what you are approving;
  the prompt only confirms it is you. The panel's hint says which click it
  belongs to (for example *Approve only if you just clicked Capture open
  windows.*); if you did not just do that, **Cancel**.
- **Reducing needs no password.** **Remove**, **Skip next boot**, **Disable
  until re-enabled** and **Enable on boot** can only make less start, so they
  apply without a prompt.
- **Editing a launcher breaks it, on purpose.** Each pin's launcher is a
  private file whose SHA-256 is part of the signed list. A launcher changed
  after you authorized it is refused at login (`verify-failed`), and so is a
  hand-edited `config.json` (its signature no longer matches). The way to
  change a command is the panel, which asks you to authorize the new one.
- **Only your own windows are eligible.** A window can be captured only if it
  is a native Wayland window whose process runs on the host as you: in your
  own mount, user and PID namespaces. XWayland windows and windows from
  sandboxes or containers are never captured from their own command line,
  because a sandboxed program can rewrite its command line to anything. At
  most they get an installed system app launcher, proposed disabled. A
  command running inside a terminal is captured only when it was started by
  your own host shell.
- **Why the boot tools run as root.** The signing key lives in
  `/var/lib/omarchy-relaunch/<uid>/` where only root can read it, so nothing
  running as you, malicious or not, can sign a boot list. The service
  (`relaunchd`) signs only after polkit has confirmed you, and the verifier and
  its loader are root-owned so nothing running as you can replace them.
- **Complete removal.** Every file Relaunch writes can be removed, and
  **Remove relaunch permanently** leaves Hyprland as it found it. See
  [Remove](#remove).

## Troubleshooting: an app didn't come back

Open the panel, then **View last boot log**. The top lines give the overall
outcome; then each launch is one line:

```
  ws 3  Files  started  pid=4120  NO-SHOW (no window)  /usr/bin/nautilus --gapplication-service
```

The workspace, the app, how its start went, then where its window ended up,
then the command that ran.

**Overall outcome**

| `outcome:` | Meaning | What to do |
|---|---|---|
| `landed` | Every app started and was placed. | Nothing. |
| `launched` | Everything started, but not every window was placed. | Read the app lines. |
| `unverified` | The boot list's signature did not verify. Nothing launched. | **Capture open windows** and **Authorize** again. |
| `schema-upgrade-needed` | The list was saved by an older Relaunch. | Capture and Authorize again. |
| `disabled` / `skipped` | A boot flag was set. | **Enable on boot** if you didn't mean it. |

**One app's line**

| On the line | Meaning | What to do |
|---|---|---|
| `placed` | Its window came up and was moved to its workspace. | Nothing. |
| `MISPLACED on ws N` | Its window came up, but ended on workspace N. | Look for a window rule of yours, or the app itself, moving it. |
| `NO-SHOW (no window)` | The command ran, but no window of that app appeared. | The captured command does not open a window: [change it](#change-an-apps-launch-command). |
| `verify-failed (…)` | Its launcher no longer matches what you signed. | Capture and Authorize again. |
| `not_found (command not found: …)` | The program is not installed, or not on your `PATH`. | Install it, or [change the command](#change-an-apps-launch-command). |
| `failed (…)` | It could not start, or exited at once. | Run the command in a terminal to see why. |
| *(not in the log)* | It is not on the boot list, or it is disabled. | Capture it, or use **+** in **WINDOWS NOT IN RELAUNCH**. |

### Change an app's launch command

1. Open the panel and find the app in its **Workspace** box.
2. Click its **rocket** icon. The row opens with the current command in the
   field.
3. Replace it with a command that opens the window, then click **Save** (or
   press Enter).
4. Review the **Authorize launch changes** card and click **Authorize**.

The field takes a command line as you would type it in a terminal. Plain
words are run directly; anything that needs a shell (pipes, `&&`, `~`,
variables) is run through `bash -c`.

Typical fixes:

- **An app that came back as a background service (`NO-SHOW`).** Some apps
  run a hidden service process, and that is what was captured. Use the
  command that opens a window, the one its own launcher uses: for Files,
  `nautilus --new-window`. (Current Relaunch already avoids the GApplication
  service case; this is the fix for an older capture.)
- **A terminal that came back empty.** Name the command after the terminal's
  own option: `foot -e btop`, or with an app id of its own,
  `foot --app-id=btop -e btop`.
- **A web app.** Use its launcher's command, for example
  `omarchy-launch-webapp https://maps.google.com`.
- **A row in the urgent colour.** Its command was not found: type one that
  is, **Save**, **Authorize**.

**An app that never gets on the list.** If Relaunch cannot find a command it
can trust for a window, it does not propose one. **+** then reports *no
verifiable launch command*. Start the app from its launcher (or with a
command line of its own) and capture again.

**Two identical windows.** Windows with the same class, workspace and command
are one pin; the review says *N open windows were identical duplicates and
merged*. To keep both, put them on different workspaces, or give one a
command of its own (for a terminal, a different `--app-id` or command).

**Two of an app where you had one.** Some apps reopen their own windows; see
[Apps that restore themselves](#apps-that-restore-themselves). Remove that app
from the boot list.

## How a launch command is found

For each window, two candidates are weighed. No application is named or
special-cased.

- **D**: the installed app launcher (`.desktop` file) that claims the window's
  class: by `StartupWMClass`, its file name, a web-app identity, or its
  `Name`. Its `Exec` line, with field codes like `%U` dropped. An app whose
  launcher is D-Bus activated still has a plain `Exec` (`nautilus
  --new-window`), and that is what is used. A `Terminal=true` launcher gives
  none.
- **P**: the command line of the window's own process, after walking past
  helper subprocesses (anything with `--type=`).

P is the window's command when it runs the same program as D, or when there
is no D. It is kept whole, with its flags and its working directory. P is not
used when its words were overwritten (Chromium and Electron do this), when its
process also owns a window of another app, or when it is a GApplication
background service (`--gapplication-service`, which opens no window). Then D
is used. With neither, the window is an unsigned guess, shown but never
proposed.

A **terminal** whose own command line names nothing to run is looked inside:
if a command is running in its foreground, the pin is the terminal running that
command (`foot -e btop`, in that command's directory); otherwise the bare
terminal, in its shell's current directory. Terminals are recognised by their launcher's
`Categories=TerminalEmulator`, never by name. This includes programs the kernel
hides from you, such as btop (which carries file capabilities), provided your
own host shell started them.

**Web apps** are matched to their launcher by the web-app identity in their
window class, so each one gets its own launch command.

## Files

Per user, in `~/.config/omarchy-relaunch/`:

- `config.json`: the signed boot list. Do not edit it: an edited list no
  longer verifies, and nothing launches.
- `launchers/relauncher-<id>.desktop`: one private launcher per pin, bound by
  its SHA-256.
- `disabled`, `skip-once`: the boot flags.
- `last-boot.json`, `last-boot.log`: the last login's report.
- `last-session.json`: only when you run `relaunch snapshot`.
- `rules.lua`: an inspection copy; nothing loads it.

Root-owned: the signing key and public key in
`/var/lib/omarchy-relaunch/<uid>/`, and the boot tools in
`/usr/lib/omarchy-relaunch/` and `/usr/share/omarchy-relaunch/`. In your
`hyprland.lua`, Relaunch adds one line that loads the root-owned
`anchor.lua`.

## Remove

Only the boot tools, keeping your layout and key: **Uninstall** in the panel's
Boot tools section (see [Install](#install)).

Everything: **Remove relaunch permanently**, then **Confirm removing
relaunch**, then the password prompt. That deletes your signing key and signed
state, removes the line from `hyprland.lua`, `~/.config/omarchy-relaunch/`,
and the plugin folder. The boot tools package stays; remove it with
`sudo pacman -Rns omarchy-relaunch`. Your `autostart.lua` is not touched,
because Relaunch never wrote to it.

Complete removal is the one thing Relaunch promises unconditionally: every file
it writes is removable, and after an uninstall Hyprland is left as it was
found.

Or from a terminal:

```sh
relaunch uninstall --yes
```

`omarchy plugin remove io.github.laytonf.relaunch --yes` removes only the
plugin folder; it does not take out the login line, the config or the key.

## Limits

### Same app, several windows

A pin is one app, on one workspace, running one command. Windows of one app
on *different* workspaces are separate pins and each comes back on its own
workspace. Windows identical in all three are merged into one pin (the review
says so). Placement moves only the windows a login opened, so a window you
open later goes wherever Hyprland puts it.

### Apps that restore themselves

Some applications reopen their own windows at startup, independently of
Relaunch. Browsers are the common case: after an unclean shutdown — which a
reboot usually is — Brave restores its previous session, including any web
app or PWA window it had open. Relaunch launches the same web app from its
pin, and you end up with two.

Relaunch cannot see this coming. At login the browser has not restored
anything yet, so there is no window to detect, and Relaunch never closes a
window it did not open. Remove that app from the boot list and let it restore
itself. A PWA that the browser restores on its own is better left off the list
entirely.

### Multiple monitors

Relaunch restores apps to workspace **numbers**, not to screens. Which monitor
a workspace is on is Hyprland's business. With your own workspace→monitor
rules, restore follows them; without them, your apps still come back on the
right workspace numbers, but which screen shows them is whatever Hyprland
decides. Only single-monitor use has been verified.

### In-app state

Relaunch reopens windows; what is inside them is up to each app. A terminal
comes back running the command it was running, not its scrollback. Lean on the
app's own session restore (tmux, herdr, the browser) for that.

## License

MIT. See [LICENSE](LICENSE).
