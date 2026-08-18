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
[[ "$(grep -c 'class = "\^(foot)\$"' <<<"$lua")" -eq 1 ]]   || fail "snapshot must not add pins; expected exactly one foot rule"
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
