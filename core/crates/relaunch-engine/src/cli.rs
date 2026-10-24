//! Command dispatch. Output contract matches the bash engine's: `--json`
//! anywhere selects JSON on stdout (errors as `{"ok":false,"error":…}`);
//! otherwise text on stdout and `error: …` on stderr. Exit 1 on a refused
//! command, 2 on usage errors and commands the Rust engine does not have yet.

use std::ffi::OsString;
use std::io::{Read, Write};

use lexopt::Arg;

use relaunch_core::MAX_DOCUMENT_BYTES;
use relaunch_sys::fs::{touch_private, write_private};
use relaunch_sys::paths;

use crate::config::parse_signed;
use crate::env::Env;

/// The process's standard streams, injectable for tests.
pub struct Io<'a> {
    /// Standard input.
    pub stdin: &'a mut dyn Read,
    /// Standard output: each command's protocol.
    pub stdout: &'a mut dyn Write,
    /// Standard error: diagnostics.
    pub stderr: &'a mut dyn Write,
}

struct Ctx<'a, 'b> {
    io: &'a mut Io<'b>,
    json: bool,
}

impl Ctx<'_, '_> {
    fn out(&mut self, text: &str) {
        let _ = self.io.stdout.write_all(text.as_bytes());
        let _ = self.io.stdout.flush();
    }
    fn fail(&mut self, msg: &str) -> i32 {
        if self.json {
            let body = serde_json::json!({ "ok": false, "error": msg });
            self.out(&format!("{body}\n"));
        } else {
            let _ = writeln!(self.io.stderr, "error: {msg}");
        }
        1
    }
    fn not_ported(&mut self, what: &str) -> i32 {
        let _ = writeln!(
            self.io.stderr,
            "error: {what} is not ported to the Rust engine yet"
        );
        2
    }
}

/// The commands that write the user's files (config, launchers, boot flags,
/// hyprland.lua, the plugin) or reach the authority service. A fixtures build
/// is a test build and refuses them unless the config dir is sandboxed, so a
/// test that forgets to sandbox fails instead of rewriting the live desktop.
const WRITERS: &[&str] = &[
    "install-signed",
    "snapshot",
    "boot-skip",
    "boot-disable",
    "boot-enable",
    "ensure-hooks",
    "uninstall",
];

