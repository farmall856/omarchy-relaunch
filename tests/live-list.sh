#!/usr/bin/env bash
# LIVE `list --json` on this desktop, bash vs the Rust engine (argument 1), with
# every write target sandboxed as `tests/differential.sh --live` does. The two
# must agree except for D2 rows (a guess whose Rust command, split back into
# words, is bash's one unquoted lowercase class), DLM's delays (bash's
# `staggerSeconds`, Rust's `preLaunchMs`/`postLaunchMs`), and DLM running rows
# for a terminal window: bash rewrites them through its terminal list, Rust
# names no terminal and shows the class guess. So a terminal hosting a command
# beside another window of its class is one more row in bash than in Rust,
# which lists one running row per class: a bash-only row from its terminal
# list is allowed.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST="${1:?usage: live-list.sh <rust engine>}"
S="$(mktemp -d)"
trap 'rm -rf "$S"' EXIT
run() { # run <tag> <engine>
  local sb="$S/$1"; mkdir -p "$sb/cfg"
  env RELAUNCH_CONFIG_DIR="$sb/cfg" RELAUNCH_HYPRLAND_LUA="$sb/hyprland.lua" \
    RELAUNCH_HYPR_CONF="$sb/hyprland.conf" RELAUNCH_PLUGIN_DIR="$sb/plugin" PREFIX="$sb/prefix" \
    "$2" list --json | sed "s#$sb#<W>#g" >"$S/$1.json"
}
run bash "$ROOT/tests/oracle/relaunch"
run rust "$RUST"
python3 -I - "$S/rust.json" "$S/bash.json" <<'EOF'
import json, shlex, sys
r = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
assert isinstance(b.pop("staggerSeconds"), int)
assert all(isinstance(r.pop(k), int) for k in ("preLaunchMs", "postLaunchMs"))
assert set(r) == set(b), (set(r) ^ set(b))
other = {k for k in r if k != "rows" and r[k] != b[k]}
assert not other, other
rust_ids = {x["id"] for x in r["rows"]}
hosted = [y for y in b["rows"] if y["id"] not in rust_ids
          and y["kind"] == "running" and y.get("execSource") == "terminal"]
b["rows"] = [y for y in b["rows"] if y not in hosted]
assert len(r["rows"]) == len(b["rows"]), ([x["id"] for x in r["rows"]], [y["id"] for y in b["rows"]])
d2 = dlm = 0
for x, y in zip(r["rows"], b["rows"]):
    if x == y:
        continue
    if x["kind"] == "running" and y.get("execSource") == "terminal":
        same = {k for k in set(x) | set(y) if x.get(k) == y.get(k)}
        assert same >= {"id", "kind", "class", "label", "workspace", "enabled", "inRelaunch"}, (x, y)
        assert (x["execSource"], x["exec"]) == ("guess", x["class"].lower()), (x, y)
        dlm += 1
        continue
    keys = {k for k in set(x) | set(y) if x.get(k) != y.get(k)}
    assert keys == {"exec"}, (x, y)
    assert x["execSource"] == "guess" and y["exec"] == x["class"].lower(), (x, y)
    assert shlex.split(x["exec"]) == [y["exec"]], (x["exec"], y["exec"])
    d2 += 1
print(f"live list: {len(r['rows'])} rows, {len(r['startup'])} startup items; "
      f"identical except {d2} D2 guess rows, {dlm} DLM terminal rows and "
      f"{len(hosted)} bash-only hosted-terminal rows")
EOF
