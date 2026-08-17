#!/usr/bin/env bash
# Fixture tests for the bash+jq relaunch engine.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELAUNCH="$ROOT/relaunch"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_contains() {
  local haystack="$1" needle="$2"
  [[ "$haystack" == *"$needle"* ]] || fail "missing ${needle@Q} in"$'\n'"$haystack"
}

assert_not_contains() {
  local haystack="$1" needle="$2"
  [[ "$haystack" != *"$needle"* ]] || fail "unexpected ${needle@Q} in"$'\n'"$haystack"
}

HYPRCTL_STUB="$WORKDIR/hyprctl"
cat >"$HYPRCTL_STUB" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  clients) cat "${FAKE_CLIENTS:?}" ;;
  reload)  printf 'ok\n' ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$HYPRCTL_STUB"

export RELAUNCH_CONFIG_DIR="$WORKDIR/cfg"
export RELAUNCH_AUTOSTART="$WORKDIR/autostart.lua"
export RELAUNCH_HYPRLAND_LUA="$WORKDIR/hyprland.lua"
export HYPRCTL="$HYPRCTL_STUB"
mkdir -p "$RELAUNCH_CONFIG_DIR"
: >"$RELAUNCH_AUTOSTART"
: >"$RELAUNCH_HYPRLAND_LUA"

# --- generate / snippet (rules only; launches are relaunch boot) ---
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{
  "staggerSeconds": 1,
  "entries": [
    {"class": "brave-browser", "workspace": 2, "exec": "brave", "enabled": true},
    {"class": "herdr", "workspace": 1, "exec": "herdr", "enabled": true},
    {"class": "org.wezfurlong.wezterm", "workspace": 6, "exec": "wezterm", "enabled": true, "float": true},
    {"class": "disabled", "workspace": 3, "exec": "nope", "enabled": false},
    {"class": "", "workspace": 4, "exec": "empty", "enabled": true},
    {"class": "zero", "workspace": 0, "exec": "zero", "enabled": true}
  ]
}
EOF

"$RELAUNCH" generate >/dev/null
grep -q 'omarchy-relaunch' "$RELAUNCH_AUTOSTART" || fail "ensure_hooks did not mark autostart"
grep -Fq "${RELAUNCH} boot" "$RELAUNCH_AUTOSTART" || fail "boot hook must use the script path, not PATH"
snippet="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.conf")"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"

assert_contains "$snippet" 'windowrulev2 = workspace 1 silent, class:^(herdr)$'
assert_contains "$snippet" 'windowrulev2 = workspace 2 silent, class:^(brave-browser)$'
assert_contains "$snippet" 'windowrulev2 = workspace 6 silent, class:^(org\.wezfurlong\.wezterm)$'
assert_contains "$snippet" 'windowrulev2 = float, class:^(org\.wezfurlong\.wezterm)$'
assert_not_contains "$snippet" 'exec-once'
assert_not_contains "$snippet" 'disabled'
assert_contains "$lua" 'o.window({ class = "^(herdr)$" }, { workspace = "1 silent" })'
assert_contains "$lua" 'workspace = "6 silent", float = true'

# unknown class with empty exec still gets a rule
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"entries":[{"class":"SomeApp","workspace":3,"exec":"","enabled":true}]}
EOF
"$RELAUNCH" generate >/dev/null
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.conf")" 'class:^(SomeApp)$'

# knownExec guesses still used by boot command list
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"entries":[
  {"class":"brave-browser","workspace":1,"exec":"","enabled":true},
  {"class":"Alacritty","workspace":2,"exec":"","enabled":true},
  {"class":"VSCodium","workspace":3,"exec":"","enabled":true},
  {"class":"herdr","workspace":4,"exec":"","enabled":true}
]}
EOF
"$RELAUNCH" generate >/dev/null
got="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.conf")"
assert_contains "$got" 'class:^(brave-browser)$'
assert_contains "$got" 'class:^(Alacritty)$'
assert_contains "$got" 'class:^(VSCodium)$'
assert_contains "$got" 'class:^(herdr)$'

# --- capture merge ---
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{
  "staggerSeconds": 0,
  "entries": [
    {"class": "brave-browser", "workspace": 2, "exec": "brave --user-data-dir=x", "enabled": true},
    {"class": "foot", "workspace": 1, "exec": "foot", "enabled": false}
  ]
}
EOF

export FAKE_CLIENTS="$WORKDIR/clients.json"
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class": "foot", "initialClass": "foot", "floating": false, "workspace": {"id": 6, "name": "6"}},
  {"class": "brave-browser", "initialClass": "brave-browser", "floating": false, "workspace": {"id": 3, "name": "3"}},
  {"class": "signal", "initialClass": "signal", "floating": false, "workspace": {"id": 4, "name": "4"}},
  {"class": "scratch", "initialClass": "scratch", "floating": false, "workspace": {"id": -98, "name": "special"}},
  {"class": "", "initialClass": "", "floating": false, "workspace": {"id": 2, "name": "2"}},
  {"class": "Brave", "initialClass": "brave-browser", "floating": false, "workspace": {"id": 8, "name": "8"}}
]
EOF

