#!/usr/bin/env bash
# The user's boot wiring, black-box against any engine: ensure-hooks,
# uninstall, the boot flags and the hyprland.lua bootstrap. RELAUNCH_ENGINE
# selects the engine (default: the bash one). The Rust engine writes only the
# root-anchor bootstrap (H1: no engine path loads the user's own Lua), the bash
# one only the legacy loader; checks of either arrangement run against its
# engine, and the rest against both. Every path is sandboxed: nothing here touches the
# live hyprland.lua or config directory.
set -euo pipefail
# Never the installed client: a suite names its own stand-in.
unset RELAUNCH_RELAUNCHCTL

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELAUNCH="${RELAUNCH_ENGINE:-$ROOT/tests/oracle/relaunch}"
# Absolute: some sections run from another directory.
RELAUNCH="$(realpath "$RELAUNCH")"
ENGINE_IS_BASH=false
[[ "$(head -c 64 "$RELAUNCH" 2>/dev/null | tr -d '\0')" == '#!'*bash* ]] && ENGINE_IS_BASH=true
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
LUABIN="$(command -v lua5.4 || command -v lua || true)"
[[ -n "$LUABIN" ]] || { printf 'FAIL: lua is required\n' >&2; exit 1; }

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

# One sandbox per scenario: $T is its root.
T=""
sandbox() {
  T="$WORKDIR/$1"
  rm -rf "$T"; mkdir -p "$T/cfg"
  printf '%s\n' '-- Extra autostart processes.' 'o.launch_on_start("sunsetr")' >"$T/autostart.lua"
  printf '%s\n' 'require("hypr.autostart")' >"$T/hyprland.lua"
  # The installed anchor (a fixtures build trusts our uid in place of root).
  printf -- '-- anchor\n' >"$T/anchor.lua"; chmod 0644 "$T/anchor.lua"
  printf '#!/bin/sh\n' >"$T/anchor-bin"; chmod 0755 "$T/anchor-bin"
}
# What identifies our bootstrap line in hyprland.lua.
if $ENGINE_IS_BASH; then HOOKPAT='omarchy-relaunch/relaunch.lua'; else HOOKPAT='pcall(dofile, '; fi
rl() {
  env HOME="$T/home" PREFIX="$T/prefix" RELAUNCH_CONFIG_DIR="$T/cfg" \
    RELAUNCH_AUTOSTART="$T/autostart.lua" RELAUNCH_HYPRLAND_LUA="$T/hyprland.lua" \
    RELAUNCH_HYPR_CONF="$T/hyprland.conf" RELAUNCH_PLUGIN_DIR="$T/plugin" \
    RELAUNCH_ANCHOR_BIN="$T/anchor-bin" RELAUNCH_ANCHOR_LUA="$T/anchor.lua" \
    RELAUNCH_RELAUNCHCTL="$T/relaunchctl" "$RELAUNCH" "$@"
}
rl_ctl() { # rl_ctl <client> <args...>: rl with another relaunchctl override
  local ctl="$1"; shift
  env HOME="$T/home" PREFIX="$T/prefix" RELAUNCH_CONFIG_DIR="$T/cfg" \
    RELAUNCH_AUTOSTART="$T/autostart.lua" RELAUNCH_HYPRLAND_LUA="$T/hyprland.lua" \
    RELAUNCH_HYPR_CONF="$T/hyprland.conf" RELAUNCH_PLUGIN_DIR="$T/plugin" \
    RELAUNCH_ANCHOR_BIN="$T/anchor-bin" RELAUNCH_ANCHOR_LUA="$T/anchor.lua" \
    RELAUNCH_RELAUNCHCTL="$ctl" "$RELAUNCH" "$@"
}

# --- first install
sandbox first
rl ensure-hooks >/dev/null 2>&1 || fail "ensure-hooks failed"
grep -q 'relaunch boot' "$T/autostart.lua" && fail "nothing may be written into autostart.lua"
cp "$T/hyprland.lua" "$T/hyprland.once"
if $ENGINE_IS_BASH; then
# The legacy loader registers boot before the gate, nothing else.
[[ -f "$T/cfg/relaunch.lua" ]] || fail "first install must create the loader"
grep -Fq "$T/prefix/bin/relaunch boot" "$T/cfg/relaunch.lua" \
  || fail "the loader must register the canonical engine: $(grep exec_on_start "$T/cfg/relaunch.lua")"
