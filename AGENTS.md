# Relaunch

Omarchy Quattro bar widget that snapshots the current app→workspace layout
and writes a Hyprland snippet so those apps come back on the same workspaces
after a reboot or crash.

This folder is the source tree. The public repo name is `omarchy-relaunch`
(`github.com/laytonf/omarchy-relaunch`); plugin id is
`io.github.laytonf.relaunch`. Do not rename the id without updating
`manifest.json`, QML `moduleName`, `install.sh`, and `git-init.sh`.

## Architecture

No daemon. Save is a one-shot inventory; restore is declarative Hyprland
config sourced at startup.

| Piece | Role |
|---|---|
| `BarWidget.qml` | Bar button. Follows the `omarchy.clock` popup contract. |
| `Panel.qml` | Popup UI. Shells out to the `relaunch` binary; never calls `hyprctl` itself. |
| `engine/` | Go CLI (`github.com/laytonf/omarchy-relaunch/engine`). All Hyprland I/O and snippet generation. |
| `manifest.json` | Omarchy plugin schema v1, `kinds: ["bar-widget"]`. |

Runtime files (user-owned, never commit):

- `~/.config/omarchy-relaunch/config.json` — editable table
- `~/.config/omarchy-relaunch/relaunch.conf` — generated `windowrulev2` + `exec-once`
- `~/.config/omarchy/plugins/io.github.laytonf.relaunch/` — installed QML copy

Hyprland must contain `source = ~/.config/omarchy-relaunch/relaunch.conf`.
`install.sh` appends that line; do not invent a different snippet path.

## Engine CLI

```
relaunch save [--json]     # hyprctl clients → merge into config → write snippet
relaunch generate [--json] # rewrite snippet from config
relaunch list [--json]     # print the table
relaunch reload            # hyprctl reload
```

`--json` is the widget contract. Keep the `result` shape in `engine/main.go`
stable (`ok`, `error`, `added`, `updated`, `staggerSeconds`, `entries`,
`snippetPath`, `configPath`). `Panel.qml` parses that object.

## Invariants

- One entry per window class. Capture keeps the first seen (lowest workspace).
  A `class:^(brave-browser)$` rule sends every Brave window to that workspace.
- Match `initialClass`, not `class`. Brave/Electron mutate class after launch.
- Launch commands come from the curated `knownExec` table in
  `engine/internal/config/config.go`. Do not parse `/proc/<pid>/cmdline`.
  Unknown apps fall back to a lowercased class. When adding a well-known app,
  extend `knownExec`.
- Recapture refreshes workspace only. Preserve user `exec`, `enabled`, and
  `float` edits.
- Skip special/negative workspaces (`Workspace.ID < 1`) and empty classes.
- Regex-escape class names in generated `windowrulev2` lines.
- Persist with temp-file + rename (`config.json.tmp`, `relaunch.conf.tmp`).
- Everything runs as the user. No sudo, no IPC beyond `hyprctl` and the
  engine binary on `PATH`.

## QML conventions

Mirror `omarchy.clock` / `omarchy.weather`:

- Root type is `BarWidget` with `moduleName` matching `manifest.json` `id`.
- Expose `opened`, `open()`, `close()`, `toggle()`, `closeForPopoutSwitch()`,
  and `injectPanel()` so the bar can route popouts.
- Theme via `root.barForeground` / `root.barBackground`, `Style.space()`,
  `Style.font.*`. No hardcoded palette.
- Use `Quickshell.Io.Process` to run `relaunch`. The binary must be on `PATH`
  (`~/.local/bin` after `install.sh`).
- After QML edits in a live install, `omarchy-shell shell rescanPlugins`
  (user plugin dir hot-reloads on save; this tree does not).

Do not edit `/usr/share/omarchy/`. Read it for the plugin contract.

## Build, test, install

```bash
# Engine
( cd engine && go build -o ~/.local/bin/relaunch . )
( cd engine && go test ./... )

# Plugin schema (must stay green)
omarchy plugin validate .

# QML (qt6 qmllint; include the shell import path)
/usr/lib/qt6/bin/qmllint -I "$OMARCHY_PATH/shell" BarWidget.qml Panel.qml

# Local install: binary + plugin copy + hyprland source line
./install.sh
omarchy bar move io.github.laytonf.relaunch --section right   # first time
```

Prefer `./install.sh` over copying files by hand. Once published:

```bash
omarchy plugin add https://github.com/laytonf/omarchy-relaunch.git --enable
```

The engine still needs a separate build onto `PATH`.

## Repo / publish

Not initialized as a git repo yet. One-time:

```bash
./git-init.sh laytonf
# then: gh repo create laytonf/omarchy-relaunch --public --source=. --remote=origin --push
```

Marketplace submit:
https://github.com/HANCORE-linux/omarchy-plugin-marketplace/issues/new?template=submit-plugin.yml

`.gitignore` already excludes the engine binary and generated
`relaunch.conf` / `config.json`. Keep it that way.

## Out of scope unless asked

- Live window tracking / a background daemon
- Per-window (not per-class) restore
- Restoring terminal session contents (tmux/herdr's job)
- Editing Hyprland or Omarchy packaged files
