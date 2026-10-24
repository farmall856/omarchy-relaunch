#!/usr/bin/env bash
# Differential oracle for the bash -> Rust engine port.
#
#   tests/differential.sh ENGINE                 # the port gate: ENGINE vs goldens,
#                                                # listed deltas (deltas.txt) required
#   tests/differential.sh --oracle ENGINE        # bash self-check vs its own goldens
#   tests/differential.sh --update ENGINE        # (re)write goldens from the bash oracle
#   tests/differential.sh ENGINE_A ENGINE_B      # fixture cases, A vs B
#   --cases=REGEX  run only cases whose name matches (an engine port in progress)
#   --write-deltas ENGINE  record ENGINE's output as the expected delta for each
#                  LISTED delta case; review every one against its bash golden
#   tests/differential.sh --live ENGINE_A ENGINE_B
#       read-only cases on THIS desktop: real hyprctl, /proc and XDG dirs, with a
#       throwaway config dir (never the live ~/.config/omarchy-relaunch).
#
# One observation per case: exit code, stdout, stderr, and the resulting file
# tree (mode + sha256). JSON output is compared as canonical JSON (jq -cS): key
# order and whitespace are not part of the contract, every value byte is.
# Timestamps and the sandbox path are masked. A listed, intended difference is
# a DELTA (tests/differential/DELTAS.md): its expected output lives in
# golden/<case>.delta.obs and the bash golden must still differ from it.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIFF_DIR="$ROOT/tests/differential"
GOLDEN="$DIFF_DIR/golden"
ANCHOR_BIN="${RELAUNCH_ANCHOR_BIN:-$ROOT/core/target/debug/relaunch-anchor}"

update=false live=false oracle=false only="" write_deltas=false
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --update) update=true ;;
    --live) live=true ;;
    --oracle) oracle=true ;;
    --cases=*) only="${1#--cases=}" ;;
    --write-deltas) write_deltas=true ;;
    *) printf 'unknown option %s\n' "$1" >&2; exit 2 ;;
  esac
  shift