grep -qx 'return' "$T/cfg/relaunch.lua" && fail "the loader must not be a bare return placeholder"
[[ -f "$T/cfg/rules.lua" ]] || fail "rules.lua must exist for the loader to read"
grep -q 'o.window' "$T/cfg/relaunch.lua" && fail "pins must live in rules.lua, not the loader"
regline="$(grep -n 'exec_on_start' "$T/cfg/relaunch.lua" | head -1 | cut -d: -f1)"
gateline="$(grep -n 'disabled' "$T/cfg/relaunch.lua" | head -1 | cut -d: -f1)"
[[ -n "$regline" && -n "$gateline" && "$regline" -lt "$gateline" ]] \
  || fail "boot registration must precede the disable/skip gate (reg=$regline gate=$gateline)"
cp "$T/cfg/relaunch.lua" "$T/loader.once"
else
# The anchor line only: no loader, and rules.lua is a copy nothing loads.
[[ "$(cat "$T/hyprland.lua")" == "require(\"hypr.autostart\")"$'\n'"-- omarchy-relaunch"$'\n'"pcall(dofile, \"$T/anchor.lua\")" ]] \
  || fail "a fresh install must write exactly the anchor line: $(cat "$T/hyprland.lua")"
[[ ! -e "$T/cfg/relaunch.lua" ]] || fail "a fresh install must not write the legacy loader"
grep -q 'Inspection copy only' "$T/cfg/rules.lua" || fail "rules.lua must be the inspection copy"
grep -qE 'o\.|dofile|require' "$T/cfg/rules.lua" && fail "the inspection copy must hold no Lua: $(cat "$T/cfg/rules.lua")"
fi
rl ensure-hooks >/dev/null 2>&1
cmp -s "$T/hyprland.once" "$T/hyprland.lua" || fail "a second ensure-hooks changed hyprland.lua"
if $ENGINE_IS_BASH; then cmp -s "$T/loader.once" "$T/cfg/relaunch.lua" || fail "a second ensure-hooks changed the loader"; fi
[[ "$(grep -cF "$HOOKPAT" "$T/hyprland.lua")" -eq 1 ]] || fail "the bootstrap was duplicated"

if $ENGINE_IS_BASH; then
# --- fault isolation, EXECUTED: a malformed rules.lua must not take boot with it
printf 'this is not ) valid lua (\n' >"$T/cfg/rules.lua"
cat >"$T/harness.lua" <<'EOF'
local captured = nil
o = { exec_on_start = function(cmd) captured = cmd end }
local reported = nil
local real_print = print
print = function(msg) reported = tostring(msg) end
local ok = pcall(dofile, os.getenv("LOADER"))
print = real_print
if captured == nil then print("NO_REGISTRATION"); os.exit(1) end
if reported == nil or not string.find(reported, "rules.lua failed to load", 1, true) then
  print("NO_ERROR_REPORT"); os.exit(1)
end
if not ok then print("LOADER_ABORTED"); os.exit(1) end
print("OK " .. captured)
EOF
mkdir -p "$T/xdg"; ln -sfn "$T/cfg" "$T/xdg/omarchy-relaunch"
luaout="$(HOME="$T" XDG_CONFIG_HOME="$T/xdg" LOADER="$T/cfg/relaunch.lua" "$LUABIN" "$T/harness.lua" 2>&1)" \
  || fail "the loader must survive a malformed rules.lua: $luaout"
[[ "$luaout" == OK*boot* ]] || fail "loader fault isolation: $luaout"
fi

# --- autostart.lua is the user's: never written, whatever it contains
sandbox autostart
cat >"$T/autostart.lua" <<'EOF'
-- Extra autostart processes.
o.launch_on_start("sunsetr")
o.exec_on_start("relaunch boot")
-- omarchy-relaunch (managed; hidden from the Relaunch list)
o.launch_on_start("brave")
EOF
cp "$T/autostart.lua" "$T/autostart.before"
rl ensure-hooks >/dev/null 2>&1
cmp -s "$T/autostart.before" "$T/autostart.lua" || fail "ensure-hooks modified autostart.lua"
rl uninstall --yes >/dev/null 2>&1
cmp -s "$T/autostart.before" "$T/autostart.lua" || fail "uninstall modified autostart.lua"