/// Run one command line (`args` excludes the program name). Returns the exit
/// status. Arguments are parsed with `lexopt` (OsString-native: a non-UTF-8
/// argument is refused, never a panic). Probes take their arguments raw, so a
/// probed value that looks like an option (`--app-id=x`) is not parsed.
pub fn run(args: Vec<OsString>, env: &Env, io: &mut Io<'_>) -> i32 {
    let mut parser = lexopt::Parser::from_args(args);
    let cmd = match parser.next() {
        Ok(Some(Arg::Value(v))) => match v.into_string() {
            Ok(s) => s,
            Err(_) => return usage(io, "the command must be valid UTF-8"),
        },
        Ok(Some(other)) => return usage(io, &format!("expected a command, got {other:?}")),
        Ok(None) => return usage(io, "missing command"),
        Err(e) => return usage(io, &e.to_string()),
    };
    if cmd.starts_with("__") {
        let raw: Result<Vec<String>, OsString> = match parser.raw_args() {
            Ok(r) => r.map(OsString::into_string).collect(),
            Err(e) => return usage(io, &e.to_string()),
        };
        let Ok(raw) = raw else {
            return usage(io, "arguments must be valid UTF-8");
        };
        let mut ctx = Ctx { io, json: true };
        return probes::run(&mut ctx, env, &cmd, &raw);
    }
    let opts = match Opts::parse(&mut parser, &cmd) {
        Ok(o) => o,
        Err(msg) => return usage(io, &msg),
    };
    let mut ctx = Ctx {
        io,
        json: opts.json,
    };
    if cfg!(feature = "fixtures") && WRITERS.contains(&cmd.as_str()) && !env.config_dir_named {
        return ctx.fail(&format!(
            "{cmd}: a test build writes only into a sandbox; set RELAUNCH_CONFIG_DIR"
        ));
    }
    let flags = &opts.flags;
    match cmd.as_str() {
        "import" | "set-exec" | "drop" => {
            let result = edit(&mut ctx, env, &cmd, &opts);
            emit(&mut ctx, result.map(|v| format!("{v}\n")))
        }
        "set-delays" => {
            ctx.json = true;
            let ms = |name: &str| {
                opts.value(name)
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or_else(|| format!("set-delays requires --{name} MS"))
            };
            let result = ms("pre")
                .and_then(|pre| Ok((pre, ms("post")?)))
                .and_then(|(pre, post)| crate::edit::set_delays(env, pre, post));
            emit(&mut ctx, result.map(|v| format!("{v}\n")))
        }
        "install-signed" => install_signed(&mut ctx, env),
        "propose" => propose(&mut ctx, env, flags.contains(&"merge")),
        "list" => list(&mut ctx, env),
        "snapshot" => {
            let result = crate::session::snapshot(env, ctx.json);
            emit(&mut ctx, result)
        }
        "last-session" => {
            let result = crate::session::last_session(env, ctx.json, flags.contains(&"diff"));
            emit(&mut ctx, result)
        }
        "last-boot" => last_boot(&mut ctx, env, flags.contains(&"open")),
        "reload" => reload(&mut ctx, env),
        "boot-skip" => boot_flags(&mut ctx, env, Flags::Skip),
        "boot-disable" => boot_flags(&mut ctx, env, Flags::Disable),
        "boot-enable" => boot_flags(&mut ctx, env, Flags::Enable),
        "ensure-hooks" => {
            let result = crate::hooks::ensure_hooks(env).map(|()| String::new());
            emit(&mut ctx, result)
        }
        "uninstall" => uninstall(&mut ctx, env, flags.contains(&"yes")),
        "boot" => boot(&mut ctx, env),
        // Retired by D-P4: nothing writes the launch list without the human's
        // authorization, and the boot rules come only from the signed document.
        "save" => ctx.fail("save is retired: capture from the Relaunch panel, then review and Authorize"),
        "generate" => ctx.fail("generate is retired: boot rules come from the signed config (relaunch-anchor emit-lua)"),
        other => usage(ctx.io, &format!("unknown command {other:?}")),
    }
}

/// A command's options after the command word.
struct Opts {
    json: bool,
    flags: Vec<&'static str>,
    values: Vec<(&'static str, String)>,
}

impl Opts {
    /// Parse every option: `--json`, the command's flags and valued options;
    /// anything else is a usage error.
    fn parse(parser: &mut lexopt::Parser, cmd: &str) -> Result<Self, String> {
        let mut o = Opts {
            json: false,
            flags: Vec::new(),
            values: Vec::new(),
        };
        loop {
            match parser.next() {
                Ok(None) => return Ok(o),
                Ok(Some(Arg::Long("json"))) => o.json = true,
                Ok(Some(Arg::Long(l))) if command_flag(cmd, l).is_some() => {
                    o.flags.extend(command_flag(cmd, l));
                }
                Ok(Some(Arg::Long(l))) if command_value(cmd, l).is_some() => {
                    let name = command_value(cmd, l).unwrap_or_default();
                    match parser.value().map(OsString::into_string) {
                        Ok(Ok(v)) => o.values.push((name, v)),
                        Ok(Err(_)) => return Err(format!("{cmd}: --{name} must be valid UTF-8")),
                        Err(e) => return Err(format!("{cmd}: {e}")),
                    }
                }
                Ok(Some(Arg::Long("exec"))) if cmd == "set-exec" => {
                    return Err("set-exec: pass the command on stdin with --exec-stdin (a command line is readable by other users)".into());
                }
                Ok(Some(other)) => {
                    let what = match other {
                        Arg::Value(v) => format!("unexpected argument {}", v.display()),
                        Arg::Long(l) => format!("unknown option --{l}"),
                        Arg::Short(c) => format!("unknown option -{c}"),
                    };
                    return Err(format!("{cmd}: {what}"));
                }
                Err(e) => return Err(format!("{cmd}: {e}")),
            }
        }
    }

