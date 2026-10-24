#!/usr/bin/env bash
# Build the differential fixture world into $1 (deterministic; no network, no
# live state). Both engines run against identical copies of it.
#
# The desktop index gets the heaviest coverage because it is the most tuned
# code being ported: every lookup tier, first-wins ordering across two data
# dirs, a symlinked entry, Hidden masking (web-app map only), CRLF + BOM, a
# terminal-wrapping Exec (leaf -> Name map), DBusActivatable, two web apps that
# share an identity (ambiguity), and a user override that masks a system id.
set -euo pipefail
W="${1:?world dir}"
rm -rf "$W"
mkdir -p "$W"/{data1/applications,data2/applications,cmd,cfg,hypr}

desk() { # desk <dir> <file> <lines...>
  local d="$1" f="$2"; shift 2
  printf '%s\n' "[Desktop Entry]" "Type=Application" "$@" >"$W/$d/applications/$f"
}

# --- data1: user-level dir, searched first (first wins per key) ---
desk data1 myapp.desktop "Name=My App" "StartupWMClass=MyApp" "Exec=myapp --flag %U"
desk data1 idonly.desktop "Name=Id Only" "Exec=idonly"
desk data1 nameonly-xyz.desktop "Name=Name Only" "Exec=nameonly"
desk data1 dbusapp.desktop "Name=Dbus App" "StartupWMClass=DbusApp" "Exec=dbusapp" "DBusActivatable=true"
desk data1 disk.desktop "Name=Disk Usage" \
  'Exec=xdg-terminal-exec --app-id=TUI.float -e bash -c "dua i /"'
# Spec forms bash's line matcher misses: spaces around `=` ("should be
# ignored"), and Hidden spelled other than the exact key `Hidden` and boolean
# `true` (keys and booleans are case-sensitive).
desk data1 spaced.desktop "Name = Spaced App" "StartupWMClass = SpacedApp" "Exec = spacedapp"
desk data1 hiddencase.desktop "Name=Hidden Case" "Hidden=True" \
  "Exec=omarchy-launch-webapp https://hiddencase.example.com/"
desk data1 hiddenkey.desktop "Name=Hidden Key" "hidden=true" \
  "Exec=omarchy-launch-webapp https://hiddenkey.example.com/"
# A user override masking a system entry's desktop id (web-app index only).
desk data1 shadowed.desktop "Name=Shadowed Override" "Hidden=true"
# A Hidden=true override that still carries a web-app Exec: it must claim the
# id (masking the system entry below) WITHOUT registering the identity.
desk data1 hiddenweb.desktop "Name=Hidden Web" "Hidden=true" \
  "Exec=omarchy-launch-webapp https://hidden.example.com/"
# CRLF line endings and a UTF-8 BOM on the group header.
printf '\xef\xbb\xbf[Desktop Entry]\r\nType=Application\r\nName=Crlf App\r\nStartupWMClass=CrlfApp\r\nExec=crlfapp\r\n' \
  >"$W/data1/applications/crlf.desktop"
# Web apps (Omarchy launcher shape): one unique, two sharing an identity.
desk data1 brave-maps.desktop "Name=Maps" \
  "Exec=omarchy-launch-webapp https://maps.example.com/app"
desk data1 web-a.desktop "Name=Web A" "Exec=omarchy-launch-webapp https://dup.example.com/"
desk data1 web-b.desktop "Name=Web B" "Exec=omarchy-launch-webapp https://dup.example.com/"

# --- data2: system-level dir, searched second ---
desk data2 myapp-system.desktop "Name=My App System" "StartupWMClass=MyApp" "Exec=myapp-system"
desk data2 shadowed.desktop "Name=Shadowed" "Exec=omarchy-launch-webapp https://shadow.example.com/"
desk data2 hiddenweb.desktop "Name=Hidden Web System" "Exec=omarchy-launch-webapp https://hidden.example.com/"
desk data2 real-target.desktop "Name=Linked App" "StartupWMClass=LinkedApp" "Exec=linkedapp"
mkdir -p "$W/data2/applications/sub"
desk data2 sub/nested.desktop "Name=Nested" "StartupWMClass=Nested" "Exec=nested"
# A symlinked entry (LibreOffice ships these; find -type f alone skips them).
ln -s real-target.desktop "$W/data2/applications/linked.desktop"

