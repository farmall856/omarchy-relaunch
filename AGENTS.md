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

- `~/.config/omarchy-relaunch/config.json` — entries, ignored startup ids, `skipOnce`, `windows`
- `~/.config/omarchy-relaunch/overrides.json` — user `class → exec` exceptions (starts empty)
- `~/.config/omarchy-relaunch/relaunch.lua` — generated `o.window` pins
- `~/.config/omarchy-relaunch/disabled` / `skip-once` — boot flags (skip is also in config.json so Save cannot drop it)
- `~/.config/omarchy-relaunch/last-boot.log` / `last-boot.json` — last `relaunch boot` diagnostic
- `~/.config/omarchy-relaunch/last-session.json` — pre-shutdown window
  snapshot, written only when the opt-in hook is enabled
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
relaunch snapshot                      # layout now, for shutdown/boot diffs
relaunch last-session [--json] [--diff]
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
  `xdg-terminal-exec --app-id=<cmd> -e <cmd...>` so Hyprland gets a distinct
  class. Do not special-case individual wrappers like herdr.
- Hosted argv keeps its argument boundaries. `/proc` reports Omarchy's Disk
  Usage as `foot --app-id=TUI.float -e bash -c 'dua i /'`, where `dua i /` is
  one argument. Quote each element (`printf %q`) instead of joining with
  `"${argv[*]}"`; boot runs the string through `bash -c`, so a flattened join
  re-splits and runs `dua` bare — it prints and exits before the pin applies.
  Keep the `-e`: `xdg-terminal-exec` accepts it as the explicit end of options.
- Never carry an exec string through jq's `@tsv`. It escapes backslashes, and
  a shell-quoted hosted command legitimately contains them; doubling them
  breaks the launch. Use `join("\u001f")` with `IFS=$'\037'`.
- Launch commands: `terminal` → `overrides.json` → `.desktop` (via
  `gio launch <file>`, user `~/.local/share/applications` first, then
  `$XDG_DATA_DIRS`) → cmdline → lowercased class (`guess`).
  `overrides.json` starts empty and only grows when the user saves a
  command. `--json` includes `execSource`, `execOk`, `unverified`, and
  `warnings`.
- `config.json` carries a `windows` snapshot: every window seen at the last
  save, with class, workspace, floating, and monitor id/name/description.
  Rewritten by `save`, carried through untouched by every other command.
  Nothing reads it yet — it exists so a future per-window feature has real
  data. It is deliberately unfiltered (no first-per-class dedupe, special and
  negative workspaces kept) because a snapshot loses information by applying
  capture's filters. Store both monitor name and description: output names
  (`DP-1`) are reassigned across reboots, descriptions (`BOE 0x0BCA`) identify
  the panel. `hyprctl monitors` failing degrades to empty names, never a save
  failure. It must never feed `entries[]` or the generated pins.
- Entries carry a `label`: a human-readable display name, resolved at
  `save`/`import` time and stored, never resolved at `list` time. Recomputed
  on recapture so it self-heals; it is not a user field, so a hand-edited
  label is replaced. Tiers: `desktop-file` reads `Name=` from the exact file
  the exec already names; `terminal` takes the leaf command and looks for a
  `.desktop` whose Exec is a terminal wrap running that leaf (Omarchy's
  `Disk Usage.desktop` maps `dua` → "Disk Usage"), else uses the leaf;
  anything else uses the leaf of the exec; the fallback is `class`.
  **`class` remains the identity and the only lookup key.** The label must
  never key anything and never reaches the generated pins. That is the only
  reason this reverse `.desktop` lookup is acceptable here after being
  rejected for resolving launch commands: a wrong label is cosmetic, a wrong
  exec is not.
