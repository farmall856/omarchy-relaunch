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
  clients)  cat "${FAKE_CLIENTS:?}" ;;
  monitors) if [[ -n "${FAKE_MONITORS:-}" ]]; then cat "$FAKE_MONITORS"; else exit 1; fi ;;
  reload)   printf 'ok\n' ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$HYPRCTL_STUB"

export RELAUNCH_VERIFY_SLEEP=0
export RELAUNCH_CONFIG_DIR="$WORKDIR/cfg"
export RELAUNCH_AUTOSTART="$WORKDIR/autostart.lua"
export RELAUNCH_HYPRLAND_LUA="$WORKDIR/hyprland.lua"
export RELAUNCH_PLUGIN_DIR="$WORKDIR/plugin"
export RELAUNCH_HYPR_CONF="$WORKDIR/hyprland.conf"
export PREFIX="$WORKDIR"
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
grep -q 'io.open(_rl)' "$RELAUNCH_HYPRLAND_LUA" || fail "hyprland hook must skip a missing relaunch.lua"
[[ -f "$RELAUNCH_CONFIG_DIR/relaunch.lua" ]] || fail "ensure_hooks must create relaunch.lua before the hyprland hook"
[[ -e "$RELAUNCH_CONFIG_DIR/relaunch.conf" ]] && fail "must not write the leftover windowrulev2 relaunch.conf"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"

assert_contains "$lua" 'o.window({ class = "^(herdr)$" }, { workspace = "1 silent" })'
assert_contains "$lua" 'o.window({ class = "^(brave-browser)$" }, { workspace = "2 silent" })'
assert_contains "$lua" 'o.window({ class = "^(org\\.wezfurlong\\.wezterm)$" }, { workspace = "6 silent", float = true })'
assert_not_contains "$lua" 'windowrulev2'
assert_not_contains "$lua" 'exec-once'
printf '%s\n' 'windowrulev2 leftover' >"$RELAUNCH_CONFIG_DIR/relaunch.conf"
printf '%s\n' '# keep' '# omarchy-relaunch' "source = $RELAUNCH_CONFIG_DIR/relaunch.conf" '# after' \
  >"$RELAUNCH_HYPR_CONF"
"$RELAUNCH" generate >/dev/null
[[ -e "$RELAUNCH_CONFIG_DIR/relaunch.conf" ]] && fail "generate must delete leftover relaunch.conf"
grep -q 'relaunch.conf' "$RELAUNCH_HYPR_CONF" && fail "generate must unsource leftover relaunch.conf"
grep -q '# keep' "$RELAUNCH_HYPR_CONF" || fail "legacy conf cleanup ate unrelated hyprland.conf"

# unknown class with empty exec still gets a rule
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"entries":[{"class":"SomeApp","workspace":3,"exec":"","enabled":true}]}
EOF
"$RELAUNCH" generate >/dev/null
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'o.window({ class = "^(SomeApp)$" }'

# class → exec comes from .desktop files, not a built-in table
XDG_APPS="$WORKDIR/xdg/applications"
mkdir -p "$XDG_APPS"
cat >"$XDG_APPS/libreoffice-calc.desktop" <<'EOF'
[Desktop Entry]
Name=LibreOffice Calc
Exec=libreoffice --calc %U
StartupWMClass=libreoffice-calc
Type=Application
EOF
cat >"$XDG_APPS/brave.desktop" <<'EOF'
[Desktop Entry]
Name=Brave Web Browser
Exec=brave %U
StartupWMClass=brave-browser
Type=Application
EOF
cat >"$XDG_APPS/proton-mail.desktop" <<'EOF'
[Desktop Entry]
Name=Proton Mail
Exec=proton-mail %U
Type=Application
EOF
cat >"$XDG_APPS/pct-app.desktop" <<'EOF'
[Desktop Entry]
Name=Percent App
Exec=pct-app --token %%user %u
StartupWMClass=pct-app
Type=Application
EOF
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
export FAKE_CLIENTS="$WORKDIR/clients.json"
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}},
  {"class":"Proton Mail","initialClass":"Proton Mail","pid":72,"floating":false,"workspace":{"id":3}},
  {"class":"pct-app","initialClass":"pct-app","pid":73,"floating":false,"workspace":{"id":8}}
]
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
CALC_DESK="$XDG_APPS/libreoffice-calc.desktop"
MAIL_DESK="$XDG_APPS/proton-mail.desktop"
PCT_DESK="$XDG_APPS/pct-app.desktop"
# Packaged apps (LibreOffice) ship .desktop files as symlinks.
mkdir -p "$WORKDIR/xdg-sym/applications"
ln -s "$CALC_DESK" "$WORKDIR/xdg-sym/applications/libreoffice-calc.desktop"
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-sym"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}}]
EOF
"$RELAUNCH" save --json | jq -e --arg calc "gio launch $WORKDIR/xdg-sym/applications/libreoffice-calc.desktop" '
  ([.entries[] | select(.class == "libreoffice-calc") | .exec] == [$calc])
  and ([.entries[] | select(.class == "libreoffice-calc") | .execSource] == ["desktop-file"])
' >/dev/null || fail "symlinked .desktop files must be indexed"
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}},
  {"class":"Proton Mail","initialClass":"Proton Mail","pid":72,"floating":false,"workspace":{"id":3}},
  {"class":"pct-app","initialClass":"pct-app","pid":73,"floating":false,"workspace":{"id":8}}
]
EOF
"$RELAUNCH" save --json | jq -e --arg calc "gio launch $CALC_DESK" --arg mail "gio launch $MAIL_DESK" --arg pct "gio launch $PCT_DESK" '
  ([.entries[] | select(.class == "libreoffice-calc") | .exec] == [$calc])
  and ([.entries[] | select(.class == "Proton Mail") | .exec] == [$mail])
  and ([.entries[] | select(.class == "pct-app") | .exec] == [$pct])
  and ([.entries[] | select(.class == "libreoffice-calc") | .execSource] == ["desktop-file"])
' >/dev/null || fail "desktop match must store gio launch <desktop-file>"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"libreoffice-calc","workspace":7,"exec":"libreoffice-calc","enabled":true}
]}
EOF
"$RELAUNCH" save --json | jq -e --arg calc "gio launch $CALC_DESK" '
  [.entries[] | select(.class == "libreoffice-calc") | .exec] == [$calc]
' >/dev/null || fail "recapture must heal fallback exec from desktop"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"libreoffice-calc","workspace":7,"exec":"localc --norestore","enabled":true}
]}
EOF
"$RELAUNCH" save --json | jq -e '
  [.entries[] | select(.class == "libreoffice-calc") | .exec] == ["localc --norestore"]
' >/dev/null || fail "recapture must keep a real user exec override"

# User applications dir wins over a later system dir with the same Name=
mkdir -p "$WORKDIR/xdg-user/applications" "$WORKDIR/xdg-system/applications"
printf '%s\n' '[Desktop Entry]' 'Name=X' 'Exec=omarchy-launch-webapp https://x.com/' >"$WORKDIR/xdg-user/applications/X.desktop"
printf '%s\n' '[Desktop Entry]' 'Name=X' 'Exec=should-not-win' >"$WORKDIR/xdg-system/applications/X.desktop"
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-user:$WORKDIR/xdg-system"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"X","initialClass":"X","pid":74,"floating":false,"workspace":{"id":8}}]
EOF
"$RELAUNCH" save --json | jq -e --arg exec "gio launch $WORKDIR/xdg-user/applications/X.desktop" '
  [.entries[] | select(.class == "X") | .exec] == [$exec]
' >/dev/null || fail "user applications dir must win over system .desktop"

# A Brave PWA can reuse Name=Proton Mail. The official app window class must
# still map to proton-mail.desktop; the PWA maps by desktop-file id.
mkdir -p "$WORKDIR/xdg-pwa/applications"
cat >"$WORKDIR/xdg-pwa/applications/proton-mail.desktop" <<'EOF'
[Desktop Entry]
Name=Proton Mail
Exec=proton-mail %U
Type=Application
EOF
cat >"$WORKDIR/xdg-pwa/applications/brave-jnpecgipniidlgicjocehkhajgdnjekh-Default.desktop" <<'EOF'
[Desktop Entry]
Name=Proton Mail
Exec=/opt/brave-bin/brave --app-id=jnpecgipniidlgicjocehkhajgdnjekh
StartupWMClass=crx_jnpecgipniidlgicjocehkhajgdnjekh
Type=Application
EOF
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-pwa"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"Proton Mail","initialClass":"Proton Mail","pid":75,"floating":false,"workspace":{"id":3}},
  {"class":"brave-jnpecgipniidlgicjocehkhajgdnjekh-Default","initialClass":"brave-jnpecgipniidlgicjocehkhajgdnjekh-Default","pid":76,"floating":false,"workspace":{"id":3}}
]
EOF
PWA_DESK="$WORKDIR/xdg-pwa/applications/brave-jnpecgipniidlgicjocehkhajgdnjekh-Default.desktop"
OFFICIAL_MAIL="$WORKDIR/xdg-pwa/applications/proton-mail.desktop"
"$RELAUNCH" save --json | jq -e --arg official "gio launch $OFFICIAL_MAIL" --arg pwa "gio launch $PWA_DESK" '
  ([.entries[] | select(.class == "Proton Mail") | .exec] == [$official])
  and ([.entries[] | select(.class == "brave-jnpecgipniidlgicjocehkhajgdnjekh-Default") | .exec] == [$pwa])
' >/dev/null || fail "PWA Name= must not steal the official Proton Mail class"
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}}
]
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"libreoffice-calc","workspace":7,"exec":"libreoffice-calc","enabled":true,"execSource":"guess"}
]}
EOF

# Empty override file; user set-exec wins over .desktop on the next capture
[[ -f "$RELAUNCH_CONFIG_DIR/overrides.json" ]] || fail "overrides.json should exist after save"
jq -e 'type == "object"' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null || fail "overrides.json must be an object"
"$RELAUNCH" set-exec --class libreoffice-calc --exec "localc --norestore" --json >/dev/null
jq -e '.["libreoffice-calc"] == "localc --norestore"' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null \
  || fail "set-exec must write overrides.json"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
"$RELAUNCH" save --json | jq -e '
  ([.entries[] | select(.class == "libreoffice-calc") | .exec] == ["localc --norestore"])
  and ([.entries[] | select(.class == "libreoffice-calc") | .execSource] == ["overrides-table"])
' >/dev/null || fail "override must beat desktop-file on recapture"