# --- the sandbox HOME's own data dir (XDG_DATA_HOME default) ---
mkdir -p "$W/home/.local/share/applications"
desk home/.local/share homeapp.desktop "Name=Home App" "StartupWMClass=HomeApp" "Exec=homeapp"

# --- windows (hyprctl clients -j shape) ---
cat >"$W/clients.json" <<'EOF'
[
  {"address":"0x01","pid":101,"class":"MyApp","initialClass":"MyApp","workspace":{"id":1},"floating":false,"xwayland":false},
  {"address":"0x02","pid":102,"class":"foot","initialClass":"herdr","workspace":{"id":2},"floating":true,"xwayland":false},
  {"address":"0x03","pid":103,"class":"DbusApp","initialClass":"DbusApp","workspace":{"id":3},"floating":false,"xwayland":false},
  {"address":"0x04","pid":104,"class":"Unknown.Thing","initialClass":"Unknown.Thing","workspace":{"id":4},"floating":false,"xwayland":false},
  {"address":"0x05","pid":105,"class":"Sandboxed","initialClass":"Sandboxed","workspace":{"id":5},"floating":false,"xwayland":false},
  {"address":"0x06","pid":106,"class":"OldX","initialClass":"OldX","workspace":{"id":6},"floating":false,"xwayland":true},
  {"address":"0x07","pid":107,"class":"brave-maps.example.com__app-Default","initialClass":"brave-maps.example.com__app-Default","workspace":{"id":7},"floating":false,"xwayland":false},
  {"address":"0x08","pid":107,"class":"brave-dup.example.com__-Default","initialClass":"brave-dup.example.com__-Default","workspace":{"id":7},"floating":false,"xwayland":false},
  {"address":"0x09","pid":109,"class":"My Spaced App","initialClass":"My Spaced App","workspace":{"id":8},"floating":false,"xwayland":false},
  {"address":"0x0a","pid":110,"class":"Back\\slash","initialClass":"Back\\slash","workspace":{"id":8},"floating":false,"xwayland":false},
  {"address":"0x0b","pid":111,"class":"LinkedApp","initialClass":"LinkedApp","workspace":{"id":9},"floating":false,"xwayland":false},
  {"address":"0x0c","pid":112,"class":"CrlfApp","initialClass":"CrlfApp","workspace":{"id":1},"floating":false,"xwayland":false},
  {"address":"0x0d","pid":113,"class":"__proto__","initialClass":"__proto__","workspace":{"id":2},"floating":false,"xwayland":false},
  {"address":"0x0e","pid":114,"class":"Scratch","initialClass":"Scratch","workspace":{"id":-98},"floating":true,"xwayland":false}
]
EOF

# --- per-pid argv, parents, namespaces (RELAUNCH_CMDLINE_DIR) ---
argv() { local pid="$1"; shift; printf '%s\0' "$@" >"$W/cmd/$pid"; }
argv 101 myapp --flag
argv 102 herdr;   printf '202\n' >"$W/cmd/102.ppid"
argv 202 foot --app-id=herdr -e herdr
argv 103 /usr/bin/dbusapp --gapplication-service
argv 104 /opt/thing/bin/thing --profile 'a b'
argv 105 sandboxed; printf 'foreign\n' >"$W/cmd/105.ns"
argv 106 oldx
argv 107 /usr/lib/brave/brave --type=renderer; printf '207\n' >"$W/cmd/107.ppid"
argv 207 /usr/lib/brave/brave --app=https://maps.example.com/app
# 109/110 have no argv: they fall through to guess (Sol #2 shape).
argv 111 linkedapp
argv 112 crlfapp
argv 113 proto-bin
# Sol #1 shape: a host HELPER process (so the cmdline walk climbs past it)
# whose parent is sandboxed. The parent's argv must never be adopted as host.
argv 114 /usr/lib/scratch/scratch --type=renderer; printf '214\n' >"$W/cmd/114.ppid"
argv 214 /usr/lib/scratch/scratch --sandboxed-secret-profile; printf 'foreign\n' >"$W/cmd/214.ns"

