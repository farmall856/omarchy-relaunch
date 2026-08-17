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
4. Until that is fixed, apply snippets by a **manual Lua bridge** for later
   phases, or restore will look broken when the snippet was never sourced.
   Do not also add `exec-once` copies into `autostart.lua` unless labeled as
   a workaround.

Stop Phase 0 if the widget cannot open or `relaunch list --json` cannot run.

## Phase 1 — Engine on this session (no reboot)

Keep the six windows where they are.

```bash
relaunch list --json          # empty
relaunch save --json          # capture
relaunch list                 # human table
cat ~/.config/omarchy-relaunch/config.json
cat ~/.config/omarchy-relaunch/relaunch.conf
```

**Pass**

- One entry per class, first seen at lowest workspace: `foot` → **1** (not 6),
  `brave-browser` → 2, `Proton Mail` → 3, `signal` → 4, `Proton Pass` → 5.
- `initialClass` used (here it matches `class` for every window — still
  assert the JSON field is `Proton Mail` / `Proton Pass` with the space).
- Atomic files: `config.json` and `relaunch.conf` exist; no leftover `.tmp`.
- Special/negative workspaces: none open now. Later, move a window to special
  and recapture — it must not get an entry.

**Known-fail table (this laptop)**

| Class | Guessed `exec` | Real command | Result if you reboot on defaults |
|---|---|---|---|
| `brave-browser` | `brave` | `brave` | OK |
| `foot` | `foot` | `foot` | OK, but both terminals collide |
| `signal` | `signal` | `signal-desktop` | **will not relaunch** |
| `Proton Mail` | `proton mail` | `proton-mail` | **will not relaunch** |
| `Proton Pass` | `proton pass` | `proton-pass` | **will not relaunch** |

Confirm those guessed values in `config.json` before fixing them. That is
the curated-table gap, not a capture bug.

`windowrulev2` is still accepted on this Hyprland. Check generated lines:

```
windowrulev2 = workspace 3 silent, class:^(Proton Mail)$
```

Space does not need escaping; a dotted class must.

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
- **Regenerate** after a config edit rewrites the snippet only.
- **Reload Hyprland** runs `hyprctl reload` and does not crash the shell.
- `omarchy-shell shell summon io.github.laytonf.relaunch '{}'` opens;
  `hide` closes.
- Disable the plugin and confirm the rest of the bar is unchanged. Re-enable.

If the icon is missing, it is a font/slot problem, not an engine problem.

## Phase 4 — Apply rules without a reboot

`autostart.lua` already launches these apps. The snippet will add
`exec-once` for the same classes **plus** `windowrulev2` pins.

1. With execs corrected, `relaunch generate` then **Reload Hyprland**.
2. Open a **new** Brave window. It must land on workspace 2 (`silent`).
3. Open a new Signal. It must land on 4.
4. Open a new foot. **Every** foot, including the Claude terminal on 6, is
   subject to `class:^(foot)$` → workspace 1. That is the documented
   one-class rule.
5. Count processes after reload: `pgrep -a brave`, `proton-mail`,
   `proton-pass`, `signal-desktop`, `foot`. Reload must **not** spawn a
   second copy of each.

Do **not** leave snippet `exec-once` *and* the current `autostart.lua` lines
enabled for a reboot test, or you will get two of each app. For Phase 5,
pick one launcher:

- **A (plugin owns restore):** comment the app lines in `autostart.lua`
  (keep `sunsetr`; decide separately about the Claude terminal line).
- **B (autostart owns launch, plugin owns pin):** delete `exec-once` from
  the snippet and keep only `windowrulev2`. That is a product gap today
  (rules and launches are coupled). Record it.

Recommended for a first real boot: **A**, with execs corrected, foot either
disabled or accepted as “all feet → ws 1”.

## Phase 5 — Reboot restore

This laptop boots Limine → Omarchy. LUKS unlock + fingerprint PAM: first
`sudo` after idle can time out. The plugin does not need sudo.

1. Snapshot: `hyprctl clients -j > /tmp/relaunch-before.json` and copy
   `config.json` / `relaunch.conf`.
2. Reboot from the power menu. Do **not** use s2idle as the planned
   “crash” — this Framework 13 AMD 7040 has a real data-fabric sync-flood
   hard reset on s2idle. That is hardware; do not manufacture it.
3. After login, wait for Hyprland + omarchy-shell, then:

```bash
hyprctl clients -j
hyprctl workspaces -j
```

**Pass (strategy A, execs fixed, foot disabled):** Brave 2, Proton Mail 3,
Signal 4, Proton Pass 5. No extras. `sunsetr` still ran from autostart. No
`hyprctl configerrors`.

**Fail if:** Proton/Signal missing (exec table), two of each (double start),
everything on ws 1 (rules not sourced — Phase 0 Lua gap), or the Claude
foot vanished because you disabled all feet.

4. Optional second boot: cold power-off vs reboot. Same checks.

## Phase 6 — Edges that matter here

- **Two feet:** enable `foot` again, reboot once, confirm both land on
  ws 1. If you want ws 1 + ws 6 terminals, this plugin cannot do it without
  a class split (`--app-id=org.omarchy.terminal` is already used for Claude;
  today both windows still report `foot`).
- **herdr:** `SUPER+SHIFT+H` launches it. Open herdr on ws 1, save.
  `knownExec` maps `herdr` → `herdr`. herdr sessions restore themselves;
  the plugin only places the window.
- **Float:** float Proton Pass, recapture a *new* class. Existing entries
  do **not** refresh `float`. A new class should emit a float rule.
- **Stagger:** set `staggerSeconds: 1`, regenerate, confirm
  `exec-once = sleep 1 && …`.
- **Missing script:** `mv ~/.local/bin/relaunch{,.bak}`, open the panel.
  Status must show an error, not hang the bar. Restore the script.
- **Hand-edit + Regenerate** without Save: snippet follows the file, not
  live windows.
- **Disable plugin, leave snippet sourced:** next boot still pins/launches
  (snippet is Hyprland, not QML). “Remove widget ≠ stop restore”.
- **Uninstall:** `omarchy plugin remove io.github.laytonf.relaunch`, remove
  the source/workaround, `rm -rf ~/.config/omarchy-relaunch`. Bar and
  autostart return to today’s behavior.

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
| 1 | Phase 0–3 on the live session | Widget missing, `relaunch` not on PATH, or `hyprland.conf` source silently does nothing and you have no Lua workaround |
| 1 | Phase 4 reload + new-window pins | Double-spawn or `configerrors` |
| 2 | Phase 5 one reboot with strategy A | Apps missing or dumped on ws 1 |
| 2 | Phase 6 two-foot + uninstall | Bar or autostart left dirty |

**Minimum green for “works on this laptop”:** widget save/list/reload; Brave
pins to 2 after a new window; reboot restores Brave + the three
Proton/Signal apps **after** exec overrides; foot collision understood;
snippet actually loaded despite Lua-only Hyprland; existing bar widgets
unaffected.

**Already known red, treat as product work not test surprises:**

- `install.sh` targets a `.conf` this machine does not use
- `signal` / `Proton Mail` / `Proton Pass` are missing from `knownExec`
- two `foot` windows cannot occupy 1 and 6
- `autostart.lua` will double-start unless you pick A or B