- Unquote a stored exec before reading a leaf from it. It is `printf %q`
  output, so `bash -c dua\ i\ /` holds `dua i /` as one argument, and naive
  tokenizing yields the leaf `dua\`, which matches nothing. `unquote_argv`
  does this without `eval`, which would run command substitution in a string
  the user can edit in `overrides.json`.
- **Omarchy web apps are the one sanctioned reverse `.desktop` lookup for a
  launch command.** Chromium encodes an `--app=<url>` window class as
  `brave-<host>_<path with / as _>-Default`, which no `StartupWMClass`,
  desktop-file id or `Name` can match, and `/proc` cannot help either:
  Chromium merges the request into its existing process, so a web-app window
  reports the *main browser* argv with no `--app=`. Resolution therefore
  derives the identity **forward** from `Exec=omarchy-launch-webapp <URL>`
  and compares encoded strings exactly. The class is never decoded back into
  a URL — `/` → `_` is lossy, so `/foo/bar` and `/foo_bar` collide. Query and
  fragment are excluded because Chromium discards them; an empty path and `/`
  are already identical at the source. This is a deterministic exception to
  the rule above, not a softening of it: it reconnects an exact app-mode
  identity to the exact user-maintained launcher that produced it, rather
  than inferring an executable from a cosmetic `Name`. The tier sits after
  the exact `StartupWMClass` and desktop-id matches and before `Name`.
- The web-app index applies XDG desktop-ID masking: the highest-priority file
  bearing an id claims it, **including** a `Hidden=true` or non-web-app
  override, so a user copy genuinely replaces the system entry. After
  masking, two surviving distinct ids yielding one identity mark that
  identity ambiguous and unresolvable. This masking is scoped to the web-app
  map only — the class/id/`Name` maps keep their first-wins-per-key
  behaviour, and general XDG masking is a separate change.
- Only `brave-…-Default` is supported, the form verified on hardware. Custom
  Chromium profiles and other browsers stay unresolved and keep the existing
  cmdline fallback and its `unverified` flag. Do not broaden the matcher by
  guessing at suffix formats.
- Entries carry `startupKeys`: a lowercased list of tokens by which an
  autostart line can be recognised as this entry — the class, the
  desktop-file id, and the `Exec` leaf of the `.desktop` the exec already
  names. Resolved at `save`/`import` time and stored, never at `list` time,
  because `list` must not index applications dirs. It exists so
  `o.launch_on_start("brave")` correlates with a saved `brave-browser` entry
  and the row keeps `kind: both` and its "Delete startup config" action.
  **It is a correlation list only.** It is never an identity, never a lookup
  key, and never reaches the generated pins — `class` remains the only
  identity. A missing field normalizes to `[]`, so a config written before
  the field behaves exactly as it did before; the next `save` fills it in.
  It gets the same override protection as `label`: recapture leaves it alone
  on an `overrides-table` entry, whose exec is the user's own text.
- A synthesized `--app-id=<class>` goes through `quote_argv` like the inner
  argv. Boot runs the whole string through `bash -c`, so an app id containing
  a space would split and one containing a metacharacter would execute.
- Web-app identity follows `GURL::host()`/`GURL::path()`: userinfo and port
  excluded, dot segments removed by a literal transcription of RFC 3986
  5.2.4. A segment-stack approximation is **not** equivalent — it drops empty
  segments and the trailing slash a final `.` or `..` produces, so `/a/b/..`
  would give `/a` instead of `/a/` and `/a//b` would give `/a/b`. Those are
  different Chromium identities. A URL with no scheme is not a GURL and is
  rejected.
- Web-app masking keys on the case-sensitive XDG desktop-file id (path
  relative to `applications/`, `/` → `-`), not a lowercased basename:
  `Foo.desktop` and `foo.desktop` are distinct ids, and `foo/bar.desktop`
  claims `foo-bar`.
- Recapture never relabels an `overrides-table` entry. Its exec is the user's
  text and is protected, so a label describing the live hosted command would
  describe something it does not run.
- `list` (panel display) must not index `.desktop` files or resolve
  launchers. It reads saved rows, cheap terminal identity, and startup
  lines. Resolve only on `save` and `import`.
- Recapture refreshes workspace and re-heals the exec. A `guess`/`cmdline`
  fallback yields to any resolved value, and a `terminal` or `desktop-file`
  exec is rewritten whenever live resolution disagrees with it — otherwise a
  broken value written by an older engine is sticky forever and reinstalling
  never repairs it. `overrides-table` is the only source the user typed, so it
  is never healed away; `enabled` edits are always preserved.
