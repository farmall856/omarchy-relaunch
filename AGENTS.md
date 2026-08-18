# Relaunch

Omarchy Quattro bar widget that snapshots the current app→workspace layout
and writes Hyprland Lua pins so those apps come back on the same workspaces
after a reboot or crash.

This folder is the source tree. The public repo is
`github.com/farmall856/omarchy-relaunch`; plugin id is
`io.github.laytonf.relaunch`. Do not rename the id without updating
`manifest.json`, QML `moduleName`, `install.sh`, and `git-init.sh`.

## Architecture

No daemon. Save inventories windows; `relaunch boot` (a hidden autostart
hook) launches the list. Workspace pins live in generated `relaunch.lua`.

| Piece | Role |
|---|---|
| `BarWidget.qml` | Bar button. Follows the `omarchy.clock` popup contract. |
| `Panel.qml` | Popup UI. Shells out to `relaunch`; never calls `hyprctl` itself. |
| `Overlay.qml` | Fullscreen last-boot log. Same Item/open/close contract as `omarchy.emojis`. |
| `relaunch` | bash + jq CLI. Inventory, list edits, boot policy, rule generation. |
| `manifest.json` | Omarchy plugin schema v1, `kinds: ["bar-widget", "overlay"]`. |

Runtime files (user-owned, never commit):

- `~/.config/omarchy-relaunch/config.json` — entries, ignored startup ids, `skipOnce`
- `~/.config/omarchy-relaunch/overrides.json` — user `class → exec` exceptions (starts empty)
- `~/.config/omarchy-relaunch/relaunch.lua` — generated `o.window` pins
- `~/.config/omarchy-relaunch/disabled` / `skip-once` — boot flags (skip is also in config.json so Save cannot drop it)
- `~/.config/omarchy-relaunch/last-boot.log` / `last-boot.json` — last `relaunch boot` diagnostic
- `~/.config/omarchy/plugins/io.github.laytonf.relaunch/` — installed QML copy

Do not generate `relaunch.conf` / `windowrulev2`. Pins are Lua only.
`ensure_hooks` and `write_generated` delete a leftover `relaunch.conf` and
any `source = …/relaunch.conf` line in `hyprland.conf`.

`install.sh` copies the engine and plugin files, then runs `ensure-hooks`
(hidden `o.exec_on_start("relaunch boot")` in `autostart.lua` and
`dofile(.../relaunch.lua)` in `hyprland.lua`). It does not touch
`hyprland.conf`. Never show that hook in the inventory.

## Engine CLI

```
relaunch save [--json]                 # capture running layout
relaunch generate [--json]             # rebuild rules
relaunch list [--json]                 # entries + startup inventory + rows + boot
relaunch reload
relaunch boot                          # hidden autostart hook
relaunch import --class CLASS --workspace N
relaunch import --exec CMD --workspace N
relaunch set-exec --class CLASS --exec CMD
relaunch drop --class CLASS
relaunch drop-startup --id ID
relaunch ignore --id ID
relaunch unignore --id ID
relaunch boot-skip | boot-disable | boot-enable
relaunch last-boot [--json] [--open]
relaunch uninstall --yes
```

`--json` is the widget contract. Keep `ok`, `error`, `added`, `updated`,
`entries`, `rows`, `startup`, `ignored`, `boot`, `snippetPath`, `configPath`
stable. `Panel.qml` parses that object.

## Invariants

- One entry per window class. Capture keeps the first seen (lowest workspace).
  A `class:^(brave-browser)$` rule sends every Brave window to that workspace.
- Match `initialClass`, not `class`. Brave/Electron mutate class after launch.
- A terminal hosting another command (`foot -e cmd`, `omarchy-launch-terminal cmd`)
  is identified by that command, and relaunched with
  `xdg-terminal-exec --app-id=<cmd> <cmd...>` so Hyprland gets a distinct class.
  Do not special-case individual wrappers like herdr.
- Launch commands: `terminal` → `overrides.json` → `.desktop` (via
  `gio launch <file>`, user `~/.local/share/applications` first, then
  `$XDG_DATA_DIRS`) → cmdline → lowercased class (`guess`).
  `overrides.json` starts empty and only grows when the user saves a
  command. `--json` includes `execSource`, `execOk`, `unverified`, and
  `warnings`.
