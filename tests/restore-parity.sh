#!/usr/bin/env bash
# Spec §11: the new engine restores as completely as the bash oracle, judged by
# IDENTITY and PLACEMENT (window class, workspace, enabled), not by command
# bytes, which differ by design. Over the same windows, the oracle's Save and
# the engine's capture are compared, every difference is explained, and every
# pin's launcher is written (through install-signed) and read back by the
# anchor's own parser to exactly the signed command.
#
#   tests/restore-parity.sh ENGINE ANCHOR           fixture world (+ two plain
#                                                   foots and a hosted one)
#   tests/restore-parity.sh --live ENGINE ANCHOR    this desktop, every write
#                                                   target sandboxed
#
# Allowed differences, each printed:
#   - a window the oracle saved and the engine does not: only if the engine's
#     capture row says why (§9 untrusted, special workspace, no command);
#   - a terminal's window: the oracle names it by the command it hosts, the
#     engine by its real class with the whole command line; it counts as the
#     same window when an engine pin on that workspace runs the hosted command;
#   - a pin the engine has and the oracle does not: only a second window of a
#     class on another workspace (the oracle keeps one window per class), or
#     another command at a place the oracle has (the engine keys a pin by
#     class, workspace and command);
#   - enabled: the oracle enables nothing it captures new, the engine enables
#     every trusted capture (DEN), so only engine-enabled-and-trusted counts.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
live=false
[[ "${1:-}" == --live ]] && { live=true; shift; }
ENGINE="${1:?usage: restore-parity.sh [--live] ENGINE ANCHOR}"
ANCHOR="${2:?usage: restore-parity.sh [--live] ENGINE ANCHOR}"
ORACLE="$ROOT/tests/oracle/relaunch"
S="$(mktemp -d)"
trap 'rm -rf "$S"' EXIT
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }

env_for() { # env_for <sandbox>: the environment both engines run in
  local sb="$1"
  printf '%s\0' RELAUNCH_CONFIG_DIR="$sb/cfg" RELAUNCH_HYPRLAND_LUA="$sb/hypr/hyprland.lua" \
    RELAUNCH_HYPR_CONF="$sb/hypr/hyprland.conf" RELAUNCH_PLUGIN_DIR="$sb/plugin" PREFIX="$sb/prefix" \
    RELAUNCH_ANCHOR_BIN="$ANCHOR" RELAUNCH_VERIFY_SLEEP=0 RELAUNCH_VERIFY_DEADLINE=0 RELAUNCH_VERIFY_INTERVAL=0
  if ! $live; then
    printf '%s\0' HOME="$sb/home" HYPRCTL="$sb/hyprctl" RELAUNCH_DATA_DIRS="$sb/data1:$sb/data2" \
      RELAUNCH_CMDLINE_DIR="$sb/cmd" RELAUNCH_AUTOSTART="$sb/hypr/autostart.lua"
  fi
}

world() { # world <dir>: the differential world, two foots added, no config
  local w="$1"
  if $live; then
    mkdir -p "$w/cfg" "$w/hypr"
    return
  fi
  "$ROOT/tests/differential/world.sh" "$w"
  rm -f "$w/cfg/config.json"
  jq '. + [
    {"address":"0x21","pid":121,"class":"foot","initialClass":"foot","workspace":{"id":3},"floating":false,"xwayland":false},
    {"address":"0x22","pid":122,"class":"foot","initialClass":"foot","workspace":{"id":6},"floating":false,"xwayland":false},
    {"address":"0x23","pid":123,"class":"foot","initialClass":"foot","workspace":{"id":9},"floating":false,"xwayland":false}
  ]' "$w/clients.json" >"$w/clients.json.new" && mv "$w/clients.json.new" "$w/clients.json"
  # A bare terminal running htop, started from its shell (the §11 case for a
  # terminal's foreground command): the engine keeps it, the oracle has the
  # terminal alone.
  jq '. + [{"address":"0x24","pid":124,"class":"foot","initialClass":"foot","workspace":{"id":4},"floating":false,"xwayland":false}]' \
    "$w/clients.json" >"$w/clients.json.new" && mv "$w/clients.json.new" "$w/clients.json"
  printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=Foot' 'Exec=foot' \
    'Categories=System;TerminalEmulator;' >"$w/data2/applications/foot.desktop"
  printf '%s\0' foot >"$w/cmd/124"
  printf '%s\0' bash >"$w/cmd/125"; printf '124\n' >"$w/cmd/125.ppid"; printf '126\n' >"$w/cmd/125.tpgid"
  printf '%s\0' htop >"$w/cmd/126"; printf '125\n' >"$w/cmd/126.ppid"
  printf '%s\0' foot >"$w/cmd/121"
  printf '%s\0' /usr/bin/foot >"$w/cmd/122"
  printf '%s\0' foot -e htop >"$w/cmd/123"
  # A second foot on workspace 6 running its own command: a pin of its own.
  jq '. + [{"address":"0x25","pid":127,"class":"foot","initialClass":"foot","workspace":{"id":6},"floating":false,"xwayland":false}]' \
    "$w/clients.json" >"$w/clients.json.new" && mv "$w/clients.json.new" "$w/clients.json"
  printf '%s\0' foot -e cliamp >"$w/cmd/127"
  cat >"$w/hyprctl" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in clients) cat "$w/clients.json" ;; monitors) printf '[]\n' ;; *) exit 1 ;; esac