- Float/tile follows the live window, like workspace. Capture stores `float`
  as a real boolean and recapture refreshes it — it is not a preserved user
  edit. Generated rules must say which they want: `float = true` when true,
  `tile = true` when false, **plus `tag = "-floating-window"` for a tiled
  entry**. `tile = true` alone is not enough: a direct per-class float rule is
  overridden by a later `tile = true`, but a tag-driven one is not, so a
  tagged class comes back floating at 875×600 with the tile rule present.
  Verified on hardware. Silence is not neutral either, because
  `default/hypr/apps/system.lua` tags `TUI.float` (and friends)
  `+floating-window`, so an unqualified pin lets a tiled window come back
  floating at 875×600. `tile` is a first-class rule; Omarchy uses it in
  `default/hypr/apps/browser.lua`. A genuinely absent `float` means
  "unknown" — a legacy entry, or one added by `import` with no window to
  read — and emits neither rule. `relaunch.lua` is loaded from
  `hyprland.lua` after the Omarchy defaults, so these rules win.
  Per-window geometry (center, size) is out of scope: it is not per-class.
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
- Theme via `root.barForeground`, `Style.space()`,
  `Style.font.*`. No hardcoded palette. There is no `root.barBackground`:
  `Ui/Panel.qml` exposes only `barForeground`. For a background, use the
  shell idiom `root.bar ? root.bar.background : Color.background` (see
  `Ui/PanelSlider.qml`). Assigning the non-existent property fails at
  runtime, not at lint time — `qmllint` cannot resolve `qs.Ui` types, so it
  says nothing. Watch the journal after `omarchy restart shell` instead.
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

Published at `github.com/farmall856/omarchy-relaunch` (public). Trunk is
`main`; feature work happens on a branch and lands through a pull request.

Marketplace submit:
https://github.com/HANCORE-linux/omarchy-plugin-marketplace/issues/new?template=submit-plugin.yml

That marketplace repo is someone else's — the account here has `pull` but not
`push`, so submission is fork-and-PR, never a direct push.

`.gitignore` already excludes generated `config.json` (and a leftover
`relaunch.conf` name from older installs). Keep it that way.

The `gh` token on this machine carries `gist, read:org, repo` — **no
`workflow` scope**. Pushing anything under `.github/workflows/` from the CLI
is rejected, so there is no CI and no bot review. Every gate here is manual.

## Working with other agents

Several agents run side by side in herdr panes on this machine. They are not
interchangeable, and the split is deliberate:

| Agent | Kind | Role |
|---|---|---|
| Claude | `claude` | **The only agent that writes code.** Implements, commits, opens PRs. |
| Sol | `opencode` (GPT 5.4) | Review only. Reads diffs, reports findings. Never commits. |
| Grok | `grok` | Review only. Same terms as Sol. |

Grok was previously given implementation work and it was withdrawn after
mistakes in the resolver. Do not hand a non-Claude agent an implementation
task without the user saying so explicitly.

**Every agent on this machine runs as the same Unix user and shares one `gh`
token.** GitHub therefore cannot tell them apart — every issue, comment and
push is attributed to `farmall856` regardless of which agent made it. Do not
rely on GitHub identity to know who did what; say so in the text. It also
means any agent *can* push to `main` or close any issue. Do not.

### The loop

Review has to survive a context compaction, so it lives on the pull request,
not in a terminal pane.

```bash
# Claude, after the work is committed
gh pr create --base main --fill

# Sol / Grok, reviewing
gh pr diff <n>                                  # the change
gh pr view <n>                                  # intent
gh pr review <n> --comment --body "..."         # findings go HERE

# Claude, folding review back in
gh pr view <n> --comments
```

Rules:

- **Do not merge your own pull request.** The user merges. A reviewer's
  approval is advice, not a gate that anyone but the user may clear.
- Reference issues with a closing keyword in the PR body (`Closes #1`), not
  only a bare `(#1)` in a commit subject — a bare reference links but never
  closes, which is how eleven fixed issues stayed open.
- Issues are the queue: `gh issue list --state open`. Findings that are not
  being fixed in the current branch become issues, not PR comments on an
  unrelated diff.

### Worktrees

A reviewer that runs `git checkout` or `git stash` in the shared checkout
destroys whatever the implementer has not committed yet. Read-only *intent*
is not read-only *behaviour*. Give each non-implementing agent its own tree:

```bash
git worktree add --detach ~/Projects/rl-review origin/main
```

Separate index, shared object store, and it can check out anything without
touching the working tree.