    /// The last value given for a valued option.
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The valued options a command takes.
fn command_value(cmd: &str, opt: &str) -> Option<&'static str> {
    match (cmd, opt) {
        ("import" | "set-exec" | "drop", "class") => Some("class"),
        ("import", "workspace") => Some("workspace"),
        ("set-delays", "pre") => Some("pre"),
        ("set-delays", "post") => Some("post"),
        _ => None,
    }
}

/// An edit of the signed list, as the reply the panel relays to `relaunchctl`.
fn edit(
    ctx: &mut Ctx<'_, '_>,
    env: &Env,
    cmd: &str,
    opts: &Opts,
) -> Result<serde_json::Value, String> {
    ctx.json = true;
    let class = opts
        .value("class")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| format!("{cmd} requires --class"))?;
    match cmd {
        "import" => {
            let ws = opts
                .value("workspace")
                .and_then(crate::host::parse_pid)
                .ok_or("import requires --workspace N (N>=1)")?;
            crate::edit::import(env, class, ws)
        }
        "set-exec" => {
            if !opts.flags.contains(&"exec-stdin") {
                return Err("set-exec requires --exec-stdin".into());
            }
            let mut buf = Vec::new();
            let limit = u64::try_from(relaunch_core::MAX_EXEC_LEN).unwrap_or(u64::MAX) + 2;
            ctx.io
                .stdin
                .take(limit)
                .read_to_end(&mut buf)
                .map_err(|e| format!("set-exec: {e}"))?;
            let text = String::from_utf8(buf)
                .map_err(|_| "set-exec: the command must be UTF-8".to_string())?;
            let line = text.split('\n').next().unwrap_or_default().trim();
            crate::edit::set_exec(env, class, line)
        }
        _ => crate::edit::drop(env, class),
    }
}

/// The flags a command takes besides `--json`.
fn command_flag(cmd: &str, flag: &str) -> Option<&'static str> {
    match (cmd, flag) {
        ("last-boot", "open") => Some("open"),
        ("last-session", "diff") => Some("diff"),
        ("ensure-hooks", "anchor") => Some("anchor"),
        ("propose", "merge") => Some("merge"),
        ("uninstall", "yes") => Some("yes"),
        ("set-exec", "exec-stdin") => Some("exec-stdin"),
        _ => None,
    }
}

fn usage(io: &mut Io<'_>, msg: &str) -> i32 {
    let _ = writeln!(
        io.stderr,
        "error: {msg}\nusage: relaunch <command> [--json]"
    );
    2
}

