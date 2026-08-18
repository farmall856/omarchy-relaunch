# Per-launch placement

Design doc. **Not implemented.** Written for review before any code changes.

Status: proposal. Every claim marked *verified* was run on this machine
(`omarchy13`, Hyprland 0.56.2) and the probes were cleaned up. Claims marked
*open* are not settled and are called out at the end.

## The problem

Placement today is per **class**, through persistent `o.window` pins in the
generated `relaunch.lua`:

```lua
o.window({ class = "^(brave-browser)$" }, { workspace = "2 silent" })
```

Two structural consequences follow from that mechanism, not from any decision:

**a. One workspace per class.** Brave on workspace 2 and Brave on workspace 5
collapse to a single entry; capture keeps the first seen at the lowest
workspace and drops the other. There is nowhere to put the second placement,
because a class-matching rule can only carry one workspace.

**b. The pin is a standing rule.** It has no expiry and applies to every
window of that class for the whole session, not only the ones `relaunch boot`
started. Verified: with `brave-browser` pinned to workspace 2, launching Brave
by hand from workspace 7 opened it on workspace 2. That is a side effect of
using window rules as the placement mechanism, and it is the behaviour that
surprises users.

Hyprland window rules match on class, not on a window instance. So no amount
of care with per-class pins can express "Brave on 2 *and* 5". A different
mechanism can.

## The mechanism

Placement can be attached to a **launch** instead of to a class, using a
dispatch-scoped rule prefix. The rule then applies to the window that launch
produces, and to nothing else.

### Invocation (verified)

The legacy prefix form is rejected by Hyprland 0.56, which parses `dispatch`
as Lua:

```console
$ hyprctl dispatch exec "[workspace 9 silent] foot"
error: [string "return hl.dispatch(exec [workspace 9 silent] ..."]:1: ']' expected near '9'
 → Note: dispatch in lua is a shorthand for hl.dispatch(...), your syntax might need to be updated.
```

`hyprctl dispatch` wraps its argument as `return hl.dispatch(<argument>)`, so
the argument has to be a Lua expression. This form works from bash, which is
what the engine needs:

```bash
hyprctl dispatch 'hl.dsp.exec_cmd("[workspace 9 silent] <command>")'
```

Verified: the window landed on workspace 9 while the active workspace stayed
at 2 — `silent` holds, focus does not move.

### Two windows, one class (verified)

The central claim of this design. Dispatching the same `--app-id` twice, to
two different workspaces:

```
class=RL-SAME ws=9  pid=1134459
class=RL-SAME ws=10 pid=1134473
```

Both windows share one class and hold different workspaces, which per-class
pins cannot express. Focus never moved.

### Rule precedence is not uniform (verified)

Worth knowing before relying on "a later rule wins": it depends on how the
earlier rule was applied. A direct per-class rule *is* overridden by a later
one — `o.window("RL-DIRECT", { float = true })` followed by our
`tile = true` produced a tiled window. A **tag-driven** rule is not:
`+floating-window` plus a later `tile = true` still produced a floating
875x600 window. Removing the tag first (`tag = "-floating-window"`) works.

This bit the float/tile feature in production and is now handled by the
generator. Any design that assumes load order determines the winner is wrong.

### Backslashes must be doubled for Lua (verified)

The stored `exec` is `printf %q` output, so a hosted TUI command contains
backslashes. Embedded naively into the Lua string, Hyprland rejects it:

```console
$ hyprctl dispatch "hl.dsp.exec_cmd(\"[workspace 9 silent] foot -e bash -c sleep\ 4\")"
error: ...:1: invalid escape sequence near '"[workspace 9 silent] foot -e bash -c sleep\ '
```

Doubling the backslashes first (`${exec//\\/\\\\}`) works. This is the third
appearance of this same bug class in this project, after the `@tsv` escaping
and the `"${inner[*]}"` flattening; the invariant list already warns about
both. **Any implementation must escape for Lua at the boundary and have a test
that a hosted TUI entry survives it.** Without it, `TUI.float` fails 100% of
the time while every other entry works — the worst possible failure shape,
because it looks like an app-specific bug.

### Argument boundaries survive (verified)

Dispatching `foot --app-id=RL-PROBE4 -e bash -c sleep\ 25` produced exactly:

```
[foot] [--app-id=RL-PROBE4] [-e] [bash] [-c] [sleep 25]
```

`sleep 25` stayed one argument through `printf %q` → Lua escaping → dispatch →
Hyprland's shell. The boundary invariant survives the new mechanism.

### `gio launch` and D-Bus activation

Six of the current entries launch via `gio launch <desktop-file>`, and the
earlier concern was that `gio` might hand off over D-Bus, leaving Hyprland no
child process to attribute the window to. It does not apply here: none of the
six `.desktop` files set `DBusActivatable=true`, so `gio` forks directly and
Hyprland attributes the window normally.

That is a property of *these* files, not a guarantee. A `DBusActivatable=true`
app is launched by the session bus, its window is not a descendant of the
dispatch, and the scoped rule will not match it. Such an entry has to degrade
to something else — see "What happens to pins".

## Decisions

### 1. Config shape

Entries can no longer be keyed by class, because there can now be several per
class. Proposal — split what is per-app from what is per-placement:

```json
{
  "staggerSeconds": 0,
  "apps": {
    "brave-browser": {
      "exec": "gio launch /usr/share/applications/brave-browser.desktop",
      "execSource": "desktop-file",
      "label": "Brave"
    }
  },
  "entries": [
    { "id": "brave-browser#1", "class": "brave-browser", "workspace": 2, "float": false, "enabled": true },
    { "id": "brave-browser#2", "class": "brave-browser", "workspace": 5, "float": false, "enabled": true }
  ]
}
```