# list / display must not search .desktop files. Add-from-running uses --class.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
printf '{}\n' >"$RELAUNCH_CONFIG_DIR/overrides.json"
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}}]
EOF
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.kind == "running" and .class == "libreoffice-calc")] | length == 1)
  and ([.rows[] | select(.kind == "running" and .class == "libreoffice-calc") | .execSource] == ["guess"])
' >/dev/null || fail "list must not resolve .desktop launchers for running windows"
"$RELAUNCH" import --class libreoffice-calc --workspace 7 --json | jq -e --arg calc "gio launch $CALC_DESK" '
  ([.entries[] | select(.class == "libreoffice-calc") | .exec] == [$calc])
  and ([.entries[] | select(.class == "libreoffice-calc") | .execSource] == ["desktop-file"])
' >/dev/null || fail "import --class must store gio launch <desktop-file>"
jq -e '.["libreoffice-calc"]' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null \
  && fail "import --class from desktop must not write overrides.json"
"$RELAUNCH" drop --class libreoffice-calc --json >/dev/null
"$RELAUNCH" set-exec --class missing --exec true 2>/dev/null \
  && fail "set-exec on missing class should fail"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"libreoffice-calc","workspace":7,"exec":"gio launch /tmp/x.desktop","enabled":true}
]}
EOF
"$RELAUNCH" set-exec --class libreoffice-calc --exec "localc --norestore" >/dev/null
jq -e '.["libreoffice-calc"] == "localc --norestore"' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null \
  || fail "set-exec must write override before drop"
"$RELAUNCH" drop --class libreoffice-calc >/dev/null
jq -e '.["libreoffice-calc"]' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null \
  && fail "drop must remove the class from overrides.json"

# Guess with a missing command is flagged immediately
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-empty"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/nocmd"
mkdir -p "$WORKDIR/xdg-empty/applications" "$WORKDIR/nocmd"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"NoSuchApp99","initialClass":"NoSuchApp99","pid":88,"floating":false,"workspace":{"id":9}}]
EOF
"$RELAUNCH" save --json | jq -e '
  (.entries[] | select(.class == "NoSuchApp99") | .execSource == "guess" and .execOk == false)
  and ([.warnings[] | select(.class == "NoSuchApp99")] | length == 1)
' >/dev/null || fail "guess with missing command must warn at save"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# Terminal identity walks to a parent foot --app-id / -e
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
printf 'herdr\0' >"$RELAUNCH_CMDLINE_DIR/90"
printf '91\n' >"$RELAUNCH_CMDLINE_DIR/90.ppid"
printf 'foot\0--app-id=herdr\0-e\0herdr\0' >"$RELAUNCH_CMDLINE_DIR/91"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"herdr","initialClass":"herdr","pid":90,"floating":false,"workspace":{"id":1}}]
EOF
"$RELAUNCH" save --json | jq -e '
  [.entries[] | select(.class == "herdr") | .exec] ==
    ["xdg-terminal-exec --app-id=herdr -e herdr"]
' >/dev/null || fail "parent terminal --app-id must supply the launch command"
unset RELAUNCH_CMDLINE_DIR

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
echo "$out" | jq -e --arg p "$RELAUNCH_CONFIG_DIR/relaunch.lua" '.snippetPath == $p' >/dev/null \
  || fail "snippetPath should be relaunch.lua"

cfg="$(cat "$RELAUNCH_CONFIG_DIR/config.json")"
echo "$cfg" | jq -e '
  (.entries | map(select(.class == "brave-browser")) | .[0]
    | .workspace == 3 and .exec == "brave --user-data-dir=x")
  and (.entries | map(select(.class == "foot")) | .[0]
    | .workspace == 6 and .enabled == false)
  and (.entries | map(select(.class == "signal")) | .[0]
    | .workspace == 4 and .exec == "signal" and .enabled == true)
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

export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
listed="$("$RELAUNCH" list --json)"
echo "$listed" | jq -e '
  ([.startup[].exec] | index("relaunch boot") == null)
  and ([.startup[].exec] | index("brave") != null)
  and ([.startup[] | select(.exec == "brave") | .class] | .[0] == "brave")
  and ([.startup[] | select(.exec | startswith("setsid")) | .class] | .[0] == "org.omarchy.terminal")
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
jq -e '.skipOnce == true' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "skipOnce not persisted in config"
assert_not_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'o.window'
"$RELAUNCH" save >/dev/null
jq -e '.skipOnce == true' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "save dropped skipOnce"
[[ -f "$RELAUNCH_CONFIG_DIR/skip-once" ]] || fail "save removed skip-once file"
assert_not_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'o.window'
rm -f "$RELAUNCH_CONFIG_DIR/skip-once"
"$RELAUNCH" boot
[[ -f "$RELAUNCH_CONFIG_DIR/skip-once" ]] && fail "skip-once not consumed"
jq -e '.skipOnce == true' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null && fail "skipOnce left armed after boot"
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'o.window'
"$RELAUNCH" last-boot --json | jq -e '.outcome == "skipped" and (.text | length) > 0' >/dev/null \
  || fail "skip last-boot log"

"$RELAUNCH" boot-disable
[[ -f "$RELAUNCH_CONFIG_DIR/disabled" ]] || fail "disabled file"
"$RELAUNCH" boot
"$RELAUNCH" last-boot --json | jq -e '.outcome == "disabled"' >/dev/null || fail "disabled last-boot log"
"$RELAUNCH" list --json | jq -e '.boot.disabled == true and .boot.active == false and .lastBoot.outcome == "disabled"' >/dev/null \
  || fail "boot disabled state"
"$RELAUNCH" boot-enable
[[ -f "$RELAUNCH_CONFIG_DIR/disabled" ]] && fail "disabled not cleared"
"$RELAUNCH" list --json | jq -e '.boot.active == true' >/dev/null || fail "boot re-enabled"

"$RELAUNCH" reload | grep -qx 'Hyprland reloaded.' || fail "reload text"

cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"NoSuch","workspace":1,"exec":"/no/such/relaunch-test-cmd","enabled":true}
]}
EOF
"$RELAUNCH" boot
"$RELAUNCH" last-boot --json | jq -e '
  .outcome == "launched"
  and .launches[0].status == "not_found"
  and (.text | contains("not_found"))
' >/dev/null || fail "launch not_found last-boot log"

mkdir -p "$WORKDIR/bin"
cat >"$WORKDIR/bin/omarchy-launch-editor" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$1" >"${OPENED_FILE:?}"
EOF
chmod +x "$WORKDIR/bin/omarchy-launch-editor"
export OPENED_FILE="$WORKDIR/opened-log"
PATH="$WORKDIR/bin:$PATH" "$RELAUNCH" last-boot --open >/dev/null
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [[ -f "$OPENED_FILE" ]] && break
  sleep 0.05
done
[[ "$(cat "$OPENED_FILE")" == "$RELAUNCH_CONFIG_DIR/last-boot.log" ]] \
  || fail "last-boot --open did not pass the log path"

# Terminal-wrapped commands get their own class from argv, not the terminal.
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
printf 'foot\0-e\0herdr\0' >"$RELAUNCH_CMDLINE_DIR/4242"
printf 'foot\0\0' >"$RELAUNCH_CMDLINE_DIR/4243"
printf 'foot\0-e\0btop\0' >"$RELAUNCH_CMDLINE_DIR/4244"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class": "foot", "initialClass": "foot", "pid": 4242, "floating": false, "workspace": {"id": 1}},
  {"class": "foot", "initialClass": "foot", "pid": 4243, "floating": false, "workspace": {"id": 6}},
  {"class": "foot", "initialClass": "foot", "pid": 4244, "floating": false, "workspace": {"id": 3}}
]
EOF
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.kind == "running" and .class == "herdr" and .workspace == 1)] | length == 1)
  and ([.rows[] | select(.kind == "running" and .class == "foot" and .workspace == 6)] | length == 1)
  and ([.rows[] | select(.kind == "running" and .class == "btop" and .workspace == 3)] | length == 1)
' >/dev/null || fail "wrapped terminal identity"
"$RELAUNCH" save --json | jq -e '
  ([.entries[] | select(.class == "herdr" and .workspace == 1 and (.exec | contains("--app-id=herdr")))] | length == 1)
  and ([.entries[] | select(.class == "btop" and .workspace == 3 and (.exec | contains("--app-id=btop")))] | length == 1)
  and ([.entries[] | select(.class == "foot" and .workspace == 6)] | length == 1)
  and ([.rows[] | select(.inRelaunch) | .workspace] == [1, 3, 6])
' >/dev/null || fail "save splits wrapped terminals"

printf 'foot\0--app-id=mytool\0-e\0mytool\0--flag\0value\0' >"$RELAUNCH_CMDLINE_DIR/4300"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class": "mytool", "initialClass": "mytool", "pid": 4300, "floating": false, "workspace": {"id": 2}}]
EOF
"$RELAUNCH" save --json | jq -e '
  [.entries[] | select(.class == "mytool") | .exec] ==
    ["xdg-terminal-exec --app-id=mytool -e mytool --flag value"]
' >/dev/null || fail "explicit --app-id must keep trailing hosted args"
unset RELAUNCH_CMDLINE_DIR

# --- hosted argv keeps argument boundaries (Omarchy TUI .desktop shape) ---
# Disk Usage runs as: foot --app-id=TUI.float -e bash -c 'dua i /'
# "dua i /" is ONE argv element. A "${inner[*]}" join loses that, and boot's
# bash -c re-split then runs `dua` with no arguments: it prints and exits, so
# the window is gone before the workspace pin applies.
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
printf 'foot\0--app-id=TUI.float\0-e\0bash\0-c\0dua i /\0' >"$RELAUNCH_CMDLINE_DIR/4400"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class": "TUI.float", "initialClass": "TUI.float", "pid": 4400, "floating": true, "workspace": {"id": 4}}]
EOF
saved="$("$RELAUNCH" save --json | jq -r '.entries[] | select(.class == "TUI.float") | .exec')"
[[ -n "$saved" ]] || fail "hosted TUI must be captured"
# The saved string must re-split, through the same bash -c boot uses, back into
# the exact argv /proc reported — 5 words with the script as a single argument.
eval "set -- ${saved#xdg-terminal-exec }"
[[ "$#" -eq 5 ]] || fail "hosted TUI argv must re-split into 5 words, got $#"
[[ "$1" == "--app-id=TUI.float" ]] || fail "hosted TUI must keep its app-id, got $1"
[[ "$2" == "-e" ]] || fail "hosted TUI must keep -e, got $2"
[[ "$5" == "dua i /" ]] || fail "hosted TUI must keep 'dua i /' as one argument, got [$5]"