- `list` (panel display) must not index `.desktop` files or resolve
  launchers. It reads saved rows, cheap terminal identity, and startup
  lines. Resolve only on `save` and `import`.
- Recapture refreshes workspace only (and heals a fallback exec). Preserve
  user `exec`, `enabled`, and `float` edits.
- Skip special/negative workspaces (`Workspace.ID < 1`) and empty classes.
- Regex-escape class names in generated `o.window` lines, then Lua-escape
  backslashes so dotted classes (`org.omarchy.agent`) are valid Lua strings.
- Persist with temp-file + rename (`config.json.tmp`, `relaunch.lua.tmp`).
- Everything runs as the user. No sudo, no IPC beyond `hyprctl` and the
  `relaunch` script on `PATH`.
- Inventory only the user's `~/.config/hypr/autostart.lua`. Never list or
  edit packaged Omarchy autostart. Hide any `relaunch` / `omarchy-relaunch`
  hook from every list.
- Existing startup lines are not deleted unless the user chooses "Delete
  startup config". "Leave alone" records the id in `ignored` and still
  shows the row when editing.

## QML conventions

Mirror `omarchy.clock` / `omarchy.weather`:

- Root type is `BarWidget` with `moduleName` matching `manifest.json` `id`.
- Expose `opened`, `open()`, `close()`, `toggle()`, `closeForPopoutSwitch()`,
  and `injectPanel()` so the bar can route popouts.
- Theme via `root.barForeground` / `root.barBackground`, `Style.space()`,
  `Style.font.*`. No hardcoded palette.
- Use `Quickshell.Io.Process` to run `relaunch`. The script must be on `PATH`
  (`~/.local/bin` after `install.sh`).
- Overlay is a second kind, not a replacement for the panel. Bar click still
  toggles the small panel; "View last boot log" summons the overlay via
  `omarchy-shell shell summon io.github.laytonf.relaunch`. There is no
  `Overlay {}` type — copy `omarchy.emojis` (`Item` + `opened`/`open`/`close`).
- After QML edits in a live install, `omarchy-shell shell rescanPlugins`
  (user plugin dir hot-reloads on save; this tree does not). A new `kinds`
  entry needs `omarchy restart shell`.

Do not edit `/usr/share/omarchy/`. Read it for the plugin contract.

## Build, test, install

```bash
# Engine
./tests/relaunch-test.sh
install -m 0755 ./relaunch ~/.local/bin/relaunch

# Plugin schema (must stay green)
omarchy plugin validate .

# QML (qt6 qmllint; include the shell import path)
/usr/lib/qt6/bin/qmllint -I "$OMARCHY_PATH/shell" BarWidget.qml Panel.qml Overlay.qml

# Local install: script + plugin copy + ensure-hooks
./install.sh
omarchy bar move io.github.laytonf.relaunch --section right   # first time
```

Prefer `./install.sh` over copying files by hand. Once published:

```bash
omarchy plugin add https://github.com/farmall856/omarchy-relaunch.git --enable
```

`omarchy plugin add <repo-url> --enable` is the supported install. The
widget invokes `./relaunch` from the plugin folder (not PATH). First
`list`/`save`/`generate` runs `ensure_hooks` so autostart and
`hyprland.lua` get wired without `install.sh`.

## Repo / publish

Not initialized as a git repo yet. One-time:

```bash
./git-init.sh farmall856
# then: gh repo create farmall856/omarchy-relaunch --public --source=. --remote=origin --push
```

Marketplace submit:
https://github.com/HANCORE-linux/omarchy-plugin-marketplace/issues/new?template=submit-plugin.yml

`.gitignore` already excludes generated `config.json` (and a leftover
`relaunch.conf` name from older installs). Keep it that way.

## Out of scope unless asked

- Live window tracking / a background daemon
- Per-window (not per-class) restore
- Restoring terminal session contents (tmux/herdr's job)
- Editing Hyprland or Omarchy packaged files