# --- startup inventory + legacy config + overrides ---
cat >"$W/hypr/autostart.lua" <<'EOF'
-- autostart
o.launch_on_start("myapp --flag")
o.exec_on_start("uwsm-app -- idonly")
o.launch_on_start("xdg-terminal-exec --app-id=TUI.float -e bash -c 'dua i /'")
EOF
cat >"$W/cfg/config.json" <<'EOF'
{"staggerSeconds":2,"entries":[
 {"class":"MyApp","workspace":1,"exec":"myapp --flag","execSource":"desktop-file","enabled":true,"label":"My App","float":false},
 {"class":"herdr","workspace":2,"exec":"xdg-terminal-exec --app-id=herdr -e herdr","execSource":"terminal","enabled":false,"label":"herdr"},
 {"class":"Custom","workspace":3,"exec":"custom --typed","execSource":"overrides-table","enabled":true,"label":"Typed Label"}
]}
EOF
printf '{"Custom":"custom --typed"}\n' >"$W/cfg/overrides.json"
printf '%s\n' '-- keep this line' >"$W/hypr/hyprland.lua"

# --- session diagnostics: the anchor's boot record and a layout snapshot ---
# The snapshot is built to hit every last-session --diff verdict against the
# windows above: ok (herdr), MISSING (a second MyApp; Gone), MOVED by
# workspace (CrlfApp) and by float state (DbusApp).
cat >"$W/cfg/last-boot.json" <<'EOF'
{"startedAt":"2026-10-20T08:00:00+01:00","outcome":"launched","stagger":2,"launches":[{"class":"MyApp","workspace":1,"state":"landed"}]}
EOF
printf 'boot 2026-10-20T08:00:00+01:00\nMyApp ws1 landed\n\n\n' >"$W/cfg/last-boot.log"
cat >"$W/cfg/last-session.json" <<'EOF'
{
  "capturedAt": "2026-10-19T22:00:00+01:00",
  "windows": [
    {"class": "MyApp", "label": "My App", "workspace": 1, "floating": false},
    {"class": "MyApp", "label": "My App", "workspace": 4, "floating": false},
    {"class": "herdr", "label": "herdr", "workspace": 2, "floating": true},
    {"class": "CrlfApp", "label": "Crlf", "workspace": 5, "floating": false},
    {"class": "DbusApp", "label": "Dbus", "workspace": 3, "floating": true},
    {"class": "Gone", "label": "Gone App", "workspace": 6, "floating": false}
  ]
}
EOF

# --- a signed configuration (structure only; nothing here verifies it) ---
mkdir -p "$W/cfg-signed"
sig="$(printf '0%.0s' {1..128})"
cat >"$W/cfg-signed/config.json" <<EOF
{"v":2,"signed":{"v":1,"uid":1000,"user":"u","gen":3,"staggerSeconds":1,"entries":[
 {"class":"MyApp","workspace":1,"float":false,"label":"My App","exec":"myapp --flag","execSource":"desktop-file","provenance":"none","launcher":{"path":"/usr/share/applications/myapp.desktop"},"enabled":true},
 {"class":"Typed","workspace":4,"label":"","exec":"typed --own","execSource":"overrides-table","provenance":"none","enabled":false}
]},"signature":"$sig"}
EOF
: >"$W/cfg-signed/skip-once"