done
(($# >= 1 && $# <= 2)) || { sed -n '2,10p' "$0" >&2; exit 2; }
ENGINE_A="$(readlink -f "$1")"
ENGINE_B="${2:+$(readlink -f "$2")}"
$live && [[ -z "$ENGINE_B" ]] && { printf -- '--live needs two engines\n' >&2; exit 2; }
[[ -x "$ANCHOR_BIN" ]] || { printf 'build the anchor first: %s\n' "$ANCHOR_BIN" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- cases -------------------------------------------------------------------
# One JSON object per case: {name, args, stdin?}. Window-derived probe cases are
# generated from the world's clients so every window is covered.
fixture_cases() {
  local clients="$1"
  jq -c . <<'EOF'
{"name":"index-dump","args":["__index-dump"]}
{"name": "suite:hosted:resolve", "args": ["__resolve", "TUI.float", "4400"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/hosted/d1:@W/s/hosted/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/hosted/cmd"}}
{"name": "suite:hosted:capture", "args": ["__capture", "TUI.float", "4400", "false"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/hosted/d1:@W/s/hosted/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/hosted/cmd"}}
{"name": "suite:fg:command:capture", "args": ["__capture", "foot", "4500", "false"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/fg/d1:@W/s/fg/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/fg/cmd"}}
{"name": "suite:fg:shell:capture", "args": ["__capture", "foot", "4510", "false"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/fg/d1:@W/s/fg/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/fg/cmd"}}
{"name": "suite:cmdline:resolve", "args": ["__resolve", "weirdapp", "950"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/cmdline/d1:@W/s/cmdline/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/cmdline/cmd"}}
{"name": "suite:cmdline:capture", "args": ["__capture", "weirdapp", "950", "false"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/cmdline/d1:@W/s/cmdline/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/cmdline/cmd"}}
{"name": "suite:pwa:Proton Mail", "args": ["__resolve", "Proton Mail", "75"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/pwa/d1:@W/s/pwa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/pwa/cmd"}}
{"name": "suite:pwa:brave-jnpecgipniidlgicjocehkhajgdnjekh-Default", "args": ["__resolve", "brave-jnpecgipniidlgicjocehkhajgdnjekh-Default", "76"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/pwa/d1:@W/s/pwa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/pwa/cmd"}}
{"name": "suite:wa:brave-maps.google.com__-Default", "args": ["__resolve", "brave-maps.google.com__-Default", "1000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:brave-example.com__foo_bar-Default", "args": ["__resolve", "brave-example.com__foo_bar-Default", "1001"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:brave-queried.example.com__-Default", "args": ["__resolve", "brave-queried.example.com__-Default", "1002"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:brave-my-site.com__a-b-Default", "args": ["__resolve", "brave-my-site.com__a-b-Default", "1003"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:brave-browser", "args": ["__resolve", "brave-browser", "1004"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:brave-maps.google.com__-Profile_2", "args": ["__resolve", "brave-maps.google.com__-Profile_2", "1000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:wa:chromium-maps.google.com__-Default", "args": ["__resolve", "chromium-maps.google.com__-Default", "1001"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/wa/cmd"}}
{"name": "suite:collide:brave-example.com__foo_bar-Default", "args": ["__resolve", "brave-example.com__foo_bar-Default", "1001"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/collide/d1:@W/s/collide/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/collide/cmd"}}
{"name": "suite:mask-sys:brave-maps.google.com__-Default", "args": ["__resolve", "brave-maps.google.com__-Default", "1000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/mask-sys/d1:@W/s/mask-sys/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/mask-sys/cmd"}}
{"name": "suite:mask-hidden:brave-maps.google.com__-Default", "args": ["__resolve", "brave-maps.google.com__-Default", "1000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/mask-hidden/d1:@W/s/mask-hidden/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/mask-hidden/cmd"}}
{"name": "suite:mask-other:brave-maps.google.com__-Default", "args": ["__resolve", "brave-maps.google.com__-Default", "1000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/mask-other/d1:@W/s/mask-other/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/mask-other/cmd"}}
{"name": "suite:appid:my app; touch /tmp/rl-appid-pwned", "args": ["__resolve", "my app; touch /tmp/rl-appid-pwned", "1200"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/appid/d1:@W/s/appid/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/appid/cmd"}}
{"name": "suite:gurl:brave-ported.example.com__app-Default", "args": ["__resolve", "brave-ported.example.com__app-Default", "1300"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/gurl/d1:@W/s/gurl/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/gurl/cmd"}}
{"name": "suite:gurl:brave-dotty.example.com__a_b-Default", "args": ["__resolve", "brave-dotty.example.com__a_b-Default", "1301"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/gurl/d1:@W/s/gurl/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/gurl/cmd"}}
{"name": "suite:gurl:brave-userinfo.example.com__x-Default", "args": ["__resolve", "brave-userinfo.example.com__x-Default", "1302"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/gurl/d1:@W/s/gurl/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/gurl/cmd"}}
{"name": "suite:gurl-collide:brave-ported.example.com__app-Default", "args": ["__resolve", "brave-ported.example.com__app-Default", "1300"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/gurl-collide/d1:@W/s/gurl-collide/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/gurl-collide/cmd"}}
{"name": "suite:case:brave-upper.example.com__-Default", "args": ["__resolve", "brave-upper.example.com__-Default", "1400"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/case/d1:@W/s/case/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/case/cmd"}}
{"name": "suite:case:brave-lower.example.com__-Default", "args": ["__resolve", "brave-lower.example.com__-Default", "1401"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/case/d1:@W/s/case/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/case/cmd"}}
{"name": "suite:nested:brave-nested.example.com__-Default", "args": ["__resolve", "brave-nested.example.com__-Default", "1500"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/nested/d1:@W/s/nested/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/nested/cmd"}}
{"name": "suite:rfc:brave-trail-a.example.com__a_-Default", "args": ["__resolve", "brave-trail-a.example.com__a_-Default", "2000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc/d1:@W/s/rfc/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc/cmd"}}
{"name": "suite:rfc:brave-trail-b.example.com__a_-Default", "args": ["__resolve", "brave-trail-b.example.com__a_-Default", "2001"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc/d1:@W/s/rfc/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc/cmd"}}
{"name": "suite:rfc:brave-empty.example.com__a__b-Default", "args": ["__resolve", "brave-empty.example.com__a__b-Default", "2002"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc/d1:@W/s/rfc/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc/cmd"}}
{"name": "suite:rfc:brave-dot.example.com__a_-Default", "args": ["__resolve", "brave-dot.example.com__a_-Default", "2003"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc/d1:@W/s/rfc/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc/cmd"}}
{"name": "suite:rfc:brave-[2001:db8::1]__y-Default", "args": ["__resolve", "brave-[2001:db8::1]__y-Default", "2004"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc/d1:@W/s/rfc/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc/cmd"}}
{"name": "suite:rfc-schemeless:brave-example.com__app-Default", "args": ["__resolve", "brave-example.com__app-Default", "2100"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/rfc-schemeless/d1:@W/s/rfc-schemeless/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/rfc-schemeless/cmd"}}
{"name": "suite:crlf:crlf-app", "args": ["__resolve", "crlf-app", "4000"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/crlf/d1:@W/s/crlf/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/crlf/cmd"}}
{"name": "suite:crlf:lf-app", "args": ["__resolve", "lf-app", "4001"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/crlf/d1:@W/s/crlf/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/crlf/cmd"}}
{"name": "suite:crlf:brave-crlf.example.com__-Default", "args": ["__resolve", "brave-crlf.example.com__-Default", "4002"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/crlf/d1:@W/s/crlf/d2", "RELAUNCH_CMDLINE_DIR": "@W/s/crlf/cmd"}}
{"name": "suite:wa:label", "args": ["__label", "brave-maps.google.com__-Default", "gio launch '@W/s/wa/d1/applications/Google Maps.desktop'", "desktop-file"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/wa/d1:@W/s/wa/d2"}}
{"name": "suite:crlf:label", "args": ["__label", "crlf-app", "gio launch @W/s/crlf/d1/applications/crlf-app.desktop", "desktop-file"], "env": {"RELAUNCH_DATA_DIRS": "@W/s/crlf/d1:@W/s/crlf/d2"}}
{"name":"list","args":["list","--json"]}
{"name":"propose","args":["propose","--json"]}
{"name":"ensure-hooks","args":["ensure-hooks"]}
{"name":"ensure-hooks-twice","args":["ensure-hooks"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"uninstall","args":["uninstall","--yes"]}
{"name":"uninstall-no-yes","args":["uninstall"]}
{"name":"boot-skip-json-signed","args":["boot-skip","--json"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"boot-skip","args":["boot-skip","--json"]}
{"name":"boot-disable","args":["boot-disable","--json"]}
{"name":"boot-enable","args":["boot-enable","--json"]}
{"name":"snapshot","args":["snapshot","--json"]}
{"name":"snapshot-text","args":["snapshot"]}
{"name":"list-text","args":["list"]}
{"name":"list-signed","args":["list","--json"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"list-signed-text","args":["list"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"list-bad-overrides","args":["list","--json"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-badover"}}
{"name":"reload","args":["reload","--json"]}
{"name":"reload-text","args":["reload"]}
{"name":"last-boot","args":["last-boot","--json"]}
{"name":"last-boot-text","args":["last-boot"]}
{"name":"last-boot-missing","args":["last-boot","--json"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"last-session","args":["last-session","--json"]}
{"name":"last-session-text","args":["last-session"]}
{"name":"last-session-diff","args":["last-session","--diff","--json"]}
{"name":"last-session-diff-text","args":["last-session","--diff"]}
{"name":"last-session-missing","args":["last-session"],"env":{"RELAUNCH_CONFIG_DIR":"@W/cfg-signed"}}
{"name":"label-gio","args":["__label","MyApp","gio launch /nonexistent/x.desktop","desktop-file"]}
{"name":"label-terminal","args":["__label","TUI.float","xdg-terminal-exec --app-id=TUI.float -e bash -c dua\\ i\\ /","terminal"]}
{"name":"label-override","args":["__label","Custom","custom --typed","overrides-table"]}
{"name":"label-gio-real-file","args":["__label","MyApp","gio launch @W/data1/applications/myapp.desktop","desktop-file"]}
{"name":"label-gio-crlf-file","args":["__label","CrlfApp","gio launch @W/data1/applications/crlf.desktop","desktop-file"]}
{"name":"label-gio-quoted-path","args":["__label","X","gio launch '@W/data1/applications/nameonly-xyz.desktop'","desktop-file"]}
{"name":"label-terminal-unmapped","args":["__label","htop","foot -e htop","terminal"]}
{"name":"label-terminal-mapped-leaf","args":["__label","TUI.float","foot -e dua i /","terminal"]}
{"name":"label-cmdline-leaf","args":["__label","brave","/usr/bin/brave --app=https://x.example.com/","cmdline"]}
{"name":"label-empty-exec","args":["__label","SomeClass","","guess"]}
{"name":"label-shell-c","args":["__label","x","bash -c 'lazygit --path /x'","cmdline"]}
{"name":"trust-xwayland","args":["__trust","true","101"]}
{"name":"trust-missing-field","args":["__trust","","101"]}
{"name":"trust-foreign","args":["__trust","false","105"]}
{"name":"install-signed-valid","args":["install-signed","--json"],"stdin":"{\"v\":2,\"signed\":{\"v\":1,\"uid\":1000,\"user\":\"u\",\"gen\":1,\"entries\":[]},\"signature\":\"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\"}"}
{"name":"install-signed-bad-sig-length","args":["install-signed","--json"],"stdin":"{\"v\":2,\"signed\":{\"v\":1,\"uid\":1000,\"user\":\"u\",\"gen\":1,\"entries\":[]},\"signature\":\"0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\"}"}
{"name":"install-signed-null-v","args":["install-signed","--json"],"stdin":"{\"v\":null,\"signed\":null,\"signature\":\"\"}"}
{"name":"install-signed-multidoc","args":["install-signed","--json"],"stdin":"{\"x\":1}\n{\"v\":2,\"signed\":{},\"signature\":\"ab\"}"}
{"name":"install-signed-legacy","args":["install-signed","--json"],"stdin":"{\"staggerSeconds\":0,\"entries\":[]}"}
{"name":"install-signed-valid-text","args":["install-signed"],"stdin":"{\"v\":2,\"signed\":{\"v\":1,\"uid\":1000,\"user\":\"u\",\"gen\":1,\"entries\":[]},\"signature\":\"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\"}\n\n"}
{"name":"boot-skip-text","args":["boot-skip"]}
{"name":"boot-disable-text","args":["boot-disable"]}
{"name":"boot-enable-text","args":["boot-enable"]}
{"name":"probe-quote","args":["__quote","a b","~x","y=~z","\t","","it's"]}
{"name":"probe-split","args":["__split","bash -c 'dua i /' \"x y\" a\\ b"]}
{"name":"probe-split-ifs","args":["__split-ifs","a  b\tc\nd 'e f'"]}
{"name":"probe-nkey","args":["__nkey","Org.Foo_Bar  Baz"]}
EOF
  # Index lookups by class, launch command, leaf and web-app class.
  jq -c . <<'EOF'
{"name":"index-dump:gtk-launch-only","args":["__index-dump"],"env":{"PATH":"@GTKONLY"}}
{"name":"index-dump:no-launcher","args":["__index-dump"],"env":{"PATH":"@NOLAUNCHER"}}
{"name":"index-dump:dirs-reversed","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@W/data2:@W/data1"}}
{"name":"index-dump:apps-dir-direct","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@W/data1/applications:@W/data2"}}
{"name":"index-dump:empty-fields","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":":@W/data1::@W/missing:@W/data2:"}}
{"name":"lookup:gtk:myapp","args":["__lookup","MyApp"],"env":{"PATH":"@GTKONLY"}}
{"name":"index-dump:xdg","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@UNSET","XDG_DATA_HOME":"@W/data1","XDG_DATA_DIRS":"@W/data2"}}
{"name":"index-dump:xdg-default-home","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@UNSET","XDG_DATA_HOME":"@UNSET","XDG_DATA_DIRS":"@W/data2"}}
{"name":"index-dump:xdg-relative-home","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@UNSET","XDG_DATA_HOME":"relative/share","XDG_DATA_DIRS":"@W/data2"}}
{"name":"index-dump:xdg-trailing-slash","args":["__index-dump"],"env":{"RELAUNCH_DATA_DIRS":"@UNSET","XDG_DATA_HOME":"@W/data1","XDG_DATA_DIRS":"@W/data2/"}}
{"name":"lookup:exec:myapp","args":["__lookup","myapp --flag %U"]}
{"name":"lookup:exec:first-word","args":["__lookup","linkedapp --whatever"]}
{"name":"lookup:leaf:dua","args":["__lookup","dua"]}
{"name":"lookup:name:Name Only","args":["__lookup","Name Only"]}
{"name":"lookup:id:idonly","args":["__lookup","idonly"]}
{"name":"lookup:nested","args":["__lookup","Nested"]}
{"name":"lookup:webapp:ambiguous","args":["__lookup","brave-dup.example.com__-Default"]}
{"name":"lookup:webapp:empty-ident","args":["__lookup","brave--Default"]}
{"name":"lookup:shadowed","args":["__lookup","shadowed"]}
{"name":"lookup:hidden-name","args":["__lookup","Shadowed Override"]}
EOF
  jq -c '.[] | (.initialClass // .class) as $c | (.pid | tostring) as $p | (.xwayland | tostring) as $x
         | {name: ("lookup:" + $c), args: ["__lookup", $c]},
           {name: ("capture:" + $c), args: ["__capture", $c, $p, $x]},
           {name: ("resolve:" + $c), args: ["__resolve", $c, $p]},
           {name: ("identity:" + $p + ":" + $c), args: ["__identity", $p]}' "$clients"
}

live_cases() {
  jq -c . <<'EOF'
{"name":"live:index-dump","args":["__index-dump"]}
{"name":"live:list","args":["list","--json"]}
{"name":"live:propose","args":["propose","--json"]}
EOF
  hyprctl clients -j | jq -c '.[] | select(.pid != null)
    | (if (.initialClass // "") != "" then .initialClass else .class end) as $c
    | (.pid | tostring) as $p | (.xwayland | tostring) as $x
    | {name: ("live:lookup:" + $c + ":" + $p), args: ["__lookup", $c]},
      {name: ("live:capture:" + $c + ":" + $p), args: ["__capture", $c, $p, $x]},
      {name: ("live:resolve:" + $c + ":" + $p), args: ["__resolve", $c, $p]},
      {name: ("live:identity:" + $p), args: ["__identity", $p]}'
}

# --- running one case --------------------------------------------------------
stub_hyprctl() {
  cat >"$1" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
  clients) cat "$2" ;;
  monitors) printf '[]\n' ;;
  reload) printf 'ok\n' ;;
  *) exit 1 ;;
esac
EOF
  chmod +x "$1"
}

normalize() { # normalize <file> <sandbox>
  local f="$1" sb="$2" out
  if [[ -s "$f" ]] && out="$(jq -cS . "$f" 2>/dev/null)"; then
    printf '%s\n' "$out"
  else
    cat "$f"
  fi | mask "$sb"
}

# The one masking rule, for streams and file content alike: the sandbox path and
# ISO-8601 timestamps (the engine stamps boot logs and session snapshots).
mask() {
  sed -E \
    -e "s#${1//#/\\#}#<W>#g" \
    -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([+-][0-9]{2}:[0-9]{2}|Z)?/<TS>/g'
}

tree_digest() { # tree_digest <dir> ; content hashed after mask()
  local sb="$1"
  (cd "$sb" && find . -mindepth 1 \( -type f -o -type l \) -print0 | LC_ALL=C sort -z |
    while IFS= read -r -d '' p; do
      if [[ -L "$p" ]]; then
        printf '%s -> %s\n' "$p" "$(readlink "$p")"
      else
        printf '%s %s %s\n' "$p" "$(stat -c %a "$p")" \
          "$(mask "$sb" <"$p" | sha256sum | cut -d' ' -f1)"
      fi
    done)
}

observe() { # observe <engine> <case-json> <outfile> ; fixture mode
  local engine="$1" case="$2" obs="$3" sb rc=0
  sb="$(mktemp -d "$WORK/sb.XXXXXX")"
  if $live; then
    mkdir -p "$sb/cfg"
  else
    "$DIFF_DIR/world.sh" "$sb"
    stub_hyprctl "$sb.hyprctl" "$sb/clients.json"
  fi
  # The installed anchor loader beside the sandbox (out of the file digest; its
  # path is masked like the sandbox's): the Rust engine wires boot only to it.
  mkdir -p "$sb.anchor"
  printf -- '-- anchor\n' >"$sb.anchor/anchor.lua"; chmod 0644 "$sb.anchor/anchor.lua"
  local -a args=()
  mapfile -t -d '' args < <(jq -j '.args[] | ., "\u0000"' <<<"$case")
  args=("${args[@]//@W/$sb}")
  local stdin
  stdin="$(jq -r '.stdin // empty' <<<"$case")"
  local -a env=(
    RELAUNCH_CONFIG_DIR="$sb/cfg"
    RELAUNCH_ANCHOR_BIN="$ANCHOR_BIN" RELAUNCH_ANCHOR_LUA="$sb.anchor/anchor.lua"
    RELAUNCH_VERIFY_SLEEP=0 RELAUNCH_VERIFY_DEADLINE=0 RELAUNCH_VERIFY_INTERVAL=0
  )
  # Live mode reads the real hyprctl, /proc, XDG dirs and autostart.lua, but every
  # WRITE target is sandboxed: list/ensure-hooks rewrite hyprland.lua, the loader
  # and the engine copy, and the live ones must never be touched.
  $live && env+=(
    RELAUNCH_HYPRLAND_LUA="$sb/hyprland.lua" RELAUNCH_HYPR_CONF="$sb/hyprland.conf"
    RELAUNCH_PLUGIN_DIR="$sb/plugin" PREFIX="$sb/prefix"
  )
  $live || env+=(
    HOME="$sb/home" HYPRCTL="$sb.hyprctl"
    RELAUNCH_DATA_DIRS="$sb/data1:$sb/data2" RELAUNCH_CMDLINE_DIR="$sb/cmd"
    RELAUNCH_AUTOSTART="$sb/hypr/autostart.lua" RELAUNCH_HYPRLAND_LUA="$sb/hypr/hyprland.lua"
    RELAUNCH_HYPR_CONF="$sb/hypr/hyprland.conf" RELAUNCH_PLUGIN_DIR="$sb/plugin" PREFIX="$sb/prefix"
  )
  # Per-case overrides; tokens: @W sandbox, @NOLAUNCHER / @GTKONLY PATH dirs.
  local kv
  local -a unset=()
  while IFS= read -r -d '' kv; do
    if [[ "${kv#*=}" == @UNSET ]]; then
      unset+=(-u "${kv%%=*}")
      continue
    fi
    kv="${kv//@W/$sb}"
    kv="${kv//@NOLAUNCHER/$WORK/path-none}"
    kv="${kv//@GTKONLY/$WORK/path-gtk}"
    env+=("$kv")
  done < <(jq -j '(.env // {}) | to_entries[] | "\(.key)=\(.value)\u0000"' <<<"$case")
  # Unsets apply after the base env, so a case can remove a harness default.
  local -a cleaned=()
  for kv in "${env[@]}"; do
    local skip=false n
    for ((n = 1; n < ${#unset[@]}; n += 2)); do [[ "${kv%%=*}" == "${unset[n]}" ]] && skip=true; done
    $skip || cleaned+=("$kv")
  done
  printf '%s' "$stdin" | env ${unset[@]+"${unset[@]}"} ${cleaned[@]+"${cleaned[@]}"} "$engine" "${args[@]}" >"$sb.out" 2>"$sb.err" || rc=$?
  # DLM: a capture names the .desktop it copied as `desktopFile`; the hash
  # belongs to the pin's own launcher now. bash's `launcher {path, sha256?}` is
  # read as that, so every other field is still compared.
  if $live && [[ "$(jq -r .name <<<"$case")" == live:capture:* ]] && jq -e 'has("launcher")' "$sb.out" >/dev/null 2>&1; then
    jq -c '.desktopFile = .launcher.path | del(.launcher)' "$sb.out" >"$sb.out.dlm" && mv "$sb.out.dlm" "$sb.out"
  fi
  {
    printf '== exit\n%s\n== stdout\n' "$rc"
    normalize "$sb.out" "$sb"
    printf '== stderr\n'
    normalize "$sb.err" "$sb"
    if ! $live; then
      printf '== files\n'
      tree_digest "$sb" | sed -E "s#${sb//#/\\#}#<W>#g"
    fi
  } >"$obs"
  rm -rf "$sb" "$sb.out" "$sb.err" "$sb.hyprctl" "$sb.anchor"
}

case_file() { printf '%s' "$1" | sed -E 's/[^A-Za-z0-9._:-]/_/g'; }

# --- main --------------------------------------------------------------------
# PATH dirs without a desktop launcher (everything in /usr/bin but gio and
# gtk-launch), and with gtk-launch only: the index's launch strings depend on
# which is found.
for f in /usr/bin/*; do
  case "${f##*/}" in gio | gtk-launch) continue ;; esac
  mkdir -p "$WORK/path-none" && ln -s "$f" "$WORK/path-none/"
done
cp -a "$WORK/path-none" "$WORK/path-gtk" && ln -s /usr/bin/gtk-launch "$WORK/path-gtk/"

W0="$WORK/world0"
"$DIFF_DIR/world.sh" "$W0"
if $live; then
  mapfile -t CASES < <(live_cases)
else
  mapfile -t CASES < <(fixture_cases "$W0/clients.json")
fi

$update && mkdir -p "$GOLDEN"

# Listed deltas: case name -> delta IDs.
declare -A DELTA_OF=()
while IFS= read -r line; do
  [[ -z "$line" || "$line" == \#* ]] && continue
  DELTA_OF["${line#* }"]+="${DELTA_OF["${line#* }"]:+,}${line%% *}"
done <"$DIFF_DIR/deltas.txt"
fail=0 n=0 deltas=0
for case in "${CASES[@]}"; do
  name="$(jq -r .name <<<"$case")"
  [[ -n "$only" && ! "$name" =~ $only ]] && continue
  cf="$(case_file "$name")"
  n=$((n + 1))
  observe "$ENGINE_A" "$case" "$WORK/a.obs"
  if [[ -n "$ENGINE_B" ]]; then
    observe "$ENGINE_B" "$case" "$WORK/b.obs"
    if ! diff -u --label "A:$name" --label "B:$name" "$WORK/a.obs" "$WORK/b.obs"; then
      fail=$((fail + 1))
    fi
  elif $update; then
    cp "$WORK/a.obs" "$GOLDEN/$cf.obs"
  else
    expect="$GOLDEN/$cf.obs"
    if $write_deltas; then
      if [[ -n "${DELTA_OF[$name]:-}" ]]; then
        cp "$WORK/a.obs" "$GOLDEN/$cf.delta.obs"
        printf 'wrote delta %s (%s)\n' "$name" "${DELTA_OF[$name]}"
      fi
      continue
    fi
    if ! $oracle && [[ -n "${DELTA_OF[$name]:-}" ]]; then
      deltas=$((deltas + 1))
      if [[ ! -f "$GOLDEN/$cf.delta.obs" ]]; then
        printf 'DELTA %s (%s): no expected output golden/%s.delta.obs\n' "$name" "${DELTA_OF[$name]}" "$cf"
        fail=$((fail + 1)); continue
      fi
      cmp -s "$GOLDEN/$cf.obs" "$GOLDEN/$cf.delta.obs" &&
        { printf 'DELTA %s: delta.obs equals the bash golden (not a real delta)\n' "$name"; fail=$((fail + 1)); }
      expect="$GOLDEN/$cf.delta.obs"
    elif [[ -f "$GOLDEN/$cf.delta.obs" ]]; then
      $oracle || { printf 'UNLISTED delta file for %s (add it to deltas.txt)\n' "$name"; fail=$((fail + 1)); }
    fi
    if [[ ! -f "$expect" ]]; then
      printf 'MISSING golden: %s\n' "$name"; fail=$((fail + 1))
    elif ! diff -u --label "golden:$name" --label "engine:$name" "$expect" "$WORK/a.obs"; then
      fail=$((fail + 1))
    fi
  fi
done

if $update; then
  printf 'wrote %d goldens to %s\n' "$n" "${GOLDEN#"$ROOT/"}"
elif ((fail)); then
  printf '\ndifferential: %d of %d cases differ\n' "$fail" "$n"; exit 1
else
  printf 'differential: %d cases identical%s\n' "$n" "$( ((deltas)) && printf ' (%d via listed deltas)' "$deltas")"
fi