# --- a stale terminal-source entry re-heals on recapture ---
# Entries written by an older engine carry execSource "terminal" with a
# flattened, -e-less command. That is not a fallback source, so the old heal
# rule skipped it and the broken value was sticky forever. Terminal identity
# is re-derived from live /proc on every save, and the user's own text lives
# in overrides.json, so a differing resolution must win.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"TUI.float","workspace":4,"exec":"xdg-terminal-exec --app-id=TUI.float bash -c dua i /","execSource":"terminal","enabled":true,"float":true}
]}
EOF
out="$("$RELAUNCH" save --json)"
echo "$out" | jq -e --arg want "$saved" '
  ([.entries[] | select(.class == "TUI.float") | .exec] == [$want])
  and ([.entries[] | select(.class == "TUI.float") | .execSource] == ["terminal"])
  and ([.entries[] | select(.class == "TUI.float") | .float] == [true])
' >/dev/null || fail "stale terminal entry must re-heal: $out"
echo "$out" | jq -e '.updated >= 1' >/dev/null   || fail "re-healing a stale terminal entry must count as an update"

# A healed entry is stable: a second save with the same window is a no-op.
"$RELAUNCH" save --json | jq -e '.updated == 0' >/dev/null   || fail "recapturing an already-correct terminal entry must not churn"

# The user's own command still wins. set-exec records "overrides-table", and
# resolve_launch checks the terminal host first, so only the stored source
# keeps the edit from being healed away.
"$RELAUNCH" set-exec --class TUI.float --exec "xdg-terminal-exec --app-id=TUI.float -e dua i /home" --json >/dev/null
"$RELAUNCH" save --json | jq -e '
  ([.entries[] | select(.class == "TUI.float") | .exec] == ["xdg-terminal-exec --app-id=TUI.float -e dua i /home"])
  and ([.entries[] | select(.class == "TUI.float") | .execSource] == ["overrides-table"])
' >/dev/null || fail "a user set-exec must survive recapture of a terminal-hosted window"
"$RELAUNCH" drop --class TUI.float --json >/dev/null

# A stale desktop-file entry re-heals the same way.
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"libreoffice-calc","workspace":7,"exec":"gio launch /gone/libreoffice-calc.desktop","execSource":"desktop-file","enabled":true}
]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"libreoffice-calc","initialClass":"libreoffice-calc","pid":71,"floating":false,"workspace":{"id":7}}]
EOF
"$RELAUNCH" save --json | jq -e --arg calc "gio launch $CALC_DESK" '
  [.entries[] | select(.class == "libreoffice-calc") | .exec] == [$calc]
' >/dev/null || fail "stale desktop-file entry must re-heal to the indexed .desktop"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# --- float/tile is captured and pinned in both directions ---
# Omarchy tags TUI.float +floating-window in default/hypr/apps/system.lua, so
# a pin that stays silent about float lets that tag win and a window that was
# tiled comes back floating at 875x600. The generated rule has to say which
# one it wants. relaunch.lua is loaded after the Omarchy defaults, so it wins.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"floaty","initialClass":"floaty","pid":510,"floating":true,"workspace":{"id":3}},
  {"class":"tiley","initialClass":"tiley","pid":511,"floating":false,"workspace":{"id":4}}
]
EOF
"$RELAUNCH" save --json | jq -e '
  ([.entries[] | select(.class == "floaty") | .float] == [true])
  and ([.entries[] | select(.class == "tiley") | .float] == [false])
' >/dev/null || fail "capture must record float as a real boolean in both directions"
jq -e '
  ([.entries[] | select(.class == "tiley") | has("float")] == [true])
' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "float:false must survive the config round trip, not be dropped as falsy"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"
assert_contains "$lua" 'o.window({ class = "^(floaty)$" }, { workspace = "3 silent", float = true })'
assert_contains "$lua" 'o.window({ class = "^(tiley)$" }, { workspace = "4 silent", tile = true })'
# tile = true alone loses to Omarchy's floating-window tag: verified on
# hardware, a tagged class came back floating at 875x600 with the tile rule
# present. The tag has to be removed first. A tiled entry therefore emits an
# untag line as well; a floating one must not.
assert_contains "$lua" 'o.window({ class = "^(tiley)$" }, { tag = "-floating-window" })'
assert_not_contains "$lua" 'o.window({ class = "^(floaty)$" }, { tag = "-floating-window" })'
[[ "$(grep -n 'tag = "-floating-window"' <<<"$lua" | cut -d: -f1)" \
   -lt "$(grep -n 'class = "\^(tiley)\$" }, { workspace' <<<"$lua" | cut -d: -f1)" ]] \
  || fail "the untag line must come before the placement rule it enables"

# Recapture refreshes float from the live window, the same way it refreshes
# workspace. Both directions, and neither counts as a second update when the
# workspace moved too.
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"floaty","initialClass":"floaty","pid":510,"floating":false,"workspace":{"id":3}},
  {"class":"tiley","initialClass":"tiley","pid":511,"floating":true,"workspace":{"id":4}}
]
EOF
out="$("$RELAUNCH" save --json)"
echo "$out" | jq -e '
  ([.entries[] | select(.class == "floaty") | .float] == [false])
  and ([.entries[] | select(.class == "tiley") | .float] == [true])
' >/dev/null || fail "recapture must refresh float from the live window: $out"
echo "$out" | jq -e '.updated == 2' >/dev/null   || fail "a float-only change is one update per entry: $out"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"
assert_contains "$lua" 'o.window({ class = "^(floaty)$" }, { workspace = "3 silent", tile = true })'
assert_contains "$lua" 'o.window({ class = "^(tiley)$" }, { workspace = "4 silent", float = true })'

# One window whose workspace and float both moved is still one update.
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"floaty","initialClass":"floaty","pid":510,"floating":true,"workspace":{"id":9}}]
EOF
"$RELAUNCH" save --json | jq -e '.updated == 1' >/dev/null   || fail "updated counts entries, not changed fields"

# A legacy entry with no float at all is unknown, not tiled: it must not
# start emitting tile = true until a live window says so.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"legacy","workspace":2,"exec":"legacy","execSource":"guess","enabled":true}
]}
EOF
"$RELAUNCH" generate >/dev/null
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"
assert_contains "$lua" 'o.window({ class = "^(legacy)$" }, { workspace = "2 silent" })'
assert_not_contains "$lua" 'legacy)$" }, { workspace = "2 silent", tile'
# …and import, which has no window to read, leaves it unknown too.
"$RELAUNCH" import --exec legacy2 --workspace 5 --json >/dev/null
jq -e '[.entries[] | select(.class == "legacy2") | has("float")] == [false]'   "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "import has no live window, so float must stay unknown"

# --- per-window snapshot (recorded for future use; changes nothing today) ---
# entries[] stays one-per-class and the pins stay identical. windows[] is a
# raw record of every observed window, so a future per-window feature has
# real data to work from.
cat >"$WORKDIR/monitors.json" <<'EOF'
[
  {"id": 0, "name": "eDP-1", "description": "BOE 0x0BCA"},
  {"id": 1, "name": "DP-3", "description": "Dell Inc. DELL U2720Q ABCD123"}
]
EOF
export FAKE_MONITORS="$WORKDIR/monitors.json"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"foot","initialClass":"foot","pid":600,"floating":false,"monitor":0,"workspace":{"id":1}},
  {"class":"foot","initialClass":"foot","pid":601,"floating":true,"monitor":1,"workspace":{"id":5}},
  {"class":"brave-browser","initialClass":"brave-browser","pid":602,"floating":false,"monitor":1,"workspace":{"id":2}},
  {"class":"scratchpad","initialClass":"scratchpad","pid":603,"floating":true,"monitor":0,"workspace":{"id":-98}}
]
EOF
"$RELAUNCH" save >/dev/null
snap="$(jq -c '.windows' "$RELAUNCH_CONFIG_DIR/config.json")"

# Every window, not just the first of each class.
jq -e '(.windows | length) == 4' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "snapshot must record every window, got: $snap"
jq -e '[.windows[] | select(.class == "foot")] | length == 2' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "snapshot must not dedupe by class: $snap"

# …while entries[] and the pins keep the one-entry-per-class model.
jq -e '[.entries[] | select(.class == "foot")] | length == 1' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "snapshot must not change the one-entry-per-class model"
jq -e '[.entries[] | select(.class == "foot") | .workspace] == [1]' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "first-seen lowest workspace still wins for the entry"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"
# Count placement rules only: a tiled entry also emits a tag-removal line.
[[ "$(grep -c 'class = "\^(foot)\$" }, { workspace' <<<"$lua")" -eq 1 ]]   || fail "snapshot must not add pins; expected exactly one foot placement rule"
assert_not_contains "$lua" 'scratchpad'

# Monitor id, name and description all land. Output names are reassigned
# across reboots, so the description is what identifies the physical panel.
jq -e '
  ([.windows[] | select(.class == "brave-browser")]
    | .[0]
    | .monitor == 1
      and .monitorName == "DP-3"
      and .monitorDescription == "Dell Inc. DELL U2720Q ABCD123")
' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "snapshot must record monitor id, name and description: $snap"
jq -e '
  ([.windows[] | select(.class == "foot" and .monitor == 0)]
    | .[0] | .monitorName == "eDP-1" and .monitorDescription == "BOE 0x0BCA")
' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "per-window monitor lookup must not collapse to one monitor: $snap"

# Float state is per window here, not per class.
jq -e '[.windows[] | select(.class == "foot") | .floating] | sort == [false, true]'   "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "snapshot must record per-window float: $snap"

# A snapshot loses information if it applies capture's filters, so special
# and negative workspaces are kept even though they can never be pinned.
jq -e '[.windows[] | select(.workspace == -98) | .class] == ["scratchpad"]'   "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "snapshot must keep special workspaces: $snap"
jq -e '[.entries[] | select(.class == "scratchpad")] | length == 0'   "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "special workspaces must still be excluded from entries"

# Commands that rewrite config.json must carry the snapshot through.
"$RELAUNCH" import --exec somethingelse --workspace 8 --json >/dev/null
jq -e --argjson want "$snap" '.windows == $want' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "import must not drop the window snapshot"
"$RELAUNCH" drop --class somethingelse --json >/dev/null
jq -e --argjson want "$snap" '.windows == $want' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "drop must not drop the window snapshot"
"$RELAUNCH" generate >/dev/null
jq -e --argjson want "$snap" '.windows == $want' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "generate must not drop the window snapshot"