## Session snapshot (opt-in, diagnostic)

`relaunch snapshot` writes `last-session.json`: every window with class,
label, workspace, floating, monitor, pid and title. Nothing in `save`,
`generate` or `boot` reads it — it exists so `relaunch last-session --diff`
can show what did not come back, or came back wrong. The diff compares
workspace and float state, not only per-class counts: a window back on the
wrong workspace reports `MOVED`, not `ok`.

`uninstall` tears the unit down **first**, before deleting the script and
config dir — `disable --now` fires `ExecStop`, which runs `relaunch snapshot`
and needs both present. Installation fails through the normal error contract
rather than `|| true`; only disable and uninstall are best-effort. The unit
carries a short `TimeoutStopSec` so a hung `hyprctl` cannot stall logout.

The trigger is a user unit installed by `relaunch snapshot-hook --enable`
and removed by `--disable`. It is `PartOf=`/`After=graphical-session.target`,
and stop ordering is the reverse of start ordering, so its `ExecStop` runs
*before* the target goes down. `wayland-wm@.service` declares
`Before=graphical-session.target`, so the compositor stops *after* the target
— and therefore after this `ExecStop`, with `hyprctl` still answering.

Verified on this machine: the systemd user environment carries
`HYPRLAND_INSTANCE_SIGNATURE` and `WAYLAND_DISPLAY`, and an `ExecStop` of such
a unit read 9 windows through `hyprctl`. **Not** verified: a real
logout/reboot. If the compositor exits first — Hyprland's own exit dispatcher
rather than a systemd-initiated teardown — `wayland-wm@.service` fires
`OnSuccess=wayland-session-shutdown.target` with Hyprland already gone, and
the snapshot would be empty. One reboot with the hook enabled settles it. A
systemd user *timer* is the fallback if ordering turns out unreliable; a timer
plus a oneshot is not a daemon, and staleness is bounded by the interval.

## Known limitations

- **One workspace per class, and the pin is a standing rule.** The generated
  `o.window` line has no expiry: it applies to every window of that class for
  the whole session, not just the ones `relaunch boot` starts. Verified on
  `omarchy13` — with `brave-browser` pinned to workspace 2, launching Brave
  from workspace 7 opened it on 2; same for `foot`. So two windows of one app
  on two workspaces can be neither restored nor kept: capture records only the
  first seen, and the standing rule would collapse both anyway. Hyprland
  window rules match class, not a window instance, so there is nothing to
  attach a second placement to. This is the ceiling of the approach, not a
  bug — do not "fix" it with per-window rules.
- **Workspace numbers, not screens.** Restore targets workspace numbers.
  Which monitor a workspace lives on is Hyprland's business, and Omarchy
  ships no workspace→monitor binding at all — only manual keybindings
  (`default/hypr/bindings/tiling.lua:34-37`). With a user's own
  workspace→monitor rules, restore follows them; without, placement across
  screens is whatever Hyprland decides. Only single-monitor is verified.
- **Shared generic TUI classes.** Omarchy launches every floating TUI under
  `TUI.float` and every tiled one under `TUI.tile`. One entry per class means
  two different TUIs cannot hold different workspaces — the second capture
  overwrites the first. Do not "fix" this by relaunching under a synthesized
  app-id: `/usr/share/omarchy/default/hypr/apps/system.lua` matches `TUI.float`
  to apply float + center + 875×600, so a renamed class silently loses its
  floating treatment. Same for `terminals.lua` and `TUI.*`.
- **Non-default Chromium profiles.** A user running something other than
  `Default` gets a class this engine does not recognise, so their Omarchy web
  apps fall back to the generic browser cmdline and relaunch as a plain
  browser window. Known and deliberate: the suffix format for other profiles
  has not been verified, and guessing it risks launching the wrong app.
- A `.desktop` file cannot be found from a generic class either — `Disk
  Usage.desktop` has no `StartupWMClass` and its `Name` is "Disk Usage", so
  nothing keys back to `TUI.float`. The hosted-argv path above is what makes
  these apps relaunch correctly, not the `.desktop` index.

## Out of scope unless asked

- Live window tracking / a background daemon
- Per-window (not per-class) restore
- Restoring terminal session contents (tmux/herdr's job)
- Editing Hyprland or Omarchy packaged files
