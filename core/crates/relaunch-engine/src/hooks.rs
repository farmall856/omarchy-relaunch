//! The user's boot wiring: `ensure-hooks` and `uninstall`.
//!
//! `hyprland.lua` carries one marked bootstrap line, which runs the root-owned
//! `anchor.lua` (spec §10/§11). No on-disk Lua of the user's is executed at
//! login: `ensure-hooks` writes only that line, and only once the root-owned
//! anchor is installed; it removes the legacy `relaunch.lua` loader and turns
//! `rules.lua` into an inspection copy nothing loads. The legacy bootstrap
//! lines are still recognized, so they are replaced (never joined) and taken
//! back by uninstall. Every edit of a user file goes through
//! `rewrite_user_file` (through any symlink, mode kept), and uninstall restores
//! `hyprland.lua` byte for byte.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use relaunch_sys::fs::{
    create_private_if_absent, rewrite_user_file, trusted_chain, write_private_artifact,
};

use crate::env::Env;

/// The comment above our bootstrap line.
const MARKER: &str = "-- omarchy-relaunch";

/// The legacy bootstrap, which `dofile`d the user-owned `relaunch.lua`: never
/// written, only replaced and taken back.
pub const HOOK_LINE: &str = r#"local _c = os.getenv("XDG_CONFIG_HOME"); if _c == nil or _c == "" then _c = os.getenv("HOME") .. "/.config" end; local _rl = _c .. "/omarchy-relaunch/relaunch.lua"; local _f = io.open(_rl); if _f then _f:close(); dofile(_rl) end"#;

/// An older legacy bootstrap: likewise only replaced and taken back.
const SUPERSEDED_HOOK_LINES: [&str; 1] = [
    r#"local _rl = (os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config")) .. "/omarchy-relaunch/relaunch.lua"; local _f = io.open(_rl); if _f then _f:close(); dofile(_rl) end"#,
];

const LOADER_FILE: &str = "relaunch.lua";
const RULES_FILE: &str = "rules.lua";
const OVERRIDES_FILE: &str = "overrides.json";
const LEGACY_SNIPPET: &str = "relaunch.conf";
/// What identifies the bash engine's PATH copy (and so is ours to remove).
const BASH_ENGINE_MARK: &str = "omarchy-relaunch bar widget";

/// A Lua double-quoted string's contents.
fn lua_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The anchor bootstrap line for `anchor_lua`.
#[must_use]
pub fn anchor_line(anchor_lua: &Path) -> String {
    format!(
        "pcall(dofile, \"{}\")",
        lua_escape(&anchor_lua.to_string_lossy())
    )
}

/// `hyprland.lua` without our bootstrap: the marker immediately above one of
/// `hooks`, and any unmarked copy of one, removed byte for byte; a lone marker
/// and everything else kept, the file's final-newline state included.
#[must_use]
pub fn remove_bootstrap(input: &[u8], hooks: &[&str]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let final_nl = input.ends_with(b"\n");
    let body = input.strip_suffix(b"\n").unwrap_or(input);
    let is_hook = |line: &[u8]| hooks.iter().any(|h| h.as_bytes() == line);
    let marker = MARKER.as_bytes();
    let mut out = Vec::with_capacity(input.len());
    let mut held = false;
    for line in body.split(|b| *b == b'\n') {
        if held {
            held = false;
            if is_hook(line) {
                continue;
            }
            out.extend_from_slice(marker);
            out.push(b'\n');
        }
        if line == marker {
            held = true;
            continue;
        }
        if is_hook(line) {
            continue;
        }
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    if held {
        out.extend_from_slice(marker);
        out.push(b'\n');
    }
    if !final_nl {
        out.pop();
    }
    out
}

/// `hyprland.lua` with exactly one marked `hook` at the end: any of `ours`
/// removed first, then the marker and hook appended with no blank separator.
/// The appended last line carries the file's own end-of-file state, so
/// [`remove_bootstrap`] restores the original bytes.
#[must_use]
pub fn install_bootstrap(input: &[u8], hook: &str, ours: &[&str]) -> Vec<u8> {
    let mut out = remove_bootstrap(input, ours);
    if !out.is_empty() && out.last() != Some(&b'\n') {
        out.extend_from_slice(format!("\n{MARKER}\n{hook}").as_bytes());
    } else {
        out.extend_from_slice(format!("{MARKER}\n{hook}\n").as_bytes());
    }
    out
}

/// Every bootstrap line Relaunch has written: the anchor line and the legacy
/// ones.
fn all_hooks(env: &Env) -> Vec<String> {
    let mut v = vec![anchor_line(&env.anchor_lua)];
    v.extend(LEGACY_HOOK_LINES.iter().map(|s| (*s).to_owned()));
    v
}

/// The legacy bootstraps, current first.
const LEGACY_HOOK_LINES: [&str; 2] = [HOOK_LINE, SUPERSEDED_HOOK_LINES[0]];

fn contains(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

fn required<'a>(p: Option<&'a PathBuf>, what: &str) -> Result<&'a Path, String> {
    p.map(PathBuf::as_path)
        .ok_or_else(|| format!("{what}: HOME is not set"))
}