# hyprctl monitors failing is not a save failure: names degrade to empty.
unset FAKE_MONITORS
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
"$RELAUNCH" save >/dev/null || fail "save must survive hyprctl monitors failing"
jq -e '
  ([.windows[] | select(.class == "brave-browser")]
    | .[0] | .monitor == 1 and .monitorName == "" and .monitorDescription == "")
' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null   || fail "unreadable monitors must degrade to empty names, keeping the id"

# --- display labels: stored at save time, one tier at a time ---
# class stays the identity and the only lookup key; label is display only.
LBL="$WORKDIR/lbl"
mkdir -p "$LBL/applications"
cat >"$LBL/applications/brave-browser.desktop" <<'EOF'
[Desktop Entry]
Name=Brave Web Browser
Exec=brave %U
StartupWMClass=brave-browser
Type=Application
EOF
# Omarchy ships Disk Usage exactly like this: the class is the generic
# TUI.float, and only the wrapped leaf command keys back to the friendly name.
cat >"$LBL/applications/Disk Usage.desktop" <<'EOF'
[Desktop Entry]
Name=Disk Usage
Exec=xdg-terminal-exec --app-id=TUI.float -e bash -c "dua i /"
Type=Application
EOF
export RELAUNCH_DATA_DIRS="$LBL"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/lblcmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
# tier 2a: hosted TUI whose leaf command matches a .desktop terminal wrap
printf 'foot\0--app-id=TUI.float\0-e\0bash\0-c\0dua i /\0' >"$RELAUNCH_CMDLINE_DIR/700"
# tier 2b: hosted TUI with no .desktop anywhere
printf 'foot\0--app-id=herdr\0-e\0herdr\0' >"$RELAUNCH_CMDLINE_DIR/701"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"TUI.float","initialClass":"TUI.float","pid":700,"floating":true,"workspace":{"id":3}},
  {"class":"herdr","initialClass":"herdr","pid":701,"floating":false,"workspace":{"id":1}},
  {"class":"brave-browser","initialClass":"brave-browser","pid":702,"floating":false,"workspace":{"id":2}},
  {"class":"NoDesktopApp","initialClass":"NoDesktopApp","pid":703,"floating":false,"workspace":{"id":8}}
]
EOF
printf '/opt/weird/bin/nodesktop-bin\0--flag\0' >"$RELAUNCH_CMDLINE_DIR/703"
out="$("$RELAUNCH" save --json)"
lbl() { jq -r --arg c "$1" '[.entries[] | select(.class == $c) | .label] | .[0] // "(none)"' <<<"$out"; }

# Tier 1: execSource desktop-file -> Name= from the exact file the exec names.
[[ "$(lbl brave-browser)" == "Brave Web Browser" ]] \
  || fail "tier 1: desktop-file label must come from Name=, got $(lbl brave-browser)"
# Tier 2a: terminal -> leaf `dua` -> the .desktop whose Exec wraps it.
[[ "$(lbl TUI.float)" == "Disk Usage" ]] \
  || fail "tier 2a: hosted TUI must resolve through its leaf command, got $(lbl TUI.float)"
# Tier 2b: terminal with no matching .desktop keeps the leaf itself.
[[ "$(lbl herdr)" == "herdr" ]] \
  || fail "tier 2b: hosted TUI with no .desktop must fall back to the leaf, got $(lbl herdr)"
# Tier 3: anything else -> leaf of the exec, basename only.
[[ "$(lbl NoDesktopApp)" == "nodesktop-bin" ]] \
  || fail "tier 3: label must be the leaf of the exec, got $(lbl NoDesktopApp)"

# Tier 4: no exec to read at all -> class.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"LegacyApp","workspace":2,"exec":"","execSource":"","enabled":true}
]}
EOF
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.class == "LegacyApp") | .label] == ["LegacyApp"]
' >/dev/null || fail "tier 4: an entry with no label must display as its class"

# The stored label survives, and the class remains the only lookup key: the
# generated pin still matches on class, never on the label.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
"$RELAUNCH" save >/dev/null
jq -e '[.entries[] | select(.class == "TUI.float") | .label] == ["Disk Usage"]' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "label must be persisted to config.json"
lua="$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")"
assert_contains "$lua" 'class = "^(TUI\\.float)$"'
assert_not_contains "$lua" 'Disk Usage'

# Recapture recomputes it, so a label self-heals when the .desktop changes.
cat >"$LBL/applications/Disk Usage.desktop" <<'EOF'
[Desktop Entry]
Name=Disk Usage Analyzer
Exec=xdg-terminal-exec --app-id=TUI.float -e bash -c "dua i /"
Type=Application
EOF
"$RELAUNCH" save >/dev/null
jq -e '[.entries[] | select(.class == "TUI.float") | .label] == ["Disk Usage Analyzer"]' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "recapture must recompute the label, like float and exec"

# A stale hand-edited label is replaced, not preserved: it is not a user field.
jq '.entries |= map(if .class == "TUI.float" then .label = "Stale Name" else . end)' \
  "$RELAUNCH_CONFIG_DIR/config.json" >"$WORKDIR/t" && mv "$WORKDIR/t" "$RELAUNCH_CONFIG_DIR/config.json"
"$RELAUNCH" save >/dev/null
jq -e '[.entries[] | select(.class == "TUI.float") | .label] == ["Disk Usage Analyzer"]' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "a stale label must be recomputed"

# list must not resolve labels: it reads what save stored, nothing more.
# Deleting the whole index cannot change what list reports.
mv "$LBL/applications" "$LBL/applications-gone"
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.class == "TUI.float") | .label] == ["Disk Usage Analyzer"]
' >/dev/null || fail "list must display the stored label without touching .desktop files"
mv "$LBL/applications-gone" "$LBL/applications"

# import stores a label too.
"$RELAUNCH" drop --class brave-browser --json >/dev/null
"$RELAUNCH" import --class brave-browser --workspace 2 --json >/dev/null
jq -e '[.entries[] | select(.class == "brave-browser") | .label] == ["Brave Web Browser"]' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "import must store a label"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# --- manual session snapshot (diagnostic; user-invoked; changes nothing) ---
export FAKE_MONITORS="$WORKDIR/monitors.json"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"brave-browser","workspace":2,"exec":"brave","execSource":"guess","label":"Brave","enabled":true}
]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"brave-browser","initialClass":"brave-browser","pid":800,"floating":false,"monitor":0,"workspace":{"id":2},"title":"Home - Brave"},
  {"class":"foot","initialClass":"foot","pid":801,"floating":true,"monitor":1,"workspace":{"id":6},"title":"~"},
  {"class":"foot","initialClass":"foot","pid":802,"floating":false,"monitor":1,"workspace":{"id":8},"title":"logs"}
]
EOF
"$RELAUNCH" snapshot >/dev/null || fail "snapshot must succeed"
SNAP="$RELAUNCH_CONFIG_DIR/last-session.json"
[[ -f "$SNAP" ]] || fail "snapshot must write last-session.json"
jq -e '.windows | length == 3' "$SNAP" >/dev/null \
  || fail "snapshot records every window, not one per class"
jq -e '
  [.windows[] | select(.pid == 800)] | .[0]
  | .class == "brave-browser" and .label == "Brave" and .workspace == 2
    and .floating == false and .monitor == 0 and .monitorName == "eDP-1"
    and .title == "Home - Brave"
' "$SNAP" >/dev/null || fail "snapshot must record class, label, workspace, float, monitor, pid and title"
jq -e '[.windows[] | select(.class == "foot") | .workspace] | sort == [6, 8]' "$SNAP" >/dev/null \
  || fail "two windows of one class must both appear in the snapshot"
# A class with no stored label falls back to its class, not to an empty string.
jq -e '[.windows[] | select(.class == "foot") | .label] | unique == ["foot"]' "$SNAP" >/dev/null \
  || fail "an unlabelled class must fall back to the class name"
jq -e '.capturedAt | test("^[0-9]{4}-")' "$SNAP" >/dev/null || fail "snapshot must be timestamped"

# The snapshot is diagnostic: it must not touch entries or the pins.
jq -e '.entries | length == 1' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "snapshot must not rewrite entries"
[[ -f "$RELAUNCH_CONFIG_DIR/last-session.json" ]] || fail "snapshot path"

# Diff: a class that did not come back is reported, not silently dropped.
cat >"$RELAUNCH_CONFIG_DIR/last-boot.json" <<'EOF'
{"startedAt":"2026-01-01T00:00:00-00:00","outcome":"launched","launches":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"brave-browser","initialClass":"brave-browser","pid":900,"floating":false,"monitor":0,"workspace":{"id":2},"title":"Home - Brave"},
  {"class":"foot","initialClass":"foot","pid":901,"floating":false,"monitor":1,"workspace":{"id":6},"title":"~"}
]
EOF
report="$("$RELAUNCH" last-session --diff --json)"
echo "$report" | jq -e '
  [.classes[] | select(.class == "foot") | {want, live, missing}] == [{want: 2, live: 1, missing: 1}]
' >/dev/null || fail "diff must report a window that did not come back: $report"
echo "$report" | jq -e '
  [.classes[] | select(.class == "brave-browser") | .missing] == [0]
' >/dev/null || fail "diff must not report a class that came back intact: $report"
"$RELAUNCH" last-session --diff | grep -q "MISSING" \
  || fail "human diff must show a MISSING row"
unset FAKE_MONITORS

# --- cmdline fallback keeps argv boundaries (github issue #1) ---
# Non-terminal window, no .desktop match, so resolve falls through to
# /proc. Boot runs the stored string through bash -c, so a "${argv[*]}"
# join lets spaces, quotes, semicolons, ampersands, globs and $(...) change
# meaning at launch.
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-empty"
mkdir -p "$WORKDIR/xdg-empty/applications"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/argvcmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
printf 'weirdapp\0--title=two words\0--re=a;b&c\0--glob=*.txt\0--sub=$(touch /tmp/rl-pwned)\0--q=he said "hi"\0--bs=back\\slash\0' \
  >"$RELAUNCH_CMDLINE_DIR/950"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"weirdapp","initialClass":"weirdapp","pid":950,"floating":false,"workspace":{"id":4}}]
EOF
saved="$("$RELAUNCH" save --json | jq -r '.entries[] | select(.class == "weirdapp") | .exec')"
[[ -n "$saved" ]] || fail "cmdline fallback must capture something"
"$RELAUNCH" save --json | jq -e '
  [.entries[] | select(.class == "weirdapp") | .execSource] == ["cmdline"]
