//! Reading an argv: which words are command-grammar wrappers and assignments,
//! the program it runs, and the leaf command a shell-wrapped argv runs. No
//! word here names an application.

use crate::shell::{split_ifs, split_stored};

/// The last path component (`${word##*/}`); empty for a trailing `/`.
#[must_use]
pub fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// A wrapper that runs the command following it.
#[must_use]
pub fn is_wrapper_cmd(word: &str) -> bool {
    matches!(
        basename(word),
        "setsid" | "env" | "command" | "nice" | "nohup" | "uwsm-app"
    )
}

/// A `NAME=value`-shaped word (the bash glob `[A-Za-z_]*[=]*`, not `--*`).
#[must_use]
pub fn is_assignment_like(word: &str) -> bool {
    let mut chars = word.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.as_str().contains('=')
}

/// Words before the program: wrappers, `--`, and assignment-like words.
fn is_prefix(word: &str) -> bool {
    is_wrapper_cmd(word) || word == "--" || is_assignment_like(word)
}

/// The program an argv runs once wrappers, `--` and assignments are seen
/// through, by basename. `None` when there is none.
#[must_use]
pub fn program(argv: &[String]) -> Option<&str> {
    argv.iter().find(|w| !is_prefix(w)).map(|w| basename(w))
}

/// The command a stored exec string runs once wrappers and `sh -c` are seen
/// through, by basename. `None` when there is no program.
#[must_use]
pub fn leaf_for_exec(exec: &str) -> Option<String> {
    let argv = split_stored(exec);
    if argv.is_empty() {
        return None;
    }
    leaf_from_argv(&argv)
}

/// [`leaf_for_exec`] over an argv.
#[must_use]
pub fn leaf_from_argv(argv: &[String]) -> Option<String> {
    let start = argv.iter().take_while(|w| is_prefix(w)).count();
    let (first, more) = argv.get(start..).unwrap_or_default().split_first()?;
    if matches!(basename(first), "bash" | "sh" | "zsh" | "dash")
        && more.first().map(String::as_str) == Some("-c")
    {
        if let Some(script) = more.get(1).filter(|s| !s.is_empty()) {
            let word = script.split(' ').next().unwrap_or("");
            if !word.is_empty() {
                return Some(basename(word).to_owned());
            }
        }
    }
    Some(basename(first).to_owned())
}

/// A command that is Relaunch itself (its boot hook): never inventoried as a
/// startup app, never launched by boot (that would recurse).
#[must_use]
pub fn is_relaunch_exec(exec: &str) -> bool {
    exec.contains("omarchy-relaunch") || split_ifs(exec).iter().any(|w| basename(w) == "relaunch")
}