# --- clean removal is byte for byte, cycle after cycle, EOF state included
cyclecheck() { # cyclecheck <label> ; original bytes in $T/hyprland.before
  cp "$T/hyprland.before" "$T/hyprland.lua"
  rl ensure-hooks >/dev/null 2>&1
  grep -qF "$HOOKPAT" "$T/hyprland.lua" || fail "$1: ensure-hooks must write the bootstrap"
  rl uninstall --yes >/dev/null 2>&1
  cmp -s "$T/hyprland.before" "$T/hyprland.lua" \
    || fail "$1: uninstall did not restore hyprland.lua: $(diff <(od -c "$T/hyprland.before") <(od -c "$T/hyprland.lua") | head -4 | tr '\n' ' ')"
}
sandbox cycle
printf '%s\n' 'require("hypr.autostart")' '-- trailing comment' >"$T/hyprland.before"
cyclecheck "a normal hyprland.lua"
for _i in 1 2 3 4 5; do cyclecheck "cycle $_i"; done
printf '%s' 'require("hypr.autostart")' >"$T/hyprland.before"
cyclecheck "a hyprland.lua with no final newline"
printf 'caf\351 \377\n' >"$T/hyprland.before"
cyclecheck "a hyprland.lua that is not UTF-8"
[[ ! -d "$T/cfg" ]] || fail "uninstall must remove the config dir"

# --- an unmarked copy of our hook is ours; uninstall removes nothing else
sandbox unmarked
rl ensure-hooks >/dev/null 2>&1
hookline="$(grep -F "$HOOKPAT" "$T/hyprland.lua")"
printf '%s\n' '-- keep' '-- omarchy-relaunch' "$hookline" '-- after' "$hookline" >"$T/hyprland.lua"
rl uninstall --yes >/dev/null 2>&1
grep -q 'omarchy-relaunch' "$T/hyprland.lua" && fail "the marker or a hook survived uninstall"
[[ "$(cat "$T/hyprland.lua")" == $'-- keep\n-- after' ]] \
  || fail "uninstall must remove exactly our lines: $(cat "$T/hyprland.lua")"
if rl uninstall >/dev/null 2>&1; then fail "uninstall must require --yes"; fi

# --- the plugin folder goes last: at once in text mode, after the reply for the panel
sandbox plugin
mkdir -p "$T/plugin"; : >"$T/plugin/Panel.qml"
rl ensure-hooks >/dev/null 2>&1
[[ "$(rl uninstall --yes)" == 'Relaunch removed.' ]] || fail "uninstall text reply"
[[ ! -e "$T/plugin" ]] || fail "text-mode uninstall must remove the plugin folder"
sandbox pluginjson
mkdir -p "$T/plugin"; : >"$T/plugin/Panel.qml"
rl ensure-hooks >/dev/null 2>&1
rl uninstall --yes --json >"$T.reply"
jq -e '.ok == true and .uninstalled == true' >/dev/null <"$T.reply" || fail "uninstall --json reply: $(cat "$T.reply")"
[[ -e "$T/plugin/Panel.qml" ]] || fail "the plugin folder must outlive the panel's reply"
for _w in 1 2 3 4 5 6; do [[ -e "$T/plugin" ]] || break; sleep 1; done
[[ ! -e "$T/plugin" ]] || fail "the plugin folder must be removed after the reply"

# --- a malformed overrides.json is left alone: ensure-hooks still works
sandbox badover
printf '{not json\n' >"$T/cfg/overrides.json"
cp "$T/cfg/overrides.json" "$T/overrides.before"
rl ensure-hooks >/dev/null 2>&1 || fail "ensure-hooks must still work over a malformed overrides.json"
cmp -s "$T/overrides.before" "$T/cfg/overrides.json" || fail "ensure-hooks must not touch a malformed overrides.json"

# --- the legacy hyprland.conf snippet is taken back
sandbox legacyconf
printf 'x\n' >"$T/cfg/relaunch.conf"
printf '%s\n' 'monitor = ,preferred,auto,1' '# omarchy-relaunch' "source = $T/cfg/relaunch.conf" >"$T/hyprland.conf"
rl ensure-hooks >/dev/null 2>&1
[[ ! -e "$T/cfg/relaunch.conf" ]] || fail "the legacy relaunch.conf must be removed"
[[ "$(cat "$T/hyprland.conf")" == 'monitor = ,preferred,auto,1' ]] \
  || fail "only our hyprland.conf lines may go: $(cat "$T/hyprland.conf")"