*Why split.* `exec`, `execSource` and `label` describe the application; two
Brave windows do not launch differently. Duplicating the exec across N entries
means `set-exec` has to update N rows, `overrides.json` (already class-keyed)
stops lining up with the entry list, and the exec-healing logic has to reconcile
N copies that can disagree. Splitting keeps exactly one truth per app and makes
each entry a small placement record.

*The cost.* It is a real schema change, and the panel and `--json` contract
both read per-entry `exec` today. Both need a compatibility pass.

*The alternative* — keep everything on the entry and duplicate `exec` — has a
one-line migration and no reader changes, at the price of the duplication
problems above. Given that this project has now been bitten twice by state
that could disagree with itself (stale execs, stale labels), one truth per app
is worth the migration.

*Entry id.* `<class>#<n>`, with `n` allocated per class and never reused
inside one config. It is stable across recapture, readable enough to type at
the CLI (`relaunch drop --id 'brave-browser#2'`), and does not encode the
workspace — so an entry that moves keeps its identity.

*Migration.* An old config is exactly the N=1 case. Each existing entry
becomes one `apps` record plus one `entries` row with id `<class>#1`. Lossless
and mechanical. `load_config` should do it on read so every command tolerates
an old file, with the new shape written on the next save.

### 2. Capture must stop collapsing

Capture keeps first-seen-at-lowest-workspace today. It has to record every
window instead — the `windows` snapshot added in `222c515` already proves the
data is there.

The new difficulty is **matching saved entries to live windows on recapture**,
so that workspace/float refresh instead of duplicating. With one entry per
class, class was the key. With N, it is an assignment problem: entries saved
on 2 and 5, windows now on 3 and 5 — which one moved?

Proposal, per class: sort saved entries by workspace and live windows by
workspace, then zip them in order. Extra windows become new entries, extra
entries are dropped only if the user asked for that. It is not clever, it is
predictable, and it keeps ids attached to the entry that most plausibly
continues.

Predictability matters more than accuracy here: a wrong guess moves a window
one workspace, and the user's next save corrects it.

### 3. What happens to pins

**Recommendation: dispatch becomes primary; pins are still emitted, but only
for entries that cannot be dispatched.**

The case for dropping pins entirely is consequence (b): the standing rule that
grabs windows the user launched by hand is the actual complaint, and it is
gone the moment placement is per-launch.

The case for keeping them is narrower but real, and it is the crash-restore
case this plugin exists for. `relaunch boot` runs after a crash where the app
**is already running** — Hyprland restarted, the app did not. Dispatch does
nothing for a window that already exists; only a rule can place it. Removing
pins removes the only mechanism that covers that.

So: emit no pin for an entry that boot will dispatch, and keep pins for

- entries whose app is already running when boot fires,
- `DBusActivatable=true` apps, whose window cannot be attributed to the
  dispatch,
- an explicit per-entry `"placement": "pin"` escape hatch.

This keeps the surprising standing-rule behaviour off by default while leaving
the fallback available where dispatch cannot work. The cost is two mechanisms
to reason about, which is why boot must say in the last-boot log which one it
used per entry.

### 4. Stagger

Stagger currently waits between launches. Launching N instances of one class
makes it load-bearing rather than optional: two `gio launch` calls for the same
`.desktop` in quick succession are the exact shape that makes an app decide the
second invocation should focus the first window instead of opening a new one.

Proposal: keep `staggerSeconds` as the global default, and always wait a
minimum interval between launches *of the same class*, even when
`staggerSeconds` is 0. The current default of 0 is right for N distinct apps
and wrong for N copies of one app.

### 5. Apps that focus instead of opening

Single-instance apps — most Electron apps, some GTK apps — respond to a second
launch by focusing the existing window. N entries then yield 1 window, and the
restore silently under-delivers.

This must be detected and reported, not hidden. Boot already verifies where
each launch landed (that is what produces `MISPLACED`). Extend it: after
launching N entries of a class, count the windows of that class. Fewer windows
than entries is a distinct outcome — `SINGLETON` — reported per class in
`last-boot.log`, saying the app refused a second instance rather than pretending
one is missing.

That is a report, not a fix. The fix, if one exists, is per-app and out of
scope.

## Open questions

1. **Pin vs dispatch precedence — dispatch wins (verified).** Settled while
   fixing the float regression. With the real generated pin
   `o.window({ class = "^(TUI\\.float)$" }, { workspace = "3 silent", ... })`
   loaded, a window dispatched with `[workspace 9 silent]` landed on **9**, not
   3. The dispatch-scoped rule beats the standing pin for workspace, which is
   what decision 3 needs: the two mechanisms can coexist without the pin
   dragging a dispatched window back.
   Still unverified: precedence for *float/tile* between the two, and what
   happens when a pin exists for a class with several dispatched entries.
2. **Does `silent` hold under load?** Verified with a handful of probes. Boot
   dispatches every entry at once on a cold start; whether focus stays put
   under that is not tested.
3. **Ordering.** Pins apply whenever the window appears. Dispatch applies at
   launch. An app slow to map its window (LibreOffice already produces a false
   `MISPLACED` for this reason) may need the verification window widened.
4. **What replaces `class` as the panel's grouping key**, given a class can now
   span workspaces. The workspace boxes added in `c6d62bc` already group by
   workspace, so this may be free.

## Not in scope

Per-window geometry (size, position, centering). It is per-window rather than
per-class, and the same argument that made it out of scope for float/tile
applies unchanged.
