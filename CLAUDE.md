# Relaunch — working notes for Claude

**Read `AGENTS.md` first and treat it as authoritative.** It holds the
architecture, the engine CLI contract, the invariants, the QML conventions, and
the build/test/install commands. Do not restate or fork any of that here — this
file only carries what AGENTS.md deliberately leaves out.

## Where the project stands

Feature work is essentially done. What is left is verification on real
hardware, then marketplace submission.

- Branch `salvage/desktop-index-and-cheap-list` is ahead of `main` and holds
  the current work. `main` is stale; do not treat it as the baseline.
- Not yet done: a real reboot on current HEAD, checked against
  `~/.config/omarchy-relaunch/last-boot.log` for `outcome: launched` with no
  `MISPLACED` rows. **Nobody in a terminal session can do this** — it kills
  every session on the machine. Ask the user; never run `reboot` yourself.
- `docs/laptop-test-plan.md` is the real-hardware plan for *this* laptop
  (Framework 13 AMD 7040, `omarchy13`). Phase 5 and the Phase 6 walkthrough
  need a re-confirm on current HEAD.
- Marketplace submission needs the repo public, a category, 1–3 tags, and the
  checklist at the `HANCORE-linux/omarchy-plugin-marketplace` issue template.

## How to verify work here

The sandboxed suite passes while the plugin is broken on the actual desktop.
Both real bugs found so far slipped past a green `./tests/relaunch-test.sh`:

- `.desktop` files were indexed with `find -type f`, which skips symlinks.
  Every LibreOffice entry on this system is a symlink, so Calc silently fell
  through to a fragile `cmdline` value.
- Hosted TUI argv was flattened with `"${inner[*]}"`, so `bash -c 'dua i /'`
  re-split at boot into `dua` with no arguments.

So: **run the suite, then prove it against a live window.** `hyprctl clients -j`
lists what is actually running with real pids. The engine's internals can be
called directly by sourcing it with the trailing `main "$@"` stripped:

```bash
head -n -1 ./relaunch > /tmp/rl-lib.sh && source /tmp/rl-lib.sh
identity_from_pid <pid>          # what capture would record
resolve_launch <class> <pid>     # exec<TAB>source
```

To run a real `save` without touching the user's live config, sandbox only the
config dir — real `hyprctl` and real `/proc` still apply:

```bash
RELAUNCH_CONFIG_DIR=/tmp/probe ./relaunch save --json
```

`RELAUNCH_CMDLINE_DIR` (fixture argv, with `<pid>.ppid` files) and
`RELAUNCH_DATA_DIRS` (fixture `.desktop` dirs) are what the suite uses.

For anything that ends up in a launch command, check the round trip, because
boot executes via `bash -c "$exec"`:

```bash
eval "set -- ${saved_exec#xdg-terminal-exec }"; printf '[%s]\n' "$@"
```

## House rules

- This laptop is the test fixture and the daily driver. Do not close, move, or
  relaunch the user's windows to test something. Read `hyprctl clients -j`,
  sandbox with the env vars above, and clean up any probe files and temporary
  harnesses you create.
- `~/.config/omarchy-relaunch/` is live user state. A stray `save` rewrites it.
  If you do modify it, say so plainly rather than leaving it changed silently.
- Do not edit `/usr/share/omarchy/`. Read it — it is the source of truth for
  the plugin contract and for the window rules the engine has to coexist with.
- Prefer `./install.sh` over hand-copying files into the plugin dir.