' >/dev/null || fail "this fixture must exercise the cmdline fallback"

# Re-split through the same bash -c boot uses, and compare to the fixture.
mapfile -t got < <(PATH="$WORKDIR/argvstub:$PATH" bash -c "$saved" 2>/dev/null)
mkdir -p "$WORKDIR/argvstub"
cat >"$WORKDIR/argvstub/weirdapp" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@"
EOF
chmod +x "$WORKDIR/argvstub/weirdapp"
mapfile -t got < <(PATH="$WORKDIR/argvstub:$PATH" bash -c "$saved" 2>/dev/null)
[[ "${#got[@]}" -eq 6 ]] || fail "cmdline argv must re-split into 6 arguments, got ${#got[@]}: ${got[*]}"
[[ "${got[0]}" == "--title=two words" ]] || fail "spaces must stay inside one argument, got ${got[0]}"
[[ "${got[1]}" == '--re=a;b&c' ]] || fail "semicolon and ampersand must stay literal, got ${got[1]}"
[[ "${got[2]}" == '--glob=*.txt' ]] || fail "glob must not expand, got ${got[2]}"
[[ "${got[3]}" == '--sub=$(touch /tmp/rl-pwned)' ]] || fail "command substitution must stay literal, got ${got[3]}"
[[ "${got[4]}" == '--q=he said "hi"' ]] || fail "quotes must stay literal, got ${got[4]}"
[[ "${got[5]}" == '--bs=back\slash' ]] || fail "backslash must stay literal, got ${got[5]}"
[[ ! -e /tmp/rl-pwned ]] || fail "command substitution executed at launch"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# --- Omarchy web-app windows resolve to their launchers (github issue #11) ---
# Chromium encodes an --app=URL class as brave-<host>_<path with / as _>-Default.
# No StartupWMClass, desktop-file id or Name can match that, so resolution used
# to fall through to the generic browser cmdline and boot started plain Brave.
WA_USER="$WORKDIR/wa-user/applications"
WA_SYS="$WORKDIR/wa-sys/applications"
mkdir -p "$WA_USER" "$WA_SYS"
cat >"$WA_USER/Google Maps.desktop" <<'EOF'
[Desktop Entry]
Name=Google Maps
Exec=omarchy-launch-webapp https://maps.google.com
Type=Application
EOF
cat >"$WA_USER/Deep App.desktop" <<'EOF'
[Desktop Entry]
Name=Deep App
Exec=omarchy-launch-webapp https://example.com/foo/bar
Type=Application
EOF
cat >"$WA_USER/Query App.desktop" <<'EOF'
[Desktop Entry]
Name=Query App
Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2
Type=Application
EOF
cat >"$WA_USER/Hyphen App.desktop" <<'EOF'
[Desktop Entry]
Name=Hyphen App
Exec=omarchy-launch-webapp https://my-site.com/a-b
Type=Application
EOF
cat >"$WA_SYS/brave-browser.desktop" <<'EOF'
[Desktop Entry]
Name=Brave Web Browser
Exec=brave %U
StartupWMClass=brave-browser
Type=Application
EOF
export RELAUNCH_DATA_DIRS="$WORKDIR/wa-user:$WORKDIR/wa-sys"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/wacmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
# Every web-app window reports the MAIN browser argv: Chromium merges an
# --app= request into its existing process, so cmdline can never recover it.
BRAVE_ARGV='/opt/brave-bin/brave\0--ozone-platform=wayland\0'
printf "$BRAVE_ARGV" >"$RELAUNCH_CMDLINE_DIR/1000"
printf "$BRAVE_ARGV" >"$RELAUNCH_CMDLINE_DIR/1001"
printf "$BRAVE_ARGV" >"$RELAUNCH_CMDLINE_DIR/1002"
printf "$BRAVE_ARGV" >"$RELAUNCH_CMDLINE_DIR/1003"
printf "$BRAVE_ARGV" >"$RELAUNCH_CMDLINE_DIR/1004"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"brave-maps.google.com__-Default","initialClass":"brave-maps.google.com__-Default","pid":1000,"floating":false,"workspace":{"id":8}},
  {"class":"brave-example.com__foo_bar-Default","initialClass":"brave-example.com__foo_bar-Default","pid":1001,"floating":false,"workspace":{"id":9}},
  {"class":"brave-queried.example.com__-Default","initialClass":"brave-queried.example.com__-Default","pid":1002,"floating":false,"workspace":{"id":10}},
  {"class":"brave-my-site.com__a-b-Default","initialClass":"brave-my-site.com__a-b-Default","pid":1003,"floating":false,"workspace":{"id":6}},
  {"class":"brave-browser","initialClass":"brave-browser","pid":1004,"floating":false,"workspace":{"id":2}}
]
EOF
out="$("$RELAUNCH" save --json)"
wex() { jq -r --arg c "$1" '[.entries[] | select(.class == $c) | .exec] | .[0] // "(none)"' <<<"$out"; }
wsrc() { jq -r --arg c "$1" '[.entries[] | select(.class == $c) | .execSource] | .[0] // "(none)"' <<<"$out"; }

# Root URL: the Maps case from the issue.
[[ "$(wex 'brave-maps.google.com__-Default')" == "gio launch $WA_USER/Google\ Maps.desktop" ]] \
  || fail "root-URL web app must resolve to its desktop file, got $(wex 'brave-maps.google.com__-Default')"
[[ "$(wsrc 'brave-maps.google.com__-Default')" == "desktop-file" ]] \
  || fail "web-app resolution must record execSource desktop-file, got $(wsrc 'brave-maps.google.com__-Default')"
# A meaningful path.
[[ "$(wex 'brave-example.com__foo_bar-Default')" == "gio launch $WA_USER/Deep\ App.desktop" ]] \
  || fail "path URL must resolve, got $(wex 'brave-example.com__foo_bar-Default')"
# Query strings are discarded by Chromium, so identity must ignore them.
[[ "$(wex 'brave-queried.example.com__-Default')" == "gio launch $WA_USER/Query\ App.desktop" ]] \
  || fail "query string must be excluded from identity, got $(wex 'brave-queried.example.com__-Default')"
# Hosts and paths containing hyphens must survive anchored parsing.
[[ "$(wex 'brave-my-site.com__a-b-Default')" == "gio launch $WA_USER/Hyphen\ App.desktop" ]] \
  || fail "hyphenated host/path must resolve, got $(wex 'brave-my-site.com__a-b-Default')"
# Ordinary browser windows are untouched by the new tier.
[[ "$(wex 'brave-browser')" == "gio launch $WA_SYS/brave-browser.desktop" ]] \
  || fail "a normal browser window must still resolve by StartupWMClass, got $(wex 'brave-browser')"

# A previously saved cmdline entry heals to desktop-file on recapture.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"brave-maps.google.com__-Default","workspace":8,"exec":"/opt/brave-bin/brave --ozone-platform=wayland","execSource":"cmdline","enabled":true}
]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"brave-maps.google.com__-Default","initialClass":"brave-maps.google.com__-Default","pid":1000,"floating":false,"workspace":{"id":8}}]
EOF
"$RELAUNCH" save --json | jq -e --arg e "gio launch $WA_USER/Google\\ Maps.desktop" '
  ([.entries[] | select(.class == "brave-maps.google.com__-Default") | .exec] == [$e])
  and ([.entries[] | select(.class == "brave-maps.google.com__-Default") | .execSource] == ["desktop-file"])
' >/dev/null || fail "a stale cmdline web-app entry must heal to desktop-file"
# …and the label now comes from the exact file.
jq -e '[.entries[] | select(.class == "brave-maps.google.com__-Default") | .label] == ["Google Maps"]' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null || fail "healed web app must label from Name="

# list must not resolve: deleting the index cannot change what it reports.
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.class == "brave-maps.google.com__-Default") | .execSource] == ["desktop-file"]
' >/dev/null || fail "list must report the stored source without resolving"

# --- two windows sharing one pid resolve independently ---
# Chromium serves every web app from one process, so a Google Maps window and
# the ordinary browser window report the SAME pid. Patches used to be keyed by
# pid, so from_entries collapsed them and both windows took whichever
# resolution ran last -- Maps resolved to brave-browser.desktop.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0xaaa","class":"brave-maps.google.com__-Default","initialClass":"brave-maps.google.com__-Default","pid":1000,"floating":false,"workspace":{"id":8}},
  {"address":"0xbbb","class":"brave-browser","initialClass":"brave-browser","pid":1000,"floating":false,"workspace":{"id":2}}
]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(wex 'brave-maps.google.com__-Default')" == "gio launch $WA_USER/Google\ Maps.desktop" ]] \
  || fail "shared pid must not collapse resolutions, got $(wex 'brave-maps.google.com__-Default')"
[[ "$(wex 'brave-browser')" == "gio launch $WA_SYS/brave-browser.desktop" ]] \
  || fail "shared pid must not collapse resolutions for the browser window either"
jq -e '.entries | length == 2' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "two windows sharing a pid must produce two entries"

# --- unsupported class variants stay unresolved (no guessing) ---
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"class":"brave-maps.google.com__-Profile_2","initialClass":"brave-maps.google.com__-Profile_2","pid":1000,"floating":false,"workspace":{"id":8}},
  {"class":"chromium-maps.google.com__-Default","initialClass":"chromium-maps.google.com__-Default","pid":1001,"floating":false,"workspace":{"id":9}}
]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(wsrc 'brave-maps.google.com__-Profile_2')" == "cmdline" ]] \
  || fail "a non-Default profile must stay unresolved and keep the cmdline fallback"
[[ "$(wsrc 'chromium-maps.google.com__-Default')" == "cmdline" ]] \
  || fail "an unsupported browser prefix must stay unresolved"

# --- ambiguity: two surviving distinct ids, one identity ---
cat >"$WA_USER/Collide App.desktop" <<'EOF'
[Desktop Entry]
Name=Collide App
Exec=omarchy-launch-webapp https://example.com/foo_bar
Type=Application
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"brave-example.com__foo_bar-Default","initialClass":"brave-example.com__foo_bar-Default","pid":1001,"floating":false,"workspace":{"id":9}}]
EOF
out="$("$RELAUNCH" save --json)"
# /foo/bar and /foo_bar encode identically. Refuse to pick one.
[[ "$(wsrc 'brave-example.com__foo_bar-Default')" == "cmdline" ]] \
  || fail "an ambiguous identity must not resolve, got $(wsrc 'brave-example.com__foo_bar-Default')"
