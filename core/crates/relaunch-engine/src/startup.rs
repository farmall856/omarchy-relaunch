//! The startup inventory: the `o.launch_on_start` / `o.exec_on_start` lines in
//! Omarchy's `autostart.lua`, read only (Relaunch never writes that file).

use std::path::Path;

use serde::Serialize;

use crate::argv::{basename, is_relaunch_exec, is_wrapper_cmd};
use crate::shell::split_ifs;

/// One autostart line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StartupItem {
    /// `lua:<kind>:<exec>`.
    pub id: String,
    /// The file's name.
    pub source: String,
    /// `launch_on_start` or `exec_on_start`.
    pub kind: String,
    /// The quoted command, verbatim.
    pub exec: String,
    /// The class the command is expected to open (best effort).
    pub class: String,
    /// The 1-based line number.
    pub line: usize,
}

/// The first `o.<call>(<space>*"<command>"` on a line.
fn call_arg<'a>(line: &'a str, call: &str) -> Option<&'a str> {
    let needle = format!("o.{call}(");
    line.match_indices(&needle).find_map(|(at, _)| {
        let rest = line
            .get(at + needle.len()..)?
            .trim_start_matches(|c: char| c.is_ascii_whitespace());
        let body = rest.strip_prefix('"')?;
        let end = body.find('"')?;
        (end > 0).then(|| body.get(..end)).flatten()
    })
}

/// The class a startup command is expected to open, without the desktop
/// index: its first program word.
#[must_use]
pub fn class_for_startup_exec(exec: &str) -> String {
    let argv = split_ifs(exec);
    argv.iter()
        .filter(|t| !t.starts_with('-') && !t.contains('='))
        .map(|t| basename(t))
        .find(|b| !is_wrapper_cmd(b))
        .or_else(|| argv.first().map(String::as_str))
        .unwrap_or_default()
        .to_owned()
}

/// Scan `autostart.lua`; a missing or unreadable file has no items.
#[must_use]
pub fn scan(path: &Path) -> Vec<StartupItem> {
    let Ok(bytes) = relaunch_sys::fs::read_regular_capped(path, relaunch_sys::fs::MAX_FILE_BYTES)
    else {
        return Vec::new();
    };
    let source = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes)
        .split('\n')
        .enumerate()
        .filter(|(_, line)| {
            !line
                .trim_start_matches(|c: char| c.is_ascii_whitespace())
                .starts_with("--")
        })
        .filter_map(|(n, line)| {
            let (kind, exec) = ["launch_on_start", "exec_on_start"]
                .iter()
                .find_map(|k| call_arg(line, k).map(|e| (*k, e)))?;
            (!is_relaunch_exec(exec)).then(|| StartupItem {
                id: format!("lua:{kind}:{exec}"),
                source: source.clone(),
                kind: kind.to_owned(),
                exec: exec.to_owned(),
                class: class_for_startup_exec(exec),
                line: n + 1,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_found_like_the_bash_regex() {
        assert_eq!(
            call_arg(r#"o.launch_on_start("a b")"#, "launch_on_start"),
            Some("a b")
        );
        assert_eq!(
            call_arg(r#"x o.exec_on_start(  "c") y"#, "exec_on_start"),
            Some("c")
        );
        assert_eq!(
            call_arg(r#"o.launch_on_start("")"#, "launch_on_start"),
            None
        );
        assert_eq!(call_arg(r"o.launch_on_start(cmd)", "launch_on_start"), None);
        assert_eq!(
            call_arg(
                r#"o.launch_on_start(x) o.launch_on_start("second")"#,
                "launch_on_start"
            ),
            Some("second")
        );
    }

    #[test]
    fn startup_classes() {
        assert_eq!(class_for_startup_exec("uwsm-app -- idonly"), "idonly");
        assert_eq!(class_for_startup_exec("myapp --flag"), "myapp");
        assert_eq!(
            class_for_startup_exec("xdg-terminal-exec --app-id=TUI.float -e bash -c 'dua i /'"),
            "xdg-terminal-exec",
            "no program is known to host another"
        );
        assert_eq!(class_for_startup_exec("A=1"), "A=1");
        assert!(is_relaunch_exec("/home/u/.local/bin/relaunch boot"));
        assert!(!is_relaunch_exec("relaunchy"));
    }
}