# --- an overrides table that is not a JSON object ---
mkdir -p "$W/cfg-badover"
printf '[1,2]\n' >"$W/cfg-badover/overrides.json"

# --- the suite's resolution sections, as differential scenarios (M8) ---
# One directory per scenario: s/<name>/d1 and d2 (data dirs, d1 searched
# first) and s/<name>/cmd (argv fixtures). The cases are `suite:` in
# tests/differential.sh; the bash engine's answers are their goldens.
scn() { # scn <scenario> <d1|d2> <file> <crlf:0|1> <lines...>
  local s="$1" d="$2" f="$3" crlf="$4" sep='\n'; shift 4
  [[ "$crlf" == 1 ]] && sep='\r\n'
  mkdir -p "$(dirname "$W/s/$s/$d/applications/$f")" "$W/s/$s/cmd"
  printf "%s$sep" "$@" >"$W/s/$s/$d/applications/$f"
}
# 74 PWA Name= must not steal the official class
mkdir -p "$W/s/pwa/d1/applications" "$W/s/pwa/d2/applications" "$W/s/pwa/cmd"
scn pwa d1 'proton-mail.desktop' 0 '[Desktop Entry]' 'Name=Proton Mail' 'Exec=proton-mail %U' 'Type=Application'
scn pwa d1 'brave-jnpecgipniidlgicjocehkhajgdnjekh-Default.desktop' 0 '[Desktop Entry]' 'Name=Proton Mail' 'Exec=/opt/brave-bin/brave --app-id=jnpecgipniidlgicjocehkhajgdnjekh' 'StartupWMClass=crx_jnpecgipniidlgicjocehkhajgdnjekh' 'Type=Application'
# 1137 web-app windows resolve to their launchers; 1242 shared pid; 1264 unsupported variants
mkdir -p "$W/s/wa/d1/applications" "$W/s/wa/d2/applications" "$W/s/wa/cmd"
scn wa d1 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
scn wa d1 'Deep App.desktop' 0 '[Desktop Entry]' 'Name=Deep App' 'Exec=omarchy-launch-webapp https://example.com/foo/bar' 'Type=Application'
scn wa d1 'Query App.desktop' 0 '[Desktop Entry]' 'Name=Query App' 'Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2' 'Type=Application'
scn wa d1 'Hyphen App.desktop' 0 '[Desktop Entry]' 'Name=Hyphen App' 'Exec=omarchy-launch-webapp https://my-site.com/a-b' 'Type=Application'
scn wa d2 'brave-browser.desktop' 0 '[Desktop Entry]' 'Name=Brave Web Browser' 'Exec=brave %U' 'StartupWMClass=brave-browser' 'Type=Application'
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/wa/cmd/1000"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/wa/cmd/1001"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/wa/cmd/1002"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/wa/cmd/1003"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/wa/cmd/1004"
# 1280 two surviving ids, one identity: ambiguous
mkdir -p "$W/s/collide/d1/applications" "$W/s/collide/d2/applications" "$W/s/collide/cmd"
scn collide d1 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
scn collide d1 'Deep App.desktop' 0 '[Desktop Entry]' 'Name=Deep App' 'Exec=omarchy-launch-webapp https://example.com/foo/bar' 'Type=Application'
scn collide d1 'Query App.desktop' 0 '[Desktop Entry]' 'Name=Query App' 'Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2' 'Type=Application'
scn collide d1 'Hyphen App.desktop' 0 '[Desktop Entry]' 'Name=Hyphen App' 'Exec=omarchy-launch-webapp https://my-site.com/a-b' 'Type=Application'
scn collide d1 'Collide App.desktop' 0 '[Desktop Entry]' 'Name=Collide App' 'Exec=omarchy-launch-webapp https://example.com/foo_bar' 'Type=Application'
scn collide d2 'brave-browser.desktop' 0 '[Desktop Entry]' 'Name=Brave Web Browser' 'Exec=brave %U' 'StartupWMClass=brave-browser' 'Type=Application'
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/collide/cmd/1000"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/collide/cmd/1001"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/collide/cmd/1002"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/collide/cmd/1003"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/collide/cmd/1004"
# 1299 the same id in a system dir is masked, not ambiguous
mkdir -p "$W/s/mask-sys/d1/applications" "$W/s/mask-sys/d2/applications" "$W/s/mask-sys/cmd"
scn mask-sys d1 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
scn mask-sys d1 'Deep App.desktop' 0 '[Desktop Entry]' 'Name=Deep App' 'Exec=omarchy-launch-webapp https://example.com/foo/bar' 'Type=Application'
scn mask-sys d1 'Query App.desktop' 0 '[Desktop Entry]' 'Name=Query App' 'Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2' 'Type=Application'
scn mask-sys d1 'Hyphen App.desktop' 0 '[Desktop Entry]' 'Name=Hyphen App' 'Exec=omarchy-launch-webapp https://my-site.com/a-b' 'Type=Application'
scn mask-sys d2 'brave-browser.desktop' 0 '[Desktop Entry]' 'Name=Brave Web Browser' 'Exec=brave %U' 'StartupWMClass=brave-browser' 'Type=Application'
scn mask-sys d2 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-sys/cmd/1000"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-sys/cmd/1001"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-sys/cmd/1002"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-sys/cmd/1003"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-sys/cmd/1004"
# 1299 a Hidden=true higher-priority entry claims the id
mkdir -p "$W/s/mask-hidden/d1/applications" "$W/s/mask-hidden/d2/applications" "$W/s/mask-hidden/cmd"
scn mask-hidden d1 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Hidden=true' 'Type=Application'
scn mask-hidden d1 'Deep App.desktop' 0 '[Desktop Entry]' 'Name=Deep App' 'Exec=omarchy-launch-webapp https://example.com/foo/bar' 'Type=Application'
scn mask-hidden d1 'Query App.desktop' 0 '[Desktop Entry]' 'Name=Query App' 'Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2' 'Type=Application'
scn mask-hidden d1 'Hyphen App.desktop' 0 '[Desktop Entry]' 'Name=Hyphen App' 'Exec=omarchy-launch-webapp https://my-site.com/a-b' 'Type=Application'
scn mask-hidden d2 'brave-browser.desktop' 0 '[Desktop Entry]' 'Name=Brave Web Browser' 'Exec=brave %U' 'StartupWMClass=brave-browser' 'Type=Application'
scn mask-hidden d2 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-hidden/cmd/1000"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-hidden/cmd/1001"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-hidden/cmd/1002"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-hidden/cmd/1003"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-hidden/cmd/1004"
# 1299 a non-web-app higher-priority entry claims the id
mkdir -p "$W/s/mask-other/d1/applications" "$W/s/mask-other/d2/applications" "$W/s/mask-other/cmd"
scn mask-other d1 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=some-other-launcher' 'Type=Application'
scn mask-other d1 'Deep App.desktop' 0 '[Desktop Entry]' 'Name=Deep App' 'Exec=omarchy-launch-webapp https://example.com/foo/bar' 'Type=Application'
scn mask-other d1 'Query App.desktop' 0 '[Desktop Entry]' 'Name=Query App' 'Exec=omarchy-launch-webapp https://queried.example.com/?view=a&x=2' 'Type=Application'
scn mask-other d1 'Hyphen App.desktop' 0 '[Desktop Entry]' 'Name=Hyphen App' 'Exec=omarchy-launch-webapp https://my-site.com/a-b' 'Type=Application'
scn mask-other d2 'brave-browser.desktop' 0 '[Desktop Entry]' 'Name=Brave Web Browser' 'Exec=brave %U' 'StartupWMClass=brave-browser' 'Type=Application'
scn mask-other d2 'Google Maps.desktop' 0 '[Desktop Entry]' 'Name=Google Maps' 'Exec=omarchy-launch-webapp https://maps.google.com' 'Type=Application'
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-other/cmd/1000"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-other/cmd/1001"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-other/cmd/1002"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-other/cmd/1003"
printf '/opt/brave-bin/brave\0--ozone-platform=wayland\0' >"$W/s/mask-other/cmd/1004"
# 1348 an app id with shell metacharacters stays one argument
mkdir -p "$W/s/appid/d1/applications" "$W/s/appid/d2/applications" "$W/s/appid/cmd"
printf 'foot\0--app-id=my app; touch /tmp/rl-appid-pwned\0-e\0htop\0' >"$W/s/appid/cmd/1200"
# 1348 port, dot segments and userinfo are excluded from identity
mkdir -p "$W/s/gurl/d1/applications" "$W/s/gurl/d2/applications" "$W/s/gurl/cmd"
scn gurl d1 'Ported.desktop' 0 '[Desktop Entry]' 'Name=Ported' 'Exec=omarchy-launch-webapp https://ported.example.com:8443/app' 'Type=Application'
scn gurl d1 'Dotty.desktop' 0 '[Desktop Entry]' 'Name=Dotty' 'Exec=omarchy-launch-webapp https://dotty.example.com/a/c/../b' 'Type=Application'
scn gurl d1 'Userinfo.desktop' 0 '[Desktop Entry]' 'Name=Userinfo' 'Exec=omarchy-launch-webapp https://joe:pw@userinfo.example.com/x' 'Type=Application'
# 1348 port and no-port URLs collide and are ambiguous
mkdir -p "$W/s/gurl-collide/d1/applications" "$W/s/gurl-collide/d2/applications" "$W/s/gurl-collide/cmd"
scn gurl-collide d1 'Ported.desktop' 0 '[Desktop Entry]' 'Name=Ported' 'Exec=omarchy-launch-webapp https://ported.example.com:8443/app' 'Type=Application'
scn gurl-collide d1 'Unported.desktop' 0 '[Desktop Entry]' 'Name=Unported' 'Exec=omarchy-launch-webapp https://ported.example.com/app' 'Type=Application'
# 1348 Foo.desktop and foo.desktop are distinct ids
mkdir -p "$W/s/case/d1/applications" "$W/s/case/d2/applications" "$W/s/case/cmd"
scn case d1 'Foo.desktop' 0 '[Desktop Entry]' 'Name=Upper' 'Exec=omarchy-launch-webapp https://upper.example.com' 'Type=Application'
scn case d1 'foo.desktop' 0 '[Desktop Entry]' 'Name=Lower' 'Exec=omarchy-launch-webapp https://lower.example.com' 'Type=Application'
# 1348 a nested desktop file still resolves
mkdir -p "$W/s/nested/d1/applications" "$W/s/nested/d2/applications" "$W/s/nested/cmd"
scn nested d1 'sub/bar.desktop' 0 '[Desktop Entry]' 'Name=Nested' 'Exec=omarchy-launch-webapp https://nested.example.com' 'Type=Application'
# 1608 RFC 3986 5.2.4 and GURL edge cases
mkdir -p "$W/s/rfc/d1/applications" "$W/s/rfc/d2/applications" "$W/s/rfc/cmd"
scn rfc d1 'TrailA.desktop' 0 '[Desktop Entry]' 'Name=TrailA' 'Exec=omarchy-launch-webapp https://trail-a.example.com/a/b/..' 'Type=Application'
scn rfc d1 'TrailB.desktop' 0 '[Desktop Entry]' 'Name=TrailB' 'Exec=omarchy-launch-webapp https://trail-b.example.com/a/b/../' 'Type=Application'
scn rfc d1 'Empty.desktop' 0 '[Desktop Entry]' 'Name=Empty' 'Exec=omarchy-launch-webapp https://empty.example.com/a//b' 'Type=Application'
scn rfc d1 'Dot.desktop' 0 '[Desktop Entry]' 'Name=Dot' 'Exec=omarchy-launch-webapp https://dot.example.com/a/.' 'Type=Application'
scn rfc d1 'Six.desktop' 0 '[Desktop Entry]' 'Name=Six' 'Exec=omarchy-launch-webapp https://[2001:db8::1]:8443/x/../y' 'Type=Application'
# 1608 a scheme-less Exec URL produces no web-app identity
mkdir -p "$W/s/rfc-schemeless/d1/applications" "$W/s/rfc-schemeless/d2/applications" "$W/s/rfc-schemeless/cmd"
scn rfc-schemeless d1 'Schemeless.desktop' 0 '[Desktop Entry]' 'Name=Schemeless' 'Exec=omarchy-launch-webapp example.com/app' 'Type=Application'
# 1864 CRLF .desktop files parse
mkdir -p "$W/s/crlf/d1/applications" "$W/s/crlf/d2/applications" "$W/s/crlf/cmd"
scn crlf d1 'crlf-app.desktop' 1 '[Desktop Entry]' 'Name=CRLF App' 'Exec=crlf-app %U' 'StartupWMClass=crlf-app' 'Type=Application'
scn crlf d1 'crlf-web.desktop' 1 '[Desktop Entry]' 'Name=CRLF Web' 'Exec=omarchy-launch-webapp https://crlf.example.com' 'Type=Application'
scn crlf d2 'lf-app.desktop' 0 '[Desktop Entry]' 'Name=LF App' 'Exec=lf-app %U' 'StartupWMClass=lf-app' 'Type=Application'
# 779 hosted argv keeps argument boundaries (Omarchy TUI .desktop shape)
mkdir -p "$W/s/hosted/d1/applications" "$W/s/hosted/d2/applications" "$W/s/hosted/cmd"
printf 'foot\0--app-id=TUI.float\0-e\0bash\0-c\0dua i /\0' >"$W/s/hosted/cmd/4400"
# A bare terminal (Categories=TerminalEmulator) running a command started in
# its shell (4502 leads the tty's foreground group), and one showing only its
# shell, cd'd elsewhere than where the terminal started.
mkdir -p "$W/s/fg/d1/applications" "$W/s/fg/d2/applications" "$W/s/fg/cmd"
printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=Foot' 'Exec=foot' \
  'Categories=System;TerminalEmulator;' 'X-TerminalArgExec=-e' >"$W/s/fg/d1/applications/foot.desktop"