# --- boot flags are born private
sandbox flags
( umask 022; rl boot-skip >/dev/null )
[[ "$(stat -c '%a' "$T/cfg/skip-once")" == 600 ]] || fail "skip-once mode $(stat -c '%a' "$T/cfg/skip-once")"
( umask 022; rl boot-disable >/dev/null )
[[ "$(stat -c '%a' "$T/cfg/disabled")" == 600 ]] || fail "disabled mode $(stat -c '%a' "$T/cfg/disabled")"

# --- the legacy bootstrap treats an empty XDG_CONFIG_HOME as unset (executed)
sandbox xdg
if $ENGINE_IS_BASH; then
mkdir -p "$T/home/.config/omarchy-relaunch" "$T/xdg/omarchy-relaunch"
for w in home:home/.config xdg:xdg; do
  printf 'io.open(os.getenv("MARK"), "w"):write("%s"):close()\n' "${w%%:*}" >"$T/${w#*:}/omarchy-relaunch/relaunch.lua"
done
rl ensure-hooks >/dev/null 2>&1
hook="$(grep -F 'omarchy-relaunch/relaunch.lua' "$T/hyprland.lua")"
bootload() {
  rm -f "$T/mark"
  env HOME="$T/home" MARK="$T/mark" "$@" "$LUABIN" -e "$hook" >/dev/null 2>&1 || true
  cat "$T/mark" 2>/dev/null || printf 'none'
}
[[ "$(bootload env XDG_CONFIG_HOME=)" == home ]] || fail "empty XDG_CONFIG_HOME must mean unset"
[[ "$(bootload env -u XDG_CONFIG_HOME)" == home ]] || fail "unset XDG_CONFIG_HOME must use HOME"
[[ "$(bootload env XDG_CONFIG_HOME="$T/xdg")" == xdg ]] || fail "a set XDG_CONFIG_HOME must be used"
fi

# --- a released bootstrap is replaced, not joined, and uninstall takes it back
OLD_HOOK='local _rl = (os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config")) .. "/omarchy-relaunch/relaunch.lua"; local _f = io.open(_rl); if _f then _f:close(); dofile(_rl) end'
printf '%s\n' '-- keep' '-- omarchy-relaunch' "$OLD_HOOK" >"$T/hyprland.lua"
rl ensure-hooks >/dev/null 2>&1
[[ "$(grep -cF "$HOOKPAT" "$T/hyprland.lua")" -eq 1 && "$(grep -cx -- '-- omarchy-relaunch' "$T/hyprland.lua")" -eq 1 ]] \
  || fail "a released bootstrap must be replaced, not joined: $(cat "$T/hyprland.lua")"
grep -qxF "$OLD_HOOK" "$T/hyprland.lua" && fail "the released bootstrap survived ensure-hooks"
printf '%s\n' '-- keep' '-- omarchy-relaunch' "$OLD_HOOK" >"$T/hyprland.lua"
rl uninstall --yes >/dev/null 2>&1
[[ "$(cat "$T/hyprland.lua")" == '-- keep' ]] || fail "uninstall left a released bootstrap: $(cat "$T/hyprland.lua")"

# --- symlinked user files are edited through the link, never replaced
sandbox symlinks
mkdir -p "$T/dotfiles"
mv "$T/hyprland.lua" "$T/dotfiles/hyprland.lua"; ln -s "$T/dotfiles/hyprland.lua" "$T/hyprland.lua"
mv "$T/autostart.lua" "$T/dotfiles/autostart.lua"; ln -s "$T/dotfiles/autostart.lua" "$T/autostart.lua"
cp "$T/dotfiles/hyprland.lua" "$T/hyprland.before"
rl ensure-hooks >/dev/null 2>&1; rl ensure-hooks >/dev/null 2>&1
[[ -L "$T/hyprland.lua" && -L "$T/autostart.lua" ]] || fail "a symlinked user file stopped being a symlink"
[[ "$(grep -cF "$HOOKPAT" "$T/dotfiles/hyprland.lua")" -eq 1 ]] || fail "the bootstrap must reach the link target once"
rl uninstall --yes >/dev/null 2>&1
[[ -L "$T/hyprland.lua" ]] || fail "uninstall replaced the symlinked hyprland.lua"
cmp -s "$T/hyprland.before" "$T/dotfiles/hyprland.lua" || fail "uninstall must restore the link target"