rm -f "$WA_USER/Collide App.desktop"

# --- XDG desktop-ID masking, web-app index only ---
# Same id in a lower-priority dir is masked by the user copy.
cat >"$WA_SYS/Google Maps.desktop" <<'EOF'
[Desktop Entry]
Name=Google Maps
Exec=omarchy-launch-webapp https://maps.google.com
Type=Application
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"brave-maps.google.com__-Default","initialClass":"brave-maps.google.com__-Default","pid":1000,"floating":false,"workspace":{"id":8}}]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(wex 'brave-maps.google.com__-Default')" == "gio launch $WA_USER/Google\ Maps.desktop" ]] \
  || fail "same desktop id in a system dir must be masked, not treated as ambiguous"

# A Hidden=true user override claims the id, so nothing resolves.
cat >"$WA_USER/Google Maps.desktop" <<'EOF'
[Desktop Entry]
Name=Google Maps
Exec=omarchy-launch-webapp https://maps.google.com
Hidden=true
Type=Application
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(wsrc 'brave-maps.google.com__-Default')" == "cmdline" ]] \
  || fail "a Hidden=true higher-priority entry must claim the id and block resolution"

# A non-web-app user override with the same id also claims it.
cat >"$WA_USER/Google Maps.desktop" <<'EOF'
[Desktop Entry]
Name=Google Maps
Exec=some-other-launcher
Type=Application
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(wsrc 'brave-maps.google.com__-Default')" == "cmdline" ]] \
  || fail "a non-web-app higher-priority entry must claim the id and block resolution"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# --- PR #12 review fixes, one block per finding ---

# (1) A synthesized --app-id must be shell-quoted like the inner argv: boot
# runs the whole string through bash -c, so a space would split it and a
# metacharacter would execute.
export RELAUNCH_CMDLINE_DIR="$WORKDIR/prcmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
printf 'foot\0--app-id=my app; touch /tmp/rl-appid-pwned\0-e\0htop\0' >"$RELAUNCH_CMDLINE_DIR/1200"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"my app; touch /tmp/rl-appid-pwned","initialClass":"my app; touch /tmp/rl-appid-pwned","pid":1200,"floating":false,"workspace":{"id":3}}]
EOF
saved="$("$RELAUNCH" save --json | jq -r '.entries[0].exec')"
mkdir -p "$WORKDIR/appidstub"
cat >"$WORKDIR/appidstub/xdg-terminal-exec" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@"
EOF
chmod +x "$WORKDIR/appidstub/xdg-terminal-exec"
mapfile -t got < <(PATH="$WORKDIR/appidstub:$PATH" bash -c "$saved" 2>/dev/null)
[[ "${got[0]}" == "--app-id=my app; touch /tmp/rl-appid-pwned" ]] \
  || fail "app id must survive as one argument, got ${got[0]}"
[[ ! -e /tmp/rl-appid-pwned ]] || fail "app id metacharacters executed at launch"
unset RELAUNCH_CMDLINE_DIR

# (2) Web-app identity follows GURL host()/path(): no port, no userinfo,
# dot segments removed. Ports especially: Chromium gives one class for both.
WB="$WORKDIR/gurl/applications"
mkdir -p "$WB"
printf '%s\n' '[Desktop Entry]' 'Name=Ported' 'Exec=omarchy-launch-webapp https://ported.example.com:8443/app' 'Type=Application' >"$WB/Ported.desktop"
printf '%s\n' '[Desktop Entry]' 'Name=Dotty' 'Exec=omarchy-launch-webapp https://dotty.example.com/a/c/../b' 'Type=Application' >"$WB/Dotty.desktop"
printf '%s\n' '[Desktop Entry]' 'Name=Userinfo' 'Exec=omarchy-launch-webapp https://joe:pw@userinfo.example.com/x' 'Type=Application' >"$WB/Userinfo.desktop"
export RELAUNCH_DATA_DIRS="$WORKDIR/gurl"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/gurlcmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0x1","class":"brave-ported.example.com__app-Default","initialClass":"brave-ported.example.com__app-Default","pid":1300,"floating":false,"workspace":{"id":3}},
  {"address":"0x2","class":"brave-dotty.example.com__a_b-Default","initialClass":"brave-dotty.example.com__a_b-Default","pid":1301,"floating":false,"workspace":{"id":4}},
  {"address":"0x3","class":"brave-userinfo.example.com__x-Default","initialClass":"brave-userinfo.example.com__x-Default","pid":1302,"floating":false,"workspace":{"id":5}}
]
EOF
out="$("$RELAUNCH" save --json)"
pex() { jq -r --arg c "$1" '[.entries[] | select(.class == $c) | .exec] | .[0] // "(none)"' <<<"$out"; }
[[ "$(pex 'brave-ported.example.com__app-Default')" == "gio launch $WB/Ported.desktop" ]] \
  || fail "a port must be excluded from identity, got $(pex 'brave-ported.example.com__app-Default')"
[[ "$(pex 'brave-dotty.example.com__a_b-Default')" == "gio launch $WB/Dotty.desktop" ]] \
  || fail "dot segments must be removed, got $(pex 'brave-dotty.example.com__a_b-Default')"
[[ "$(pex 'brave-userinfo.example.com__x-Default')" == "gio launch $WB/Userinfo.desktop" ]] \
  || fail "userinfo must be excluded, got $(pex 'brave-userinfo.example.com__x-Default')"
# Port and no-port share one Chromium class, so both launchers is ambiguous.
printf '%s\n' '[Desktop Entry]' 'Name=Unported' 'Exec=omarchy-launch-webapp https://ported.example.com/app' 'Type=Application' >"$WB/Unported.desktop"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"address":"0x1","class":"brave-ported.example.com__app-Default","initialClass":"brave-ported.example.com__app-Default","pid":1300,"floating":false,"workspace":{"id":3}}]
EOF
out="$("$RELAUNCH" save --json)"
# Falls back to whatever the normal chain yields (here guess, no /proc
# fixture); the point is that it must NOT pick one of the two launchers.
[[ "$(jq -r '.entries[0].execSource' <<<"$out")" != "desktop-file" ]] \
  || fail "port and no-port URLs collide in Chromium and must be ambiguous, got $(jq -r '.entries[0].exec' <<<"$out")"
rm -f "$WB/Unported.desktop"

# (3) Masking keys on the case-sensitive XDG id, not a lowercased basename.
printf '%s\n' '[Desktop Entry]' 'Name=Upper' 'Exec=omarchy-launch-webapp https://upper.example.com' 'Type=Application' >"$WB/Foo.desktop"
printf '%s\n' '[Desktop Entry]' 'Name=Lower' 'Exec=omarchy-launch-webapp https://lower.example.com' 'Type=Application' >"$WB/foo.desktop"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0x1","class":"brave-upper.example.com__-Default","initialClass":"brave-upper.example.com__-Default","pid":1400,"floating":false,"workspace":{"id":3}},
  {"address":"0x2","class":"brave-lower.example.com__-Default","initialClass":"brave-lower.example.com__-Default","pid":1401,"floating":false,"workspace":{"id":4}}
]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(pex 'brave-upper.example.com__-Default')" == "gio launch $WB/Foo.desktop" ]] \
  || fail "Foo.desktop and foo.desktop are distinct ids and must not mask each other"
[[ "$(pex 'brave-lower.example.com__-Default')" == "gio launch $WB/foo.desktop" ]] \
  || fail "case-distinct desktop ids must both resolve"
rm -f "$WB/Foo.desktop" "$WB/foo.desktop"
# A nested file claims the id dir-name, per XDG.
mkdir -p "$WB/sub"
printf '%s\n' '[Desktop Entry]' 'Name=Nested' 'Exec=omarchy-launch-webapp https://nested.example.com' 'Type=Application' >"$WB/sub/bar.desktop"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"address":"0x1","class":"brave-nested.example.com__-Default","initialClass":"brave-nested.example.com__-Default","pid":1500,"floating":false,"workspace":{"id":3}}]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(pex 'brave-nested.example.com__-Default')" == "gio launch $WB/sub/bar.desktop" ]] \
  || fail "a nested desktop file must still resolve"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# (6) Recapture must not relabel an overrides-table entry: its exec is the
# user's text and is protected, so the label must describe that, not the
# live hosted command.
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"herdr","workspace":1,"exec":"my-custom-herdr","execSource":"overrides-table","label":"My Custom Herdr","enabled":true}
]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"herdr","initialClass":"herdr","pid":90,"floating":false,"workspace":{"id":1}}]
EOF
"$RELAUNCH" save --json | jq -e '
  ([.entries[] | select(.class == "herdr") | .exec] == ["my-custom-herdr"])
  and ([.entries[] | select(.class == "herdr") | .label] == ["My Custom Herdr"])
' >/dev/null || fail "an overrides-table entry must keep both its exec and its label"
unset RELAUNCH_CMDLINE_DIR

# (4) import --class must find a hosted-terminal pid. Hyprland reports the
# window as class foot; list unwraps it to herdr, and the panel + button then
# calls import --class herdr. Without walking identity_from_pid that finds no
# pid and stores a bare guess instead of the xdg-terminal-exec wrap.
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg-empty"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
printf '{}\n' >"$RELAUNCH_CONFIG_DIR/overrides.json"
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"foot","initialClass":"foot","pid":90,"floating":false,"workspace":{"id":1}}]
EOF
"$RELAUNCH" import --class herdr --workspace 1 --json >/dev/null
jq -e '
  ([.entries[] | select(.class == "herdr") | .exec] == ["xdg-terminal-exec --app-id=herdr -e herdr"])
  and ([.entries[] | select(.class == "herdr") | .execSource] == ["terminal"])
' "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "import --class of a hosted terminal must find its pid and store the wrap"
jq -e '.herdr' "$RELAUNCH_CONFIG_DIR/overrides.json" >/dev/null \
  && fail "import --class must not write overrides.json for a hosted terminal"
"$RELAUNCH" drop --class herdr --json >/dev/null
unset RELAUNCH_CMDLINE_DIR
unset RELAUNCH_DATA_DIRS

