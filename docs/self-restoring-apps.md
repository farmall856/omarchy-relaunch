# Self-restoring applications

Analysis and recommendation for [#14](https://github.com/farmall856/omarchy-relaunch/issues/14).
**Nothing here is implemented.** Written for review.

## What happened

After the first reboot following the PR #12 merge, workspace 8 had two
identical Google Maps windows where one existed before:

```
ws=8  pid=1501  addr=0x56546c0e6ad0  title=Google Maps
ws=8  pid=1501  addr=0x56546c0c9b80  title=Google Maps
```

Same class, same Brave process, two windows. The second appeared roughly 30
seconds after boot, long after `relaunch boot` had finished.

Two restore mechanisms ran, neither aware of the other:

1. `relaunch boot` launched `gio launch …/Google Maps.desktop`.
2. Brave restored the window from its own session, because
   `exit_type = Crashed` — the reboot killed it rather than letting it quit,
   and crash recovery reopens the previous session.

`session.restore_on_startup` is unset, so this is Brave's default behaviour
on an unclean exit, not a setting the user chose.

## Why Relaunch cannot detect it at boot

This is the part that constrains every option below.

At the moment `boot` runs, the browser has not restored anything yet. There
is no second window to see, no marker on disk that says "I will restore two
windows", and no way to distinguish an app that will restore itself from one
that needs launching. The evidence arrives tens of seconds later, by which
time `boot` has exited.

Worse, the trigger is *conditional*: Brave only restores when the previous
exit was unclean. The same machine, the same config and a clean logout
produce no duplicate. So a static per-app rule ("Brave restores itself") is
wrong half the time, and any detection that runs at boot is looking before
the fact exists.

This generalises well beyond Brave — any browser, editor or session-managing
terminal will do the same. It is the mirror image of the documented
one-workspace-per-class limit: there Relaunch restores too few windows, here
it restores one too many.

## Option 2 is ruled out

**Post-boot reconciliation — comparing live windows against what `boot`
launched and closing the extras — is rejected. It should not be revisited
without a much stronger reason than this issue provides.**

The stated acceptance criterion is already "Relaunch never closes a window it
did not open", and reconciliation cannot honour that, because the information
needed to tell the two apart does not survive:

- Both windows belong to **one process**. pid cannot separate them.
- Both carry the **same class**, which is the only identity this project
  uses. Class cannot separate them.
- Window addresses are not stable across the launch, and `boot` has exited
  before the duplicate appears, so nothing recorded at launch time can be
  matched against a window that shows up 30 seconds later.

That leaves heuristics — "close the newer one", "close the one that is not on
the pinned workspace" — applied to a destructive, unrecoverable action. A
browser window can hold unsaved form input, a half-written message, a tab the
user just opened by hand. Closing the wrong one is not a cosmetic error, and
the failure would be silent and intermittent, appearing only after unclean
shutdowns.

The asymmetry decides it: the cost of *not* closing a duplicate is one extra
window the user closes themselves in two seconds. The cost of closing the
wrong window is unrecoverable data loss. A tool that restores windows must
never be the thing that destroys one.

There is also a scope argument. Closing windows would make Relaunch a window
manager rather than a restore tool, and would need an undo story, a
confirmation path, and a way to be sure the window it closes is not the one
the user is typing into. None of that is justified by "sometimes there are
two Google Maps windows".

## Recommendation: 3 then 1

### Step 1 — detect and report (issue option 3)

Extend the existing boot verification to notice when a class ends up with
more windows than entries, and say so. The plumbing is already there after
the fix for #15: `verify_launches` re-samples pending entries until a
deadline, so it is already looking at the window list some seconds after
launch.

Two honest limits, which should be stated in the log rather than engineered
around:

- The duplicate in this report appeared at ~30s, and the default deadline is
  20s. Catching it means either a longer deadline for this check or accepting
  that late duplicates are missed. **Do not extend the boot deadline for
  this** — boot must stay bounded. A better home is `last-session --diff`,
  which the user runs later, by hand, with no deadline at all.
- A duplicate can be entirely legitimate. The user may have opened a second
  window themselves. So this is a report, never an action.

Concretely: `last-session --diff` already compares saved windows against live
ones per class and knows both counts. It reports `MISSING` when fewer came
back. The same comparison yields `DUPLICATE` when more did, with no new
machinery and no new sampling.

That gives the user the one thing they currently lack: knowing *which* of
their apps restores itself, on their machine, with their settings.

### Step 2 — per-entry opt-out (issue option 1)

Once the user knows, let them act: a `selfRestoring: true` flag on an entry,
which makes `boot` skip launching it and leaves placement to the pin. The
window the app restores still gets placed, because the pin is a standing
class rule and applies to any window of that class however it was opened —
the same property that causes the one-workspace-per-class limitation is,
here, exactly what is wanted.

This is honest about what it is: a user decision, recorded per entry, not a
guess. It composes with step 1, which is what tells them to set it.

Two details worth deciding at implementation time, not now:

- **Where it is set.** A panel control on the row is discoverable; a
  `config.json` field alone is not. The rocket expander already holds
  per-entry launch settings and is the natural home.
- **What `boot` logs.** A skipped entry must appear in `last-boot` as
  deliberately skipped, not silently absent, or the log stops accounting for
  every entry.

### Why this order

Step 1 is diagnostic, cannot break a restore, and is nearly free given the
`--diff` machinery already exists. Step 2 changes launch behaviour and should
not ship before the user has a way to know which entries need it. Neither
step can close a window.

## Not recommended

**Detecting the unclean exit.** Brave records `exit_type` in its preferences
file, so Relaunch could in principle read it and skip launching when a crash
restore is expected. Rejected: it is per-browser, it reaches into another
application's private state file, the format is undocumented and free to
change, and it would have to be reimplemented for every self-restoring app.
The per-entry flag achieves the same outcome without Relaunch pretending to
understand Brave's internals.

**Launching and then checking.** Launching the app, waiting, and closing the
extra is option 2 with extra steps and the same destructive ending.