EOF
  chmod +x "$w/hyprctl"
}

run_in() { # run_in <sandbox> <engine> <args...>
  local sb="$1" eng="$2"; shift 2
  local -a e=(); mapfile -t -d '' e < <(env_for "$sb")
  env "${e[@]}" "$eng" "$@"
}

world "$S/old"
run_in "$S/old" "$ORACLE" save --json >/dev/null
world "$S/new"
run_in "$S/new" "$ENGINE" propose --json >"$S/propose.json"

# Install the proposal as if signed (install-signed checks the shape, the
# anchor checks the signature), so the launchers are written as they would be.
jq -c '{v: 2, signed: ({v: 2, uid: 1000, user: "u", gen: 1} + (.proposalJson | fromjson)),
        signature: ("0" * 128)}' "$S/propose.json" >"$S/doc.json"
run_in "$S/new" "$ENGINE" install-signed --json <"$S/doc.json" >/dev/null \
  || fail "install-signed refused the proposal: $(run_in "$S/new" "$ENGINE" install-signed --json <"$S/doc.json")"

# Launchability: each launcher hashes to its pin and reads back to its exec.
# (NUL-separated: @tsv would double the backslashes a canonical exec holds.)
while IFS= read -r -d '' path && IFS= read -r -d '' sha && IFS= read -r -d '' exec; do
  [[ "$(sha256sum "$path" | cut -d' ' -f1)" == "$sha" ]] || fail "launcher $path does not hash to its pin"
  [[ "$("$ANCHOR" canonical-exec "$path")" == "$exec" ]] || fail "launcher $path does not read back to $exec"
done < <(jq -j '.signed.entries[] | .launcher.path, "\u0000", .launcher.sha256, "\u0000", .exec, "\u0000"' "$S/doc.json")

python3 -I - "$S/old/cfg/config.json" "$S/propose.json" "$live" <<'EOF'
import json, sys
def argv(exec_):  # a canonical exec: `\ ` and `\\` escapes, space-separated
    out, cur, it = [], "", iter(exec_)
    for ch in it:
        if ch == "\\":
            cur += next(it, "")
        elif ch == " ":
            out.append(cur); cur = ""
        else:
            cur += ch
    return out + [cur]
old = json.load(open(sys.argv[1]))["entries"]
prop = json.load(open(sys.argv[2]))
new = json.loads(prop["proposalJson"])["entries"]
rows = prop["candidates"]
o = {(e["class"], int(e["workspace"])): e for e in old}
# Every engine pin, by place; several may share one, each its own command.
at = {}
for e in new:
    at.setdefault((e["windowClass"], e["workspace"]), []).append(e)
bad = []
hosted = {}
hosting = set()  # the engine pins that run a command the oracle names a window by
for key in sorted(set(o) - set(at)):
    runs = [e for e in new if e["workspace"] == key[1] and key[0] in argv(e["exec"])[1:]]
    if runs:
        hosting.add(runs[0]["class"])
        hosted[key] = runs[0]
        print(f"  same window: oracle {key[0]} ws{key[1]} is engine {runs[0]['windowClass']} running it ({runs[0]['exec']})")
        continue
    why = [r.get("reason") for r in rows
           if r["class"] == key[0] and r["workspace"] == key[1] and not r.get("proposed")]
    if any(why):
        print(f"  oracle only, explained: {key[0]} ws{key[1]} ({why[0]})")
    else:
        bad.append(f"oracle has {key} and the engine drops it unexplained")
for key, pins in sorted(at.items()):
    rest = [e for e in pins if e["class"] not in hosting]
    if key in o:
        if not any(e["enabled"] for e in pins) and o[key]["enabled"]:
            bad.append(f"{key}: the oracle launches it and the engine does not")
        rest = rest[1:]
        for e in rest:
            print(f"  engine only, another command at {key[0]} ws{key[1]}: {e['exec']}")
        continue
    if not rest:
        continue
    other = [k for k in o if k[0] == key[0]]
    if other:
        print(f"  engine only, a second {key[0]} window: ws{key[1]} (oracle keeps ws{other[0][1]})")
    else:
        bad.append(f"engine has {key} that the oracle never saw")
enabled_new = sum(e["enabled"] for e in new)
for b in bad:
    print("  MISMATCH:", b)
mode = "live" if sys.argv[3] == "true" else "fixture"
print(f"restore-parity ({mode}): oracle {len(o)} pins, engine {len(new)} pins ({enabled_new} enabled), "
      f"common {len(set(o) & set(at)) + len(hosted)}, every launcher reads back to its command")
sys.exit(1 if bad else 0)
EOF
