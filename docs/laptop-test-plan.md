# Laptop test plan (omarchy13)

Plan for **this** Framework 13 (AMD Ryzen 7040), hostname `omarchy13`, Omarchy
Quattro 4.0.0-1, Hyprland 0.56.2. Do not treat a generic “open some apps and
reboot” pass as success.

Recorded 2026-08-17. Plugin was **not** installed when this plan was written:
no `relaunch` script, no `~/.config/omarchy-relaunch/`, no plugin folder, no
Hyprland source line.

## Fixture

Single display: `eDP-1` 2256×1504, `omarchy_monitor_scale = 1.3`. Live layout
at plan time:

| Workspace | Window | `class` / `initialClass` | Launch command that actually works |
|---|---|---|---|
| 1 | foot (`omarchy13: ~`) | `foot` | `foot` / `omarchy-launch-terminal` |
| 2 | Brave | `brave-browser` | `brave` |
| 3 | Proton Mail | `Proton Mail` | `proton-mail` |
| 4 | Signal | `signal` | `signal-desktop` |
| 5 | Proton Pass | `Proton Pass` | `proton-pass` |
| 6 | foot (this repo / Claude) | `foot` | same as ws 1 |

`~/.config/hypr/autostart.lua` already starts the same set
(`omarchy-launch-terminal`, `brave`, `proton-pass`, `proton-mail`,
`signal-desktop`, plus a second terminal for Claude). That interaction is
part of the test.

Hyprland config is **Lua only**. There is no `~/.config/hypr/hyprland.conf`.
Entry point is `hyprland.lua` → `require("hypr.autostart")`.

Bar right side is already packed: tray, `mdick.cloudflare`, Tailscale,
agents, bluetooth, network, audio, monitor, power. JetBrainsMono Nerd Font
is installed (widget glyph `\uf1da`).

`~/.local/bin` is on `PATH`.

## Phase 0 — Install and wiring

1. From the repo: `./install.sh`
2. Confirm:
   - `command -v relaunch` → `~/.local/bin/relaunch`
   - `~/.config/omarchy/plugins/io.github.laytonf.relaunch/{manifest.json,BarWidget.qml,Panel.qml,Overlay.qml}`
   - `omarchy plugin validate ~/.config/omarchy/plugins/io.github.laytonf.relaunch`
   - `omarchy plugin list --json` shows `io.github.laytonf.relaunch` enabled
   - Widget appears on the **right** of the bar; glyph renders