fg() { # fg <pid> <ppid> <cwd> <tpgid|-> <argv...>
  local pid="$1" ppid="$2" cwd="$3" tpgid="$4"; shift 4
  printf '%s\0' "$@" >"$W/s/fg/cmd/$pid"
  printf '%s\n' "$ppid" >"$W/s/fg/cmd/$pid.ppid"
  printf '%s\n' "$cwd" >"$W/s/fg/cmd/$pid.cwd"
  [[ "$tpgid" == - ]] || printf '%s\n' "$tpgid" >"$W/s/fg/cmd/$pid.tpgid"
}
fg 4500 1 / - /usr/bin/foot
fg 4501 4500 /home/u/proj 4502 /usr/bin/bash
fg 4502 4501 /home/u/proj/sub - htop -d 5
fg 4510 1 / - /usr/bin/foot
fg 4511 4510 /home/u/Projects 4511 /usr/bin/bash
# 1094 the cmdline fallback keeps argv boundaries and every metacharacter literal
mkdir -p "$W/s/cmdline/d1/applications" "$W/s/cmdline/d2/applications" "$W/s/cmdline/cmd"
# shellcheck disable=SC2016 # the $(…) is the literal argv under test
printf 'weirdapp\0--title=two words\0--re=a;b&c\0--glob=*.txt\0--sub=$(touch /tmp/rl-pwned)\0--q=he said "hi"\0--bs=back\\slash\0' \
  >"$W/s/cmdline/cmd/950"