/// Persist a signed document produced by relaunchd and relayed by the panel
/// (§12). The body is stdin with trailing newlines removed; it is installed
/// byte-for-byte only if it is exactly one strict signed envelope. Its pins'
/// launchers are written first, each rebuilt from its signed entry and
/// refused unless it hashes to what was signed; launchers no pin names are
/// removed only after the document is in place.
fn install_signed(ctx: &mut Ctx<'_, '_>, env: &Env) -> i32 {
    const REFUSED: &str = "install-signed: input is not a signed config document";
    let mut input = Vec::new();
    let limit = u64::try_from(MAX_DOCUMENT_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(4096);
    if ctx.io.stdin.take(limit).read_to_end(&mut input).is_err() {
        return ctx.fail(REFUSED);
    }
    while input.last() == Some(&b'\n') {
        input.pop();
    }
    let Ok(doc) = parse_signed(&input) else {
        return ctx.fail(REFUSED);
    };
    if let Err(e) = crate::launcher::install(&env.config_dir, &doc.signed) {
        return ctx.fail(&format!("install-signed: {e}"));
    }
    let path = env.config_dir.join(paths::CONFIG_FILE);
    if let Err(e) = write_private(&path, &input, &env.config_dir) {
        return ctx.fail(&format!("install-signed: {e}"));
    }
    if let Err(e) = crate::launcher::collect(&env.config_dir, &doc.signed) {
        return ctx.fail(&format!("install-signed: {e}"));
    }
    if ctx.json {
        ctx.out("{\"ok\":true}\n");
    } else {
        ctx.out(&format!("Installed signed config: {}\n", path.display()));
    }
    0
}

/// Capture the running windows into a proposal for the panel to relay. JSON
/// only: the panel is its one consumer.
fn propose(ctx: &mut Ctx<'_, '_>, env: &Env, merge: bool) -> i32 {
    ctx.json = true;
    let mode = if merge {
        crate::propose::SaveMode::Merge
    } else {
        crate::propose::SaveMode::Replace
    };
    match crate::propose::run(env, mode) {
        Ok(body) => {
            ctx.out(&format!("{body}\n"));
            0
        }
        Err(e) => ctx.fail(&e),
    }
}

/// Print a command's output, or its error.
fn emit(ctx: &mut Ctx<'_, '_>, result: Result<String, String>) -> i32 {
    match result {
        Ok(out) => {
            ctx.out(&out);
            0
        }
        Err(e) => ctx.fail(&e),
    }
}

/// Show the last boot's record or log, or open the log (then, for `--json`,
/// print the panel state).
fn last_boot(ctx: &mut Ctx<'_, '_>, env: &Env, open: bool) -> i32 {
    use crate::session::LastBoot;
    let what = if open { LastBoot::Open } else { LastBoot::Show };
    match crate::session::last_boot(env, ctx.json, &what) {
        Ok(Some(out)) => {
            ctx.out(&out);
            0
        }
        Ok(None) => list(ctx, env),
        Err(e) => ctx.fail(&e),
    }
}

/// Print the panel state, or the text listing.
fn list(ctx: &mut Ctx<'_, '_>, env: &Env) -> i32 {
    let result = if ctx.json {
        crate::list::state(env).map(|v| format!("{v}\n"))
    } else {
        crate::list::text(env)
    };
    emit(ctx, result)
}

/// Reload Hyprland (its own "ok" is not part of this command's output), then
/// print the panel state or a confirmation.
fn reload(ctx: &mut Ctx<'_, '_>, env: &Env) -> i32 {
    let reloaded = std::process::Command::new(&env.hyprctl)
        .arg("reload")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !reloaded {
        return ctx.fail("hyprctl reload failed");
    }
    if ctx.json {
        list(ctx, env)
    } else {
        ctx.out("Hyprland reloaded.\n");
        0
    }
}

#[derive(Clone, Copy)]
enum Flags {
    Skip,
    Disable,
    Enable,
}

/// The boot flags are files, consulted by the anchor at login (§10). They
/// never rewrite `config.json` (a signed document must not be touched by a
/// flag change, Sol #7), and there is no `rules.lua` to regenerate (D-P4).
fn boot_flags(ctx: &mut Ctx<'_, '_>, env: &Env, which: Flags) -> i32 {
    let dir = &env.config_dir;
    let skip = dir.join(paths::SKIP_ONCE_FLAG);
    let disabled = dir.join(paths::DISABLED_FLAG);
    let result = (|| -> std::io::Result<&'static str> {
        match which {
            Flags::Skip => {
                touch_private(&skip, dir)?;
                Ok("Relaunch will skip the next boot.\n")
            }
            Flags::Disable => {
                touch_private(&disabled, dir)?;
                remove_if_present(&skip)?;
                Ok("Relaunch disabled until re-enabled.\n")
            }
            Flags::Enable => {
                remove_if_present(&disabled)?;
                remove_if_present(&skip)?;
                Ok("Relaunch enabled on boot.\n")
            }
        }
    })();
    match result {
        Ok(_) if ctx.json => list(ctx, env),
        Ok(text) => {
            ctx.out(text);
            0
        }
        Err(e) => ctx.fail(&format!("boot flags: {e}")),
    }
}

/// Remove Relaunch for this user. The reply comes before the plugin directory
/// goes, so the panel drawn from it can still show it.
fn uninstall(ctx: &mut Ctx<'_, '_>, env: &Env, yes: bool) -> i32 {
    use crate::hooks::Finish;
    if !yes {
        return ctx.fail("uninstall requires --yes");
    }
    match crate::hooks::uninstall(env, ctx.json) {
        Err(e) => ctx.fail(&e),
        Ok(Finish::RemovePluginLater(dir)) => {
            ctx.out("{\"ok\":true,\"uninstalled\":true}\n");
            match crate::hooks::remove_plugin_later(&dir) {
                Ok(()) => 0,
                Err(e) => {
                    let _ = writeln!(ctx.io.stderr, "error: {e}");
                    1
                }
            }
        }
        Ok(Finish::Done) => {
            if ctx.json {
                ctx.out("{\"ok\":true,\"uninstalled\":true}\n");
            } else {
                ctx.out("Relaunch removed.\n");
            }
            0
        }
    }
}