# --- Relaunch's files are private from creation; the user's keep their modes
sandbox modes
rm -rf "$T/cfg"
chmod 0644 "$T/hyprland.lua"; chmod 0664 "$T/autostart.lua"
cp "$T/hyprland.lua" "$T/hyprland.before"
( umask 000; rl ensure-hooks >/dev/null 2>&1 )
[[ "$(stat -c '%a' "$T/cfg")" == 700 ]] || fail "the config dir must be created 0700"
bad="$(stat -c '%a %n' "$T/cfg"/* | awk '$1 != "600"')"
[[ -z "$bad" ]] || fail "every Relaunch file must be born 0600: $bad"
[[ "$(stat -c '%a' "$T/hyprland.lua")" == 644 ]] || fail "hyprland.lua must keep its mode"
[[ "$(stat -c '%a' "$T/autostart.lua")" == 664 ]] || fail "autostart.lua must keep its mode"
chmod 0755 "$T/cfg"; chmod 0644 "$T/cfg"/*
rl ensure-hooks >/dev/null 2>&1
[[ "$(stat -c '%a' "$T/cfg")" == 700 ]] || fail "ensure-hooks must tighten a 0755 config dir"
bad="$(stat -c '%a %n' "$T/cfg"/* | awk '$1 != "600"')"
[[ -z "$bad" ]] || fail "ensure-hooks must tighten 0644 files: $bad"
rl uninstall --yes >/dev/null 2>&1
cmp -s "$T/hyprland.before" "$T/hyprland.lua" || fail "uninstall must restore hyprland.lua"

# --- the shipped `relaunch` is a shim to the root-owned engine ---
SHIM="$ROOT/relaunch"
[[ "$(grep -c '^ENGINE=/usr/lib/omarchy-relaunch/relaunch$' "$SHIM")" == 1 ]] || fail "the shim must name the fixed engine path"
grep -qE '\$\{?(RELAUNCH|PATH|HOME|PREFIX|XDG)' "$SHIM" && fail "the shim must not consult the environment"
# shellcheck disable=SC2016 # matching the shim's literal source text
grep -qx 'exec "$ENGINE" "$@"' "$SHIM" || fail "the shim must exec the engine with its arguments"
if [[ ! -e /usr/lib/omarchy-relaunch/relaunch ]]; then
  code=0; err="$("$SHIM" list --json 2>&1 >/dev/null)" || code=$?
  [[ "$code" == 127 && "$err" == *"engine is not installed"* ]] || fail "a missing engine must fail clearly (exit $code: $err)"
  # install.sh refuses, and writes nothing, until the system core is installed.
  sandbox installer
  if HOME="$T/home" PREFIX="$T/prefix" "$ROOT/install.sh" >/dev/null 2>&1; then fail "install.sh must refuse without the system core"; fi
  [[ ! -e "$T/home" && ! -e "$T/prefix" ]] || fail "a refused install.sh must write nothing"
fi

if ! $ENGINE_IS_BASH; then  # behaviour the bash engine lacks (deltas DAN, DPG, DBT)
# --- the root-anchor bootstrap: gated on the installed anchor, the only line
# written, replacing a legacy one (DAN; H1)
sandbox anchor
LEGACY_HOOK='local _c = os.getenv("XDG_CONFIG_HOME"); if _c == nil or _c == "" then _c = os.getenv("HOME") .. "/.config" end; local _rl = _c .. "/omarchy-relaunch/relaunch.lua"; local _f = io.open(_rl); if _f then _f:close(); dofile(_rl) end'
printf '%s\n' 'require("hypr.autostart")' '-- omarchy-relaunch' "$LEGACY_HOOK" >"$T/hyprland.lua"
printf 'o.exec_on_start("evil")\n' >"$T/cfg/relaunch.lua"
printf 'o.window("x", {})\n' >"$T/cfg/rules.lua"
cp "$T/hyprland.lua" "$T/hyprland.legacy"; cp -a "$T/cfg" "$T/cfg.legacy"
refused() { # refused <why> ; ensure-hooks and --anchor both refuse, and write nothing
  local a
  for a in "" --anchor; do
    # shellcheck disable=SC2086 # $a is one word or none
    if rl ensure-hooks $a >/dev/null 2>&1; then fail "ensure-hooks $a must refuse $1"; fi
    cmp -s "$T/hyprland.legacy" "$T/hyprland.lua" || fail "a refused ensure-hooks $a ($1) changed hyprland.lua"
    diff -r "$T/cfg.legacy" "$T/cfg" >/dev/null || fail "a refused ensure-hooks $a ($1) changed the config dir"
  done
}
mv "$T/anchor.lua" "$T/anchor.real"
refused "a missing anchor.lua"
ln -s /etc/hostname "$T/anchor.lua"
refused "a symlinked anchor.lua, even to a root-owned file"
rm "$T/anchor.lua"; mv "$T/anchor.real" "$T/anchor.lua"
chmod 0664 "$T/anchor.lua"; refused "a group-writable anchor.lua"; chmod 0644 "$T/anchor.lua"
chmod 0775 "$T"; refused "an anchor in a group-writable directory"; chmod 0755 "$T"
chmod 0777 "$WORKDIR"; refused "an anchor whose directory's parent is world-writable"; chmod 0700 "$WORKDIR"
mv "$T/anchor-bin" "$T/anchor-bin.real"; refused "a missing relaunch-anchor"; mv "$T/anchor-bin.real" "$T/anchor-bin"
chmod 0775 "$T/anchor-bin"; refused "a group-writable relaunch-anchor"; chmod 0755 "$T/anchor-bin"
anchorline="pcall(dofile, \"$T/anchor.lua\")"
for a in "" --anchor; do
  cp "$T/hyprland.legacy" "$T/hyprland.lua"; rm -rf "$T/cfg"; cp -a "$T/cfg.legacy" "$T/cfg"
  # shellcheck disable=SC2086 # $a is one word or none
  rl ensure-hooks $a >/dev/null 2>&1 || fail "ensure-hooks $a must wire the installed anchor"
  [[ "$(cat "$T/hyprland.lua")" == "require(\"hypr.autostart\")"$'\n'"-- omarchy-relaunch"$'\n'"$anchorline" ]] \
    || fail "ensure-hooks $a: the anchor line must replace the legacy one: $(cat "$T/hyprland.lua")"
  [[ ! -e "$T/cfg/relaunch.lua" ]] || fail "ensure-hooks $a: the legacy loader must be removed"
  grep -q 'Inspection copy only' "$T/cfg/rules.lua" || fail "ensure-hooks $a: rules.lua must become an inspection copy"
done
# Both lines, by hand: the legacy one still goes.
printf '%s\n' '-- omarchy-relaunch' "$anchorline" "$LEGACY_HOOK" >"$T/hyprland.lua"
rl ensure-hooks >/dev/null 2>&1
grep -qF "$LEGACY_HOOK" "$T/hyprland.lua" && fail "a legacy line beside the anchor line must be removed"
[[ "$(grep -cF "$anchorline" "$T/hyprland.lua")" -eq 1 ]] || fail "the anchor line must stay exactly once"
# The inspection copy is ours: a link planted at its name is replaced, not followed.
printf 'victim\n' >"$T/victim"; rm "$T/cfg/rules.lua"; ln -s "$T/victim" "$T/cfg/rules.lua"
rl ensure-hooks >/dev/null 2>&1
[[ "$(cat "$T/victim")" == victim && -f "$T/cfg/rules.lua" && ! -L "$T/cfg/rules.lua" ]] \
  || fail "ensure-hooks must replace a planted rules.lua link, not write through it"
printf 'require("hypr.autostart")\n' >"$T/hyprland.before"
printf '%s\n' 'require("hypr.autostart")' '-- omarchy-relaunch' "$anchorline" >"$T/hyprland.lua"
rl uninstall --yes >/dev/null 2>&1
cmp -s "$T/hyprland.before" "$T/hyprland.lua" || fail "uninstall must take the anchor line back: $(cat "$T/hyprland.lua")"

# --- uninstall removes the PATH shim it installed, and only that
sandbox shimgone
mkdir -p "$T/prefix/bin"; cp "$ROOT/relaunch" "$T/prefix/bin/relaunch"
rl uninstall --yes >/dev/null 2>&1
[[ ! -e "$T/prefix/bin/relaunch" ]] || fail "uninstall must remove the relaunch shim on PATH"
sandbox shimkept
mkdir -p "$T/prefix/bin"; printf '#!/bin/sh\necho mine\n' >"$T/prefix/bin/relaunch"
rl uninstall --yes >/dev/null 2>&1
[[ -e "$T/prefix/bin/relaunch" ]] || fail "uninstall must not remove someone else's relaunch"

# --- uninstall purges the root state first; a failed purge removes nothing (DPG)
sandbox purge
rl ensure-hooks >/dev/null 2>&1
printf '#!/bin/sh\necho "$@" >"%s/purged"\nexit 0\n' "$T" >"$T/relaunchctl"; chmod 0755 "$T/relaunchctl"
rl uninstall --yes >/dev/null 2>&1 || fail "uninstall failed with a working purge"
[[ "$(cat "$T/purged")" == purge ]] || fail "uninstall must run relaunchctl purge"
sandbox purgefail
rl ensure-hooks >/dev/null 2>&1
cp "$T/hyprland.lua" "$T/hyprland.hooked"
printf '#!/bin/sh\nexit 1\n' >"$T/relaunchctl"; chmod 0755 "$T/relaunchctl"
if rl uninstall --yes >/dev/null 2>&1; then fail "uninstall must fail when the purge fails"; fi
[[ -d "$T/cfg" ]] || fail "a failed purge must leave the config dir"
cmp -s "$T/hyprland.hooked" "$T/hyprland.lua" || fail "a failed purge must leave hyprland.lua"
# A test build never runs an installed client, even one an inherited override
# names: root's /usr/bin/false (or a link of ours to it) is taken as absent, so
# the purge is skipped and uninstall succeeds; were it run, it would fail.
for theirs in /usr/bin/false "$T/link-to-false"; do
  sandbox rootctl
  ln -s /usr/bin/false "$T/link-to-false"
  rl ensure-hooks >/dev/null 2>&1
  out="$(rl_ctl "$theirs" uninstall --yes 2>&1)" \
    || fail "a fixtures build ran a client it does not own ($theirs): $out"
  [[ ! -d "$T/cfg" ]] || fail "uninstall with a refused client ($theirs) must still finish"
done

# --- a test build refuses every writer unless the config dir is sandboxed
# Only HOME is given (a sandbox home holding a config and a hooked
# hyprland.lua): each writer must fail, naming RELAUNCH_CONFIG_DIR, and leave
# that home byte-for-byte as it was.
sandbox writers
mkdir -p "$T/home/.config/hypr" "$T/home/.config/omarchy-relaunch"
printf '%s\n' 'require("hypr.autostart")' >"$T/home/.config/hypr/hyprland.lua"
printf '{}\n' >"$T/home/.config/omarchy-relaunch/config.json"
tree_sum() { (cd "$T/home" && find . -printf '%p %m %s\n' | sort && find . -type f -exec sha256sum {} + | sort) | sha256sum; }
before="$(tree_sum)"
for w in install-signed snapshot boot-skip boot-disable boot-enable ensure-hooks "uninstall --yes"; do
  # shellcheck disable=SC2086 # $w is a command and its flag
  out="$(env -i PATH="$PATH" HOME="$T/home" "$RELAUNCH" $w </dev/null 2>&1)" \
    && fail "$w ran in a test build without RELAUNCH_CONFIG_DIR"
  [[ "$out" == *RELAUNCH_CONFIG_DIR* ]] || fail "$w: refusal must name RELAUNCH_CONFIG_DIR: $out"
  [[ "$(tree_sum)" == "$before" ]] || fail "$w changed HOME while refusing"
done
# A reader is not refused.
out="$(env -i PATH="$PATH" HOME="$T/home" "$RELAUNCH" last-boot --json </dev/null 2>&1 || true)"
[[ "$out" != *RELAUNCH_CONFIG_DIR* ]] || fail "a reader was refused: $out"

# --- boot is the anchor's (DBT)
sandbox boot
rm "$T/anchor-bin"
if rl boot >/dev/null 2>&1; then fail "boot must fail without the anchor"; fi
printf '#!/bin/sh\necho "$@" >"%s/booted"\nexit 7\n' "$T" >"$T/anchor-bin"; chmod 0755 "$T/anchor-bin"
code=0; rl boot >/dev/null 2>&1 || code=$?
[[ "$code" == 7 && "$(cat "$T/booted")" == boot ]] || fail "boot must become 'relaunch-anchor boot' (exit $code)"
fi

printf 'ok %s\n' "$(basename "$0")"