fn io_err<'a>(what: &str, path: &'a Path) -> impl FnOnce(std::io::Error) -> String + 'a {
    let what = what.to_owned();
    move |e| format!("{what}: {}: {e}", path.display())
}

/// The config dir is 0700 and each regular file in it (through a symlink too)
/// 0600; an install predating these modes is tightened once.
fn tighten_config_dir(dir: &Path) {
    let set = |p: &Path, mode: u32| {
        if std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o7777 != mode) {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
        }
    };
    if !dir.is_dir() {
        return;
    }
    set(dir, 0o700);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if !hidden && std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            set(&path, 0o600);
        }
    }
}

/// Drop the pre-Lua arrangement: the `relaunch.conf` snippet and the lines that
/// sourced it from `hyprland.conf`.
fn remove_legacy_conf(env: &Env) -> Result<(), String> {
    let snippet = env.config_dir.join(LEGACY_SNIPPET);
    let _ = std::fs::remove_file(&snippet);
    let _ = std::fs::remove_file(env.config_dir.join(format!("{LEGACY_SNIPPET}.tmp")));
    let Some(conf) = env.hypr_conf.as_deref() else {
        return Ok(());
    };
    let source = format!("source = {}", snippet.display());
    rewrite_user_file(conf, |old| {
        let lines: Vec<&[u8]> = old.split(|b| *b == b'\n').collect();
        let body =
            lines.split_last().map_or(
                &lines[..],
                |(last, rest)| if last.is_empty() { rest } else { &lines[..] },
            );
        let ours = |l: &[u8]| contains(l, &source) || l == b"# omarchy-relaunch";
        if !body.iter().any(|l| ours(l)) {
            return old.to_vec();
        }
        body.iter()
            .filter(|l| !ours(l))
            .flat_map(|l| [*l, b"\n"])
            .flatten()
            .copied()
            .collect()
    })
    .map(|_| ())
    .map_err(io_err("remove legacy hyprland.conf lines", conf))
}

/// A root-installed file: a regular file (not a symlink) whose own inode, its
/// directory and that directory's parent are each owned by root (or, in a
/// fixtures build, by us) and not group- or world-writable — the shared
/// [`trusted_chain`] walk the anchor uses.
fn root_installed(path: &Path) -> bool {
    let trusted_uid =
        |uid: u32| uid == 0 || (cfg!(feature = "fixtures") && uid == relaunch_sys::current_uid());
    let top = path.parent().and_then(Path::parent).unwrap_or(path);
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && trusted_chain(path, trusted_uid, top).is_ok()
}