3. Pins are Lua only (`relaunch.lua` dofile'd from `hyprland.lua`).
   `install.sh` must **not** append a `source = …/relaunch.conf` line.
   Confirm no leftover `relaunch.conf` and `hyprctl configerrors` is empty.

Stop Phase 0 if the widget cannot open or `relaunch list --json` cannot run.

## Phase 1 — Engine on this session (no reboot)

Keep the six windows where they are.

```bash
relaunch list --json          # empty
relaunch save --json          # capture
relaunch list                 # human table
cat ~/.config/omarchy-relaunch/config.json
cat ~/.config/omarchy-relaunch/relaunch.lua
```

**Pass**

- One entry per class, first seen at lowest workspace. Current saved layout:
  `herdr` → 1, `brave-browser` → 2, `Proton Mail` → 3, `Proton Pass` → 4,
  `signal` → 5, `foot` → 6.
- `initialClass` used (here it matches `class` for every window — still
  assert the JSON field is `Proton Mail` / `Proton Pass` with the space).
- Atomic files: `config.json` and `relaunch.lua` exist; no leftover `.tmp`
  or `relaunch.conf`.
- Special/negative workspaces: none open now. Later, move a window to special
  and recapture — it must not get an entry.

`known_exec` already maps `signal-desktop`, `proton-mail`, and `proton-pass`.
Confirm those values in `config.json`; they should not need a hand edit.

Generated pins are `o.window({ class = "^(…)$" }, { workspace = "N silent" })`
in `relaunch.lua`. Space does not need escaping; a dotted class must be
Lua-escaped (`org\\.omarchy\\.agent`).

## Phase 2 — Recapture merge

1. Hand-edit `signal` → `exec: "signal-desktop"`, Proton Mail / Pass to
   `proton-mail` / `proton-pass`. Set `foot` `enabled: false` once as a probe.
2. Move Brave to workspace 6. Run `relaunch save` again.

**Pass:** Brave workspace updates to 6; custom `exec` values stay;
`enabled: false` on foot stays; no duplicate classes. Then put Brave back
to 2 and save again so the fixture is restored.

## Phase 3 — Widget / bar

- Left click toggles the panel. Escape closes. Clicking another panel
  (clock, weather, agents) should hand off via `closeForPopoutSwitch`.
- On open, table matches `relaunch list --json`. Disabled rows are dimmed.
- **Save Startup App Workspaces** → status like `Saved: N new, M updated.`
- **View last boot log** summons the overlay (`omarchy-shell shell summon
  io.github.laytonf.relaunch`).
- `omarchy-shell shell summon io.github.laytonf.relaunch '{}'` opens the
  log overlay; `hide` closes it. The bar icon still toggles the small panel.
- Disable the plugin and confirm the rest of the bar is unchanged. Re-enable.

If the icon is missing, it is a font/slot problem, not an engine problem.

## Phase 4 — Apply rules without a reboot

`autostart.lua` should only start `sunsetr` plus the hidden `relaunch boot`
hook. App launches belong to Relaunch. If extra `o.launch_on_start` app
lines are present, a reboot will double-start.

1. `relaunch generate` then `hyprctl reload`.
2. Open a **new** Brave window. It must land on workspace 2 (`silent`).
3. Open a new Signal. It must land on 5 (current pin).
4. Open a new foot. **Every** untagged foot is subject to `class:^(foot)$`
   → workspace 6 with the current save. A terminal hosted as
   `--app-id=herdr` is a different class.
5. Count processes after reload: `pgrep -a brave`, `proton-mail`,
   `proton-pass`, `signal-desktop`, `foot`. Reload must **not** spawn a
   second copy of each.

## Phase 5 — Reboot restore

This laptop boots Limine → Omarchy. LUKS unlock + fingerprint PAM: first
`sudo` after idle can time out. The plugin does not need sudo.

1. Snapshot: `hyprctl clients -j > /tmp/relaunch-before.json` and copy
   `config.json` / `relaunch.lua`.
2. Reboot from the power menu. Do **not** use s2idle as the planned
   “crash” — this Framework 13 AMD 7040 has a real data-fabric sync-flood
   hard reset on s2idle. That is hardware; do not manufacture it.
3. After login, wait for Hyprland + omarchy-shell, then:

```bash
hyprctl clients -j
hyprctl workspaces -j
```

**Pass:** herdr 1, Brave 2, Proton Mail 3, Proton Pass 4, Signal 5, foot 6.
`sunsetr` still ran from autostart. No `hyprctl configerrors`. Last-boot
log `outcome: launched` and no `MISPLACED` rows.

**Fail if:** Proton/Signal missing, two of each (leftover autostart lines),
everything on ws 1 (Lua pins failed to load), or a Hyprland config error
banner.

4. Optional second boot: cold power-off vs reboot. Same checks.

## Phase 6 — Edges that matter here

- **Two feet:** a plain `foot` and another `foot` share one class, so both
  follow the saved foot workspace. Distinct hosted commands
  (`xdg-terminal-exec --app-id=herdr -e herdr`) get their own class.
- **herdr:** `SUPER+SHIFT+H` launches it. Open herdr on ws 1, save.
  Capture should record `xdg-terminal-exec --app-id=herdr -e herdr` — with
  the `-e`, which `xdg-terminal-exec` takes as the explicit end of options.
  herdr sessions restore themselves; the plugin only places the window.
- **Stale saved exec:** an entry captured by an older engine keeps whatever
  string that engine wrote, and reinstalling does not rewrite it. Recapture
  must re-heal a `terminal` or `desktop-file` exec whose live resolution
  differs, while leaving a `set-exec` value (`overrides-table`) alone.
- **Float:** float a *new* class and save. Existing entries do **not**
  refresh `float`.
- **Stagger:** set `staggerSeconds: 1` in `config.json`. Boot waits 1s
  *between* launches, not before the first. `0` is the usual value.
- **Missing script:** the panel runs
  `~/.config/omarchy/plugins/io.github.laytonf.relaunch/relaunch`, not
  `~/.local/bin/relaunch`. Move the plugin copy aside; opening the panel
  must show an error within a moment, not hang the bar. Restore the script.
  Hiding only `~/.local/bin/relaunch` must *not* break the panel.
- **Hand-edit + `relaunch generate`** without Save: pins follow the file,
  not live windows.
- **Disable plugin, leave hooks:** next boot still pins/launches
  (hooks are Hyprland, not QML). “Remove widget ≠ stop restore”.
- **Uninstall:** `relaunch uninstall --yes` (not only
  `omarchy plugin remove`). Confirm `autostart.lua` keeps `sunsetr` and
  loses the boot hook, `hyprland.lua` loses the dofile, and
  `~/.config/omarchy-relaunch` is gone. Other bar widgets stay.

## Phase 7 — Opportunistic crash (do not schedule)

If the AMD s2idle reset happens again, that *is* the crash the plugin
claims to survive: next boot should apply the same snippet as Phase 5.
Check `journalctl -k -b -1` for `data fabric sync flood` only to confirm
it was that reset, then reuse Phase 5 checks. Do not run
`sudo amd-s2idle test` as a Relaunch test.

Do not test on the OpenMandriva dual-boot.

## Order and stop rules

| Day | Do | Stop if |
|---|---|---|
| 1 | Phase 0–3 on the live session | Widget missing, or `relaunch list --json` fails |
| 1 | Phase 4 reload + new-window pins | Double-spawn or `configerrors` |
| 2 | Phase 5 one reboot on current HEAD | Apps missing, dumped on ws 1, or last-boot `MISPLACED` |
| 2 | Phase 6 missing-script + uninstall | Bar or autostart left dirty |

**Minimum green for “works on this laptop”:** widget save/list; overlay
log; reboot restores the six pinned apps to the saved workspaces; last-boot
has no `MISPLACED`; existing bar widgets unaffected.

**Already known red, treat as product work not test surprises:**

- two untagged `foot` windows cannot occupy different workspaces (one class)
- leftover app lines in `autostart.lua` will double-start (user config, not
  a Relaunch bug; this laptop’s autostart is now sunsetr + the boot hook)

**Phase 6 re-confirmed on current HEAD (2026-08-18)**, in a sandbox using
`RELAUNCH_CONFIG_DIR` plus fixture `/proc` and `.desktop` dirs: two feet
collapse to one entry at the lowest workspace; herdr keeps its own class and
its `-e`; a new floating class records `float` while an existing entry does
not gain it; `staggerSeconds: 1` waits between launches but not before the
first, and `0` never waits; `generate` follows a hand-edited `config.json`
rather than live windows; the `disabled` / `skip-once` guard is in the
generated Lua and `skip-once` is consumed after one boot; `uninstall --yes`
removes the hook, the `dofile`, and the config dir while leaving `sunsetr`
and unrelated `hyprland.lua` lines intact. The plugin copy of the engine runs
with `~/.local/bin` off `PATH`. Still needs a human at the keyboard: opening
the panel with the plugin copy of `relaunch` moved aside, which must show
"Command failed" rather than hang the bar.

**Fixed since this plan was first written (do not re-open as bugs):**

- `install.sh` / `ensure_hooks` no longer target `hyprland.conf` /
  `relaunch.conf`; pins are `relaunch.lua` only
- `known_exec` includes `signal-desktop`, `proton-mail`, `proton-pass`
- skip-next-boot persists in `config.json`; Skip / Enable chips alternate
- last-boot log is an overlay, not an in-panel editor