out="$("$RELAUNCH" save --json)"
echo "$out" | jq -e '.ok == true and .added == 1 and .updated == 2' >/dev/null \
  || fail "save json counts: $out"

cfg="$(cat "$RELAUNCH_CONFIG_DIR/config.json")"
echo "$cfg" | jq -e '
  (.entries | map(select(.class == "brave-browser")) | .[0]
    | .workspace == 3 and .exec == "brave --user-data-dir=x")
  and (.entries | map(select(.class == "foot")) | .[0]
    | .workspace == 6 and .enabled == false)
  and (.entries | map(select(.class == "signal")) | .[0]
    | .workspace == 4 and .exec == "signal-desktop" and .enabled == true)
  and (.entries | map(.class) | index("scratch") == null)
' >/dev/null || fail "merge result: $cfg"

# first-seen lowest workspace: two feet, keep ws 1
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class": "foot", "initialClass": "foot", "floating": false, "workspace": {"id": 6}},
  {"class": "foot", "initialClass": "foot", "floating": false, "workspace": {"id": 1}}
]
EOF
"$RELAUNCH" save >/dev/null
echo "$(cat "$RELAUNCH_CONFIG_DIR/config.json")" | jq -e '
  .entries | length == 1 and .[0].class == "foot" and .[0].workspace == 1
' >/dev/null || fail "first-seen lowest workspace"

# --- startup inventory / ignore / import / drop ---
cat >"$RELAUNCH_AUTOSTART" <<'EOF'
-- Extra autostart processes.
o.launch_on_start("sunsetr")
o.launch_on_start("brave")
o.launch_on_start("proton-mail")
o.exec_on_start("setsid uwsm-app -- xdg-terminal-exec --app-id=org.omarchy.terminal claude")
-- omarchy-relaunch (managed; hidden from the Relaunch list)
o.exec_on_start("relaunch boot")
EOF

listed="$("$RELAUNCH" list --json)"
echo "$listed" | jq -e '
  ([.startup[].exec] | index("relaunch boot") == null)
  and ([.startup[].exec] | index("brave") != null)
  and ([.startup[] | select(.exec == "brave") | .class] | .[0] == "brave-browser")
  and ([.startup[] | select(.exec | startswith("setsid")) | .class] | .[0] == "foot")
  and ([.rows[] | select(.kind == "startup" and .exec == "brave")] | length == 1)
' >/dev/null || fail "inventory: $listed"

"$RELAUNCH" ignore --id 'lua:launch_on_start:sunsetr' --json >/dev/null
"$RELAUNCH" list --json | jq -e '
  (.ignored | index("lua:launch_on_start:sunsetr") != null)
  and ([.rows[] | select(.kind == "ignored" and .exec == "sunsetr")] | length == 1)
' >/dev/null || fail "ignore sunsetr"

"$RELAUNCH" import --exec brave --workspace 2 --json >/dev/null
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.kind == "both" and .class == "brave-browser" and .workspace == 2)] | length == 1)
' >/dev/null || fail "import brave"

"$RELAUNCH" drop-startup --id 'lua:launch_on_start:brave'
grep -q 'o.launch_on_start("brave")' "$RELAUNCH_AUTOSTART" && fail "startup brave still present"
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.class == "brave-browser") | .kind] | .[0] == "relaunch")
' >/dev/null || fail "brave should be relaunch-only after drop-startup"

"$RELAUNCH" drop --class brave-browser
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.class == "brave-browser")] | length == 0)
' >/dev/null || fail "drop from relaunch"

# --- boot skip / disable, and hidden self ---
"$RELAUNCH" import --exec proton-mail --workspace 3 >/dev/null
"$RELAUNCH" boot-skip
[[ -f "$RELAUNCH_CONFIG_DIR/skip-once" ]] || fail "skip-once file"
assert_not_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.conf")" 'Proton Mail'
"$RELAUNCH" boot
[[ -f "$RELAUNCH_CONFIG_DIR/skip-once" ]] && fail "skip-once not consumed"
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.conf")" 'Proton Mail'

"$RELAUNCH" boot-disable
[[ -f "$RELAUNCH_CONFIG_DIR/disabled" ]] || fail "disabled file"
"$RELAUNCH" list --json | jq -e '.boot.disabled == true and .boot.active == false' >/dev/null \
  || fail "boot disabled state"
"$RELAUNCH" boot-enable
[[ -f "$RELAUNCH_CONFIG_DIR/disabled" ]] && fail "disabled not cleared"
"$RELAUNCH" list --json | jq -e '.boot.active == true' >/dev/null || fail "boot re-enabled"

"$RELAUNCH" reload | grep -qx 'Hyprland reloaded.' || fail "reload text"

printf 'ok %s\n' "$(basename "$0")"