/// Install or repair the boot wiring (`ensure-hooks`): the anchor bootstrap
/// line, and nothing that runs the user's own Lua. `--anchor` is accepted and
/// changes nothing (it named this arrangement when a legacy one existed).
///
/// # Errors
///
/// A message if a path is unknown, the root-owned anchor is not installed
/// (then nothing is written), or a write fails.
pub fn ensure_hooks(env: &Env) -> Result<(), String> {
    let lua = required(env.hyprland_lua.as_ref(), "ensure-hooks")?;
    if !(root_installed(&env.anchor_lua) && root_installed(&env.anchor_bin)) {
        return Err(format!(
            "ensure-hooks: the root-owned anchor is not installed ({} and {} must be root-owned regular files in root-owned directories, none group- or world-writable)",
            env.anchor_lua.display(),
            env.anchor_bin.display()
        ));
    }
    let hook = anchor_line(&env.anchor_lua);
    let current = relaunch_sys::fs::read_regular_capped(lua, relaunch_sys::fs::MAX_FILE_BYTES)
        .unwrap_or_default();
    let dir = &env.config_dir;
    if let Some(parent) = lua.parent() {
        std::fs::create_dir_all(parent).map_err(io_err("ensure-hooks", parent))?;
    }
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(io_err("ensure-hooks", dir))?;
    }
    tighten_config_dir(dir);
    let overrides = dir.join(OVERRIDES_FILE);
    create_private_if_absent(&overrides, b"{}\n", dir)
        .map_err(io_err("ensure-hooks", &overrides))?;
    remove_legacy_conf(env)?;

    let _ = std::fs::remove_file(dir.join(LOADER_FILE));
    let copy = format!(
        "-- Inspection copy only: nothing loads this file.\n-- Boot is loaded by {} through relaunch-anchor emit-lua.\n",
        env.anchor_lua.display()
    );
    write_private_artifact(dir, RULES_FILE, copy.as_bytes())
        .map_err(io_err("ensure-hooks", &dir.join(RULES_FILE)))?;

    if !contains(&current, &hook) || LEGACY_HOOK_LINES.iter().any(|h| contains(&current, h)) {
        let ours = all_hooks(env);
        let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
        rewrite_user_file(lua, |old| install_bootstrap(old, &hook, &ours))
            .map_err(io_err("ensure-hooks", lua))?;
    }
    Ok(())
}

/// The durable deletion of this user's root signing state, when the authority
/// service is installed.
fn purge(env: &Env) -> Result<(), String> {
    if !relaunch_sys::fs::is_executable_file(&env.relaunchctl) {
        return Ok(());
    }
    let status = Command::new(&env.relaunchctl)
        .arg("purge")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("uninstall: relaunchctl purge: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("uninstall: relaunchctl purge failed; nothing was removed (retry, or remove the system package first)".into())
    }
}

/// What uninstall still has to do after it has answered: the plugin directory
/// the panel is drawn from goes last, or the panel vanishes before its reply.
pub enum Finish {
    /// Nothing left.
    Done,
    /// Remove the plugin directory once the reply has been read.
    RemovePluginLater(PathBuf),
}

/// Remove Relaunch for this user (`uninstall --yes`): the root signing state
/// first (`relaunchctl purge`), then the bootstrap, the legacy snippet, the
/// config directory, the bash engine's PATH copy and the plugin directory.
///
/// # Errors
///
/// A message if the purge fails (then nothing is removed) or a removal fails.
pub fn uninstall(env: &Env, defer_plugin: bool) -> Result<Finish, String> {
    purge(env)?;
    if let Some(lua) = env.hyprland_lua.as_deref() {
        let ours = all_hooks(env);
        let ours: Vec<&str> = ours.iter().map(String::as_str).collect();
        rewrite_user_file(lua, |old| remove_bootstrap(old, &ours))
            .map_err(io_err("uninstall", lua))?;
    }
    remove_legacy_conf(env)?;
    remove_tree(&env.config_dir)?;
    if let Some(bin) = env.engine_bin.as_deref() {
        let is_bash_engine =
            relaunch_sys::fs::read_regular_capped(bin, relaunch_sys::fs::MAX_FILE_BYTES)
                .is_ok_and(|b| contains(&b, BASH_ENGINE_MARK));
        if is_bash_engine {
            std::fs::remove_file(bin).map_err(io_err("uninstall", bin))?;
        }
    }
    match env.plugin_dir.clone() {
        Some(dir) if defer_plugin => Ok(Finish::RemovePluginLater(dir)),
        Some(dir) => remove_tree(&dir).map(|()| Finish::Done),
        None => Ok(Finish::Done),
    }
}