# (5) A cheap startup class must still correlate with a saved entry, so the
# row is kind: both and keeps its Delete startup config action.
# o.launch_on_start("brave") inventories as class brave while the saved entry
# is brave-browser; the entry carries startup keys resolved at save time.
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
cat >"$RELAUNCH_AUTOSTART" <<'EOF'
o.launch_on_start("brave")
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"brave-browser","initialClass":"brave-browser","pid":1600,"floating":false,"workspace":{"id":2}}]
EOF
"$RELAUNCH" save >/dev/null
jq -e '[.entries[] | select(.class == "brave-browser") | .startupKeys] | .[0] | index("brave") != null' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "save must persist startup keys so list can correlate cheaply"
"$RELAUNCH" list --json | jq -e '
  ([.rows[] | select(.class == "brave-browser") | .kind] == ["both"])
  and ([.rows[] | select(.class == "brave-browser") | .startupId] | .[0] != null)
' >/dev/null || fail "a cheap startup class must correlate into kind: both"
# …and it must not also appear as its own NOT IN RELAUNCH row.
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.kind == "startup" and .exec == "brave")] | length == 0
' >/dev/null || fail "a correlated startup line must not be listed separately"
unset RELAUNCH_DATA_DIRS

# (7) uninstall must tear the snapshot unit down, and disable it BEFORE
# deleting the script and config dir, because disable --now fires ExecStop.
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
UNIT_DIR="$WORKDIR/xdgconf/systemd/user"
mkdir -p "$UNIT_DIR"
SYSTEMCTL_LOG="$WORKDIR/systemctl.log"
: >"$SYSTEMCTL_LOG"
cat >"$WORKDIR/systemctl" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >>"$SYSTEMCTL_LOG"
# disable --now fires ExecStop, which needs the script and config dir.
if [[ "\$*" == *"disable --now"* ]]; then
  [[ -d "$RELAUNCH_CONFIG_DIR" ]] || printf 'CONFIG_DIR_GONE\n' >>"$SYSTEMCTL_LOG"
fi
exit 0
EOF
chmod +x "$WORKDIR/systemctl"
printf 'placeholder\n' >"$UNIT_DIR/omarchy-relaunch-snapshot.service"
( export PATH="$WORKDIR:$PATH" XDG_CONFIG_HOME="$WORKDIR/xdgconf"
  "$RELAUNCH" uninstall --yes >/dev/null 2>&1 )
grep -q 'disable --now omarchy-relaunch-snapshot.service' "$SYSTEMCTL_LOG" \
  || fail "uninstall must disable the snapshot unit"
grep -q 'CONFIG_DIR_GONE' "$SYSTEMCTL_LOG" \
  && fail "uninstall disabled the unit after deleting the config dir"
[[ ! -e "$UNIT_DIR/omarchy-relaunch-snapshot.service" ]] \
  || fail "uninstall must remove the snapshot unit file"
mkdir -p "$RELAUNCH_CONFIG_DIR"

# (8) The hook can no longer be created: snapshot-hook is gone. It was
# withdrawn because it captured 1 of 11 windows on a real reboot and, more
# importantly, wrote window titles to disk on a schedule the user could not
# see or control. Only the teardown survives.
"$RELAUNCH" snapshot-hook --enable >/dev/null 2>&1 \
  && fail "snapshot-hook must no longer be a valid subcommand"
"$RELAUNCH" snapshot-hook --disable >/dev/null 2>&1 \
  && fail "snapshot-hook --disable must be gone too"
"$RELAUNCH" --help 2>&1 | grep -q 'snapshot-hook' \
  && fail "usage must not advertise snapshot-hook"
# Manual capture is explicitly fine and must keep working.
export FAKE_MONITORS="$WORKDIR/monitors.json"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"address":"0x1","class":"stillhere","initialClass":"stillhere","pid":5000,"floating":false,"monitor":0,"workspace":{"id":2},"title":"t"}]
EOF
"$RELAUNCH" snapshot >/dev/null || fail "relaunch snapshot must survive the hook removal"
jq -e '.windows | length == 1' "$RELAUNCH_CONFIG_DIR/last-session.json" >/dev/null \
  || fail "manual snapshot must still write last-session.json"
cat >"$RELAUNCH_CONFIG_DIR/last-boot.json" <<'EOF'
{"startedAt":"2026-01-01T00:00:00-00:00","outcome":"launched","launches":[]}
EOF
"$RELAUNCH" last-session --diff --json | jq -e '.classes | length == 1' >/dev/null \
  || fail "last-session --diff must survive the hook removal"
unset FAKE_MONITORS
# With no snapshot at all, the error must point at a command that exists.
rm -f "$RELAUNCH_CONFIG_DIR/last-session.json"
err="$("$RELAUNCH" last-session 2>&1 || true)"
assert_not_contains "$err" "snapshot-hook"
assert_contains "$err" "relaunch snapshot"

# (8b) ensure_hooks clears a unit left by an older install -- that user will
# never run uninstall -- but must not shell out when there is none, because
# ensure_hooks is on the hot path for list and save.
: >"$SYSTEMCTL_LOG"
cat >"$WORKDIR/systemctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$SYSTEMCTL_LOG_PATH"
exit 0
EOF
chmod +x "$WORKDIR/systemctl"
printf 'stale unit\n' >"$UNIT_DIR/omarchy-relaunch-snapshot.service"
( export PATH="$WORKDIR:$PATH" XDG_CONFIG_HOME="$WORKDIR/xdgconf" SYSTEMCTL_LOG_PATH="$SYSTEMCTL_LOG"
  "$RELAUNCH" list --json >/dev/null 2>&1 )
[[ ! -e "$UNIT_DIR/omarchy-relaunch-snapshot.service" ]] \
  || fail "ensure_hooks must delete a stray snapshot unit"
grep -q 'disable --now omarchy-relaunch-snapshot.service' "$SYSTEMCTL_LOG" \
  || fail "ensure_hooks must disable a stray snapshot unit, not just unlink it"
# No unit: no systemctl at all. This is the hot path.
: >"$SYSTEMCTL_LOG"
( export PATH="$WORKDIR:$PATH" XDG_CONFIG_HOME="$WORKDIR/xdgconf" SYSTEMCTL_LOG_PATH="$SYSTEMCTL_LOG"
  "$RELAUNCH" list --json >/dev/null 2>&1 )
[[ ! -s "$SYSTEMCTL_LOG" ]] \
  || fail "ensure_hooks must not fork systemctl when no unit exists: $(cat "$SYSTEMCTL_LOG")"

# (9) last-session --diff must compare workspace and float, not just counts.
export FAKE_MONITORS="$WORKDIR/monitors.json"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0x1","class":"moved","initialClass":"moved","pid":1700,"floating":false,"monitor":0,"workspace":{"id":3},"title":"m"},
  {"address":"0x2","class":"refloated","initialClass":"refloated","pid":1701,"floating":false,"monitor":0,"workspace":{"id":4},"title":"r"},
  {"address":"0x3","class":"stayed","initialClass":"stayed","pid":1702,"floating":false,"monitor":0,"workspace":{"id":5},"title":"s"}
]
EOF
"$RELAUNCH" snapshot >/dev/null
cat >"$RELAUNCH_CONFIG_DIR/last-boot.json" <<'EOF'
{"startedAt":"2026-01-01T00:00:00-00:00","outcome":"launched","launches":[]}
EOF
# Same classes, same counts: one moved workspace, one changed float state.
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0x1","class":"moved","initialClass":"moved","pid":1800,"floating":false,"monitor":0,"workspace":{"id":9},"title":"m"},
  {"address":"0x2","class":"refloated","initialClass":"refloated","pid":1801,"floating":true,"monitor":0,"workspace":{"id":4},"title":"r"},
  {"address":"0x3","class":"stayed","initialClass":"stayed","pid":1802,"floating":false,"monitor":0,"workspace":{"id":5},"title":"s"}
]
EOF
report="$("$RELAUNCH" last-session --diff --json)"
echo "$report" | jq -e '[.classes[] | select(.class == "moved") | .mismatched] == [1]' >/dev/null \
  || fail "a window back on the wrong workspace must be reported: $report"
echo "$report" | jq -e '[.classes[] | select(.class == "refloated") | .mismatched] == [1]' >/dev/null \
  || fail "a window back with the wrong float state must be reported: $report"
echo "$report" | jq -e '[.classes[] | select(.class == "stayed") | .mismatched] == [0]' >/dev/null \
  || fail "an intact window must not be reported as mismatched: $report"
echo "$report" | jq -e '[.classes[] | select(.class == "moved") | .missing] == [0]' >/dev/null \
  || fail "a moved window is not a missing window: $report"
"$RELAUNCH" last-session --diff | grep -q "MOVED" \
  || fail "the human diff must show a MOVED row"
unset FAKE_MONITORS

# --- RFC 3986 5.2.4 and GURL edge cases (PR #12 re-review) ---
# A segment-stack approximation drops empty segments and the trailing slash a
# final "." or ".." produces. Those are different Chromium identities, so each
# case gets a launcher and a window and must resolve to its own file.
RFC="$WORKDIR/rfc/applications"
mkdir -p "$RFC"
mkrfc() { printf '%s\n' '[Desktop Entry]' "Name=$1" "Exec=omarchy-launch-webapp $2" 'Type=Application' >"$RFC/$1.desktop"; }
# /a/b/..  -> /a/   -> identity host__a_
mkrfc "TrailA" "https://trail-a.example.com/a/b/.."
# /a/b/../ -> /a/   (same shape, distinct host so both can be asserted)
mkrfc "TrailB" "https://trail-b.example.com/a/b/../"
# /a//b    -> /a//b -> empty segment preserved
mkrfc "Empty" "https://empty.example.com/a//b"
# /a/.     -> /a/
mkrfc "Dot" "https://dot.example.com/a/."
# bracketed IPv6 host, which had no regression test at all
mkrfc "Six" "https://[2001:db8::1]:8443/x/../y"
export RELAUNCH_DATA_DIRS="$WORKDIR/rfc"
export RELAUNCH_CMDLINE_DIR="$WORKDIR/rfccmd"
mkdir -p "$RELAUNCH_CMDLINE_DIR"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[
  {"address":"0x1","class":"brave-trail-a.example.com__a_-Default","initialClass":"brave-trail-a.example.com__a_-Default","pid":2000,"floating":false,"workspace":{"id":1}},
  {"address":"0x2","class":"brave-trail-b.example.com__a_-Default","initialClass":"brave-trail-b.example.com__a_-Default","pid":2001,"floating":false,"workspace":{"id":2}},
  {"address":"0x3","class":"brave-empty.example.com__a__b-Default","initialClass":"brave-empty.example.com__a__b-Default","pid":2002,"floating":false,"workspace":{"id":3}},
  {"address":"0x4","class":"brave-dot.example.com__a_-Default","initialClass":"brave-dot.example.com__a_-Default","pid":2003,"floating":false,"workspace":{"id":4}},
  {"address":"0x5","class":"brave-[2001:db8::1]__y-Default","initialClass":"brave-[2001:db8::1]__y-Default","pid":2004,"floating":false,"workspace":{"id":5}}
]
EOF
out="$("$RELAUNCH" save --json)"
rex() { jq -r --arg c "$1" '[.entries[] | select(.class == $c) | .exec] | .[0] // "(none)"' <<<"$out"; }
[[ "$(rex 'brave-trail-a.example.com__a_-Default')" == "gio launch $RFC/TrailA.desktop" ]] \
  || fail "/a/b/.. must canonicalize to /a/ (trailing slash kept), got $(rex 'brave-trail-a.example.com__a_-Default')"