/// `boot` is the root anchor's (spec §10): this process becomes
/// `relaunch-anchor boot`, which verifies the signed document before launching
/// anything. Without the anchor installed nothing is launched.
fn boot(ctx: &mut Ctx<'_, '_>, env: &Env) -> i32 {
    use std::os::unix::process::CommandExt as _;
    if !relaunch_sys::fs::is_executable_file(&env.anchor_bin) {
        return ctx.fail(&format!(
            "boot: {} is not installed; nothing was launched",
            env.anchor_bin.display()
        ));
    }
    let err = std::process::Command::new(&env.anchor_bin)
        .arg("boot")
        .exec();
    ctx.fail(&format!("boot: {}: {err}", env.anchor_bin.display()))
}

fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The differential oracle's read-only probes, mirroring the bash engine's
/// `__*` commands. Present only in a `fixtures` (test) build.
#[cfg(feature = "fixtures")]
mod probes {
    use super::Ctx;
    use crate::env::Env;
    use crate::host::parse_pid;
    use crate::index::{DesktopIndex, Launcher};
    use crate::{argv, key, resolve, shell, webapp};

    /// The client's `xwayland` field as the bash probes pass it (`jq tostring`).
    fn xwayland(text: &str) -> Option<bool> {
        match text {
            "true" | "True" | "TRUE" => Some(true),
            "false" | "False" | "FALSE" => Some(false),
            _ => None,
        }
    }

    fn arg_pid(a: Option<&String>) -> Option<u32> {
        a.and_then(|s| parse_pid(s))
    }

    pub(super) fn run(ctx: &mut Ctx<'_, '_>, env: &Env, probe: &str, args: &[String]) -> i32 {
        let first = || args.first().map_or("", String::as_str);
        let index = || DesktopIndex::build(&env.app_dirs, Launcher::detect(&env.path));
        let value = match probe {
            "__quote" => serde_json::json!({ "exec": shell::quote_argv(args) }),
            "__split" => serde_json::json!(shell::split_stored(first())),
            "__split-ifs" => serde_json::json!(shell::split_ifs(first())),
            "__nkey" => serde_json::json!({ "key": key::nkey(first()) }),
            "__webapp-url" => serde_json::json!({ "url": webapp::url_from_exec(first()) }),
            "__webapp-ident" => serde_json::json!({ "ident": webapp::identity_from_url(first()) }),
            "__leaf" => serde_json::json!({ "leaf": argv::leaf_for_exec(first()) }),
            "__index-dump" => index().dump(),
            "__trust" => {
                let pid = arg_pid(args.get(1)).unwrap_or(0);
                serde_json::json!({ "trusted": resolve::window_trusted(&env.proc, xwayland(first()), pid) })
            }
            "__resolve" => {
                let pid = arg_pid(args.get(1)).unwrap_or(0);
                let (exec, source) =
                    resolve::resolve_launch(&env.proc, &index(), &env.config_dir, first(), pid);
                serde_json::json!({ "exec": exec.as_str(), "source": source.as_str() })
            }
            "__capture" => {
                let pid = arg_pid(args.get(1)).unwrap_or(0);
                let xw = xwayland(args.get(2).map_or("", String::as_str));
                resolve::capture_resolve(&env.proc, &index(), first(), pid, xw, false).to_json()
            }
            "__label" => {
                let arg = |n: usize| args.get(n).map_or("", String::as_str);
                serde_json::json!({ "label": resolve::display_label(&index(), arg(0), arg(1)) })
            }
            "__lookup" => {
                let idx = index();
                let q = first();
                serde_json::json!({
                    "exec": idx.exec_for_class(q).unwrap_or(""),
                    "launcher": idx.launcher_for_class(q).unwrap_or(""),
                    "classForExec": idx.class_for_exec(q).unwrap_or(""),
                    "nameForLeaf": idx.name_for_leaf(q).unwrap_or(""),
                })
            }
            other => return ctx.not_ported(other),
        };
        ctx.out(&format!("{value}\n"));
        0
    }
}

#[cfg(not(feature = "fixtures"))]
mod probes {
    use super::Ctx;
    use crate::env::Env;

    pub(super) fn run(ctx: &mut Ctx<'_, '_>, _env: &Env, probe: &str, _args: &[String]) -> i32 {
        ctx.not_ported(probe)
    }
}
