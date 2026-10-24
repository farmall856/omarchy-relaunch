//! Shell words: the ONE way a value becomes part of a shell-string `exec`
//! (INV-8), and the engine's two splitters for exec strings it reads back.
//!
//! A shell-string `exec` is run at login by bash (`eval`, spec §8), so every
//! argument must reach it as exactly one word. [`quote_word`] reproduces bash's
//! `printf %q` byte for byte for ASCII (D-P2(a)), so commands saved by the
//! bash engine compare equal and never show as changed. Non-ASCII words use a
//! single-quoted form instead of locale-dependent `%q` output; both are
//! bash-correct, which the argv round-trip tests check.

/// A shell-string launch command. Built only from an argv ([`Exec::from_argv`]),
/// from a class ([`Exec::guess`]), or taken verbatim from a stored document or
/// what the user typed ([`Exec::stored`]). There is no way to interpolate a
/// field into a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exec(String);

impl Exec {
    /// The command that runs `argv` exactly, one shell word per argument. A
    /// program bash would read as a keyword, a builtin or an assignment is run
    /// through `exec --`, which only ever executes a program.
    #[must_use]
    pub fn from_argv<S: AsRef<str>>(argv: &[S]) -> Self {
        let words = quote_argv(argv);
        match argv.first() {
            Some(p) if names_shell_syntax(p.as_ref()) => Self(format!("exec -- {words}")),
            _ => Self(words),
        }
    }

    /// The last-resort command for a window class: the lowercased class as ONE
    /// program word (Sol #2: never shell syntax).
    #[must_use]
    pub fn guess(class: &str) -> Self {
        Self::from_argv(&[class.to_lowercase()])
    }

    /// A command that is already a shell string: read from a document, or typed
    /// by the user (an `overrides-table` entry is never re-quoted).
    #[must_use]
    pub fn stored(command: String) -> Self {
        Self(command)
    }

    /// The command text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The command text, by value.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// GNU bash 5.3's builtins and reserved words (`compgen -b`, `compgen -k`).
pub const BASH_BUILTINS: &[&str] = &[
    ".",
    ":",
    "[",
    "alias",
    "bg",
    "bind",
    "break",
    "builtin",
    "caller",
    "cd",
    "command",
    "compgen",
    "complete",
    "compopt",
    "continue",
    "declare",
    "dirs",
    "disown",
    "echo",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "getopts",
    "hash",
    "help",
    "history",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "wait",
    "if",
    "then",
    "else",
    "elif",
    "fi",
    "case",
    "esac",
    "for",
    "select",
    "while",
    "until",
    "do",
    "done",
    "in",
    "function",
    "time",
    "{",
    "}",
    "!",
    "[[",
    "]]",
    "coproc",
];

/// `NAME=value` as bash reads an assignment prefix.
#[must_use]
pub fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A command word bash would not look up as a program on PATH: a builtin, a
/// reserved word, or an assignment (a word naming a path never is).
#[must_use]
pub fn names_shell_syntax(word: &str) -> bool {
    !word.contains('/') && (BASH_BUILTINS.contains(&word) || is_assignment(word))
}

/// `printf %q` of one word.
#[must_use]
pub fn quote_word(word: &str) -> String {
    if word.is_empty() {
        return "''".into();
    }
    if !word.is_ascii() {
        return single_quote(word);
    }
    if word.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return ansi_c_quote(word);
    }
    let mut out = String::with_capacity(word.len() + 4);
    let mut prev = None;
    for (i, c) in word.chars().enumerate() {
        let escape = match c {
            ' ' | '!' | '"' | '$' | '&' | '\'' | '(' | ')' | '*' | ',' | ';' | '<' | '>' | '?'
            | '[' | '\\' | ']' | '^' | '`' | '{' | '|' | '}' => true,
            '#' => i == 0,
            '~' => i == 0 || matches!(prev, Some('=' | ':')),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// The words of `argv`, each quoted, joined by single spaces (bash
/// `quote_argv`).
#[must_use]
pub fn quote_argv<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|w| quote_word(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bash's `$'…'` form, used for an ASCII word holding a control character.
fn ansi_c_quote(word: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("$'");
    for b in word.bytes() {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            0x1b => out.push_str("\\E"),
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push('\'');
    out
}

/// A POSIX single-quoted word: everything literal, `'` written as `'\''`.
fn single_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Split a stored exec string into words the way the bash engine's
/// `unquote_argv_arr` does (used for labels, leaf commands and validation;
/// login itself runs the string through bash).
///
/// With no quote or backslash present, only the FIRST LINE is split, on runs of
/// spaces and tabs. Otherwise `'…'` and `"…"` quote (inside `"…"` a backslash
/// takes the next character literally), an unquoted backslash takes the next
/// character literally, and spaces/tabs separate words. A newline is an
/// ordinary character on this path, and an unterminated quote runs to the end.
#[must_use]
pub fn split_stored(s: &str) -> Vec<String> {
    if !s.contains(['\\', '\'', '"']) {
        let first = s.split('\n').next().unwrap_or("");
        return first
            .split([' ', '\t'])
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && c == '\\' && it.peek().is_some() {
                cur.extend(it.next());
            } else {
                cur.push(c);
            }
            started = true;
            continue;
        }
        match c {
            '\\' => {
                cur.extend(it.next());
                started = true;
            }
            '\'' | '"' => {
                quote = Some(c);
                started = true;
            }
            ' ' | '\t' => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            _ => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Split an exec string on runs of spaces, tabs and newlines, with quotes and
/// backslashes literal (bash `split_exec_words`: unquoted word splitting with
/// globbing off).
#[must_use]
pub fn split_ifs(s: &str) -> Vec<String> {
    s.split([' ', '\t', '\n'])
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// `exec` run by `bash -c` (as the anchor runs it) with `dir` first on PATH:
    /// what the program that ran saw, `argv[0]` basename first.
    fn run_in(dir: &std::path::Path, exec: &str) -> String {
        let out = std::process::Command::new("/usr/bin/bash")
            .arg("-c")
            .arg(exec)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn the_program_in_command_position_is_the_one_that_runs() {
        let dir =
            std::env::temp_dir().join(format!("relaunch-shell-cmdpos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Programs whose names bash would otherwise read as a keyword, a
        // builtin or an assignment.
        for name in ["if", "time", "echo", "cd", "FOO=bar", "[[", "!", "{"] {
            let p = dir.join(name);
            std::fs::write(
                &p,
                "#!/bin/sh\nprintf 'ran:%s' \"${0##*/}\"; printf ' [%s]' \"$@\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            let exec = Exec::from_argv(&[name, "a b", "$x"]);
            assert_eq!(
                run_in(&dir, exec.as_str()),
                format!("ran:{name} [a b] [$x]"),
                "{name}: {}",
                exec.as_str()
            );
        }
        // An ordinary program keeps the bash engine's exact form.
        assert_eq!(
            Exec::from_argv(&["/usr/bin/foot", "-e", "htop"]).as_str(),
            "/usr/bin/foot -e htop"
        );
        assert_eq!(Exec::from_argv(&["foot"]).as_str(), "foot");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