/// `rm -rf`: a symlink is removed itself, never followed.
fn remove_tree(path: &Path) -> Result<(), String> {
    let result = match std::fs::symlink_metadata(path) {
        Err(_) => return Ok(()),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    };
    result.map_err(io_err("uninstall", path))
}

/// Remove the plugin directory two seconds from now, after this process has
/// exited and the panel has read the reply.
///
/// # Errors
///
/// A message if the remover cannot be started.
pub fn remove_plugin_later(dir: &Path) -> Result<(), String> {
    Command::new("sh")
        .args(["-c", "sleep 2; rm -rf -- \"$1\"", "relaunch-uninstall"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("uninstall: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: [&str; 2] = [HOOK_LINE, SUPERSEDED_HOOK_LINES[0]];

    fn cycle(original: &[u8]) {
        let installed = install_bootstrap(original, HOOK_LINE, &OURS);
        assert_ne!(installed, original);
        assert_eq!(
            installed,
            install_bootstrap(&installed, HOOK_LINE, &OURS),
            "idempotent"
        );
        assert_eq!(
            remove_bootstrap(&installed, &OURS),
            original,
            "{:?}",
            String::from_utf8_lossy(original)
        );
    }

    #[test]
    fn install_then_remove_restores_the_bytes() {
        for original in [
            &b"require(\"hypr.autostart\")\n-- trailing comment\n"[..],
            b"require(\"hypr.autostart\")",
            b"",
            b"\n",
            b"a\n\n",
            b"-- omarchy-relaunch\nnot ours\n",
            b"x\n-- omarchy-relaunch",
        ] {
            cycle(original);
        }
    }

    #[test]
    fn bytes_that_are_not_utf8_survive_the_cycle() {
        cycle(b"caf\xe9 -- latin-1\n\xff\xfe\n");
        cycle(b"\xc3");
    }

    #[test]
    fn appended_block_follows_the_file_end() {
        assert_eq!(
            install_bootstrap(b"a", HOOK_LINE, &OURS),
            format!("a\n{MARKER}\n{HOOK_LINE}").into_bytes()
        );
        assert_eq!(
            install_bootstrap(b"a\n", HOOK_LINE, &OURS),
            format!("a\n{MARKER}\n{HOOK_LINE}\n").into_bytes()
        );
    }

    #[test]
    fn a_released_or_unmarked_hook_is_replaced_not_joined() {
        let released = format!("-- keep\n{MARKER}\n{}\n", SUPERSEDED_HOOK_LINES[0]);
        let out =
            String::from_utf8(install_bootstrap(released.as_bytes(), HOOK_LINE, &OURS)).unwrap();
        assert_eq!(out, format!("-- keep\n{MARKER}\n{HOOK_LINE}\n"));
        let unmarked = format!("a\n{HOOK_LINE}\nb\n");
        assert_eq!(remove_bootstrap(unmarked.as_bytes(), &OURS), b"a\nb\n");
    }

    #[test]
    fn the_anchor_line_replaces_the_legacy_one() {
        let anchor = anchor_line(Path::new("/usr/share/omarchy-relaunch/anchor.lua"));
        assert_eq!(
            anchor,
            r#"pcall(dofile, "/usr/share/omarchy-relaunch/anchor.lua")"#
        );
        let ours = [HOOK_LINE, anchor.as_str()];
        let legacy = format!("x\n{MARKER}\n{HOOK_LINE}\n");
        let out = install_bootstrap(legacy.as_bytes(), &anchor, &ours);
        assert_eq!(out, format!("x\n{MARKER}\n{anchor}\n").into_bytes());
        assert_eq!(remove_bootstrap(&out, &ours), b"x\n");
    }

    #[test]
    fn the_anchor_line_escapes_its_path_for_lua() {
        assert_eq!(
            anchor_line(Path::new(r#"/x/a"b\c.lua"#)),
            r#"pcall(dofile, "/x/a\"b\\c.lua")"#
        );
    }
}