[[ "$(rex 'brave-trail-b.example.com__a_-Default')" == "gio launch $RFC/TrailB.desktop" ]] \
  || fail "/a/b/../ must canonicalize to /a/, got $(rex 'brave-trail-b.example.com__a_-Default')"
[[ "$(rex 'brave-empty.example.com__a__b-Default')" == "gio launch $RFC/Empty.desktop" ]] \
  || fail "/a//b must keep its empty segment, got $(rex 'brave-empty.example.com__a__b-Default')"
[[ "$(rex 'brave-dot.example.com__a_-Default')" == "gio launch $RFC/Dot.desktop" ]] \
  || fail "/a/. must canonicalize to /a/, got $(rex 'brave-dot.example.com__a_-Default')"
[[ "$(rex 'brave-[2001:db8::1]__y-Default')" == "gio launch $RFC/Six.desktop" ]] \
  || fail "a bracketed IPv6 host must keep its brackets and drop the port, got $(rex 'brave-[2001:db8::1]__y-Default')"

# A scheme-less URL is not a GURL: reject it instead of indexing a bogus
# identity. The guard used to be unreachable and accepted it.
mkrfc "Schemeless" "example.com/app"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"address":"0x9","class":"brave-example.com__app-Default","initialClass":"brave-example.com__app-Default","pid":2100,"floating":false,"workspace":{"id":7}}]
EOF
out="$("$RELAUNCH" save --json)"
[[ "$(jq -r '.entries[0].execSource' <<<"$out")" != "desktop-file" ]] \
  || fail "a scheme-less Exec URL must not produce a web-app identity, got $(rex 'brave-example.com__app-Default')"
rm -f "$RFC/Schemeless.desktop"
unset RELAUNCH_DATA_DIRS
unset RELAUNCH_CMDLINE_DIR

# --- an overrides-table entry keeps exec, label AND startupKeys ---
export RELAUNCH_CMDLINE_DIR="$WORKDIR/cmdlines"
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"ignored":[],"entries":[
  {"class":"herdr","workspace":1,"exec":"my-custom-herdr","execSource":"overrides-table",
   "label":"My Custom Herdr","startupKeys":["herdr","my-custom-herdr"],"enabled":true}
]}
EOF
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"herdr","initialClass":"herdr","pid":90,"floating":false,"workspace":{"id":1}}]
EOF
"$RELAUNCH" save --json | jq -e '
  [.entries[] | select(.class == "herdr")] | .[0]
  | .exec == "my-custom-herdr"
    and .label == "My Custom Herdr"
    and (.startupKeys | index("my-custom-herdr")) != null
' >/dev/null || fail "an overrides-table entry must keep exec, label and startupKeys"
unset RELAUNCH_CMDLINE_DIR

# --- a config written before startupKeys existed upgrades cleanly ---
# Not an empty config: a real pre-schema entry, exercising load/list/generate/
# boot before any Save, then the Save that fills the keys in.
export RELAUNCH_DATA_DIRS="$WORKDIR/xdg"
cat >"$RELAUNCH_AUTOSTART" <<'EOF'
o.launch_on_start("brave")
EOF
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{
  "staggerSeconds": 0,
  "ignored": [],
  "skipOnce": false,
  "entries": [
    {"class":"brave-browser","workspace":2,
     "exec":"gio launch /tmp/does-not-matter/brave-browser.desktop",
     "execSource":"desktop-file","enabled":true}
  ]
}
EOF
legacy="$("$RELAUNCH" list --json)"
echo "$legacy" | jq -e '
  ([.entries[] | select(.class == "brave-browser") | .startupKeys] == [[]])
' >/dev/null || fail "a pre-startupKeys config must normalize the field to []: $legacy"
echo "$legacy" | jq -e '.ok == true' >/dev/null || fail "a legacy config must list cleanly"
"$RELAUNCH" generate >/dev/null || fail "a legacy config must generate cleanly"
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'class = "^(brave-browser)$"'
assert_not_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'startupKeys'
echo '[]' >"$FAKE_CLIENTS"
"$RELAUNCH" boot >/dev/null 2>&1
jq -e '.outcome == "launched"' "$RELAUNCH_CONFIG_DIR/last-boot.json" >/dev/null \
  || fail "a legacy config must boot cleanly"
# Before a Save the association is simply absent -- same as before the field.
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.class == "brave-browser") | .kind] == ["relaunch"]
' >/dev/null || fail "a legacy entry correlates only after a Save"
# The Save fills them in, and the association appears.
cat >"$FAKE_CLIENTS" <<'EOF'
[{"class":"brave-browser","initialClass":"brave-browser","pid":2200,"floating":false,"workspace":{"id":2}}]
EOF
"$RELAUNCH" save >/dev/null
jq -e '[.entries[] | select(.class == "brave-browser") | .startupKeys] | .[0] | index("brave") != null' \
  "$RELAUNCH_CONFIG_DIR/config.json" >/dev/null \
  || fail "a Save must populate startupKeys on a legacy entry"
"$RELAUNCH" list --json | jq -e '
  [.rows[] | select(.class == "brave-browser") | .kind] == ["both"]
' >/dev/null || fail "after the upgrade Save the legacy entry must correlate"
unset RELAUNCH_DATA_DIRS

# --- startup exec with glob chars stays literal ---
mkdir -p "$WORKDIR/globdir"
printf '' >"$WORKDIR/globdir/not-the-class"
cat >"$RELAUNCH_AUTOSTART" <<'EOF'
o.launch_on_start("*")
-- omarchy-relaunch (managed; hidden from the Relaunch list)
o.exec_on_start("/tmp/relaunch boot")
EOF
(
  cd "$WORKDIR/globdir"
  "$RELAUNCH" list --json
) | jq -e '
  [.startup[] | select(.exec == "*") | .class] == ["*"]
' >/dev/null || fail "startup exec glob chars must not expand against CWD"

# --- stagger waits between launches, not before the first ---
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{
  "staggerSeconds": 5,
  "ignored": [],
  "entries": [
    {"class": "a", "workspace": 1, "exec": "true", "enabled": true},
    {"class": "b", "workspace": 2, "exec": "true", "enabled": true},
    {"class": "c", "workspace": 3, "exec": "true", "enabled": true}
  ]
}
EOF
SLEEP_LOG="$WORKDIR/sleeps"
: >"$SLEEP_LOG"
cat >"$WORKDIR/bin/record-sleep" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$1" >>"$SLEEP_LOG"
EOF
chmod +x "$WORKDIR/bin/record-sleep"
RELAUNCH_SLEEP="$WORKDIR/bin/record-sleep" "$RELAUNCH" boot
[[ "$(wc -l <"$SLEEP_LOG")" -eq 2 ]] || fail "stagger should sleep between launches only, got $(cat "$SLEEP_LOG")"
[[ "$(tr '\n' ' ' <"$SLEEP_LOG")" == "5 5 " ]] || fail "stagger sleep args: $(cat "$SLEEP_LOG")"

# --- class backslash is escaped for Lua ---
cat >"$RELAUNCH_CONFIG_DIR/config.json" <<'EOF'
{"staggerSeconds":0,"entries":[{"class":"foo\\bar","workspace":1,"exec":"foo","enabled":true}]}
EOF
"$RELAUNCH" generate >/dev/null
assert_contains "$(cat "$RELAUNCH_CONFIG_DIR/relaunch.lua")" 'o.window({ class = "^(foo\\\\bar)$"'

# --- env-prefixed exec is not treated as the class ---
cat >"$RELAUNCH_AUTOSTART" <<'EOF'
o.exec_on_start("FOO=bar signal-desktop")
o.launch_on_start("keep-me")
-- omarchy-relaunch (managed; hidden from the Relaunch list)
o.exec_on_start("/tmp/relaunch boot")
o.launch_on_start("also-keep")
EOF
"$RELAUNCH" list --json | jq -e '
  [.startup[] | select(.exec == "FOO=bar signal-desktop") | .class] | length == 1
    and .[0] != "FOO=bar" and .[0] != ""
' >/dev/null || fail "env-prefixed exec class"

# --- uninstall must not delete adjacent user startup lines ---
printf '%s\n' '-- keep' '-- omarchy-relaunch' 'local _rl = "/tmp/omarchy-relaunch/relaunch.lua"; dofile(_rl)' '-- after' >"$RELAUNCH_HYPRLAND_LUA"
"$RELAUNCH" uninstall --yes >/dev/null
grep -q 'o.launch_on_start("keep-me")' "$RELAUNCH_AUTOSTART" || fail "unrelated keep-me deleted"
grep -q 'o.launch_on_start("also-keep")' "$RELAUNCH_AUTOSTART" || fail "adjacent also-keep deleted"
grep -q 'relaunch boot' "$RELAUNCH_AUTOSTART" && fail "boot hook survived uninstall"
grep -q 'omarchy-relaunch' "$RELAUNCH_AUTOSTART" && fail "marker survived uninstall"
grep -q 'omarchy-relaunch' "$RELAUNCH_HYPRLAND_LUA" && fail "hyprland marker survived uninstall"
grep -q 'dofile' "$RELAUNCH_HYPRLAND_LUA" && fail "hyprland dofile survived uninstall"
grep -q -- '-- keep' "$RELAUNCH_HYPRLAND_LUA" || fail "unrelated hyprland comment deleted"
grep -q -- '-- after' "$RELAUNCH_HYPRLAND_LUA" || fail "adjacent hyprland comment deleted"

grep -q 'install -m 0755 "$REPO_DIR/relaunch"      "$PLUGIN_DST/relaunch"' \
  "$ROOT/install.sh" || fail "install.sh must copy relaunch into the plugin folder"

printf 'ok %s\n' "$(basename "$0")"
