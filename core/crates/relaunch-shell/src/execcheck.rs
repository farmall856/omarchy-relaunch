//! Whether a stored command would run: the one verdict behind the panel's
//! "command not found" warnings and the boot log's.
//!
//! The program word is found the way `bash -c` finds it — past assignments and
//! the wrapper commands — and judged in the mode it would be run in: by the
//! shell ([`Mode::Shell`]: a builtin or keyword runs) or by `execve`
//! ([`Mode::Direct`]: only an executable file does, as after `env`, `exec` or
//! for a launcher's argv). The words are read with the quoting the generator
//! writes (`'…'`, `"…"`, `\x`, `$'…'`); any other shell syntax is
//! [`Program::Unjudged`], never a rejection.

use std::path::Path;

use crate::argv::{basename, is_wrapper_cmd};
use crate::shell::{is_assignment, BASH_BUILTINS};

/// How the program word would be executed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// By bash: a builtin or keyword runs as well as a program.
    Shell,
    /// By `execve` (directly, or through `exec`/`env`/a wrapper): only an
    /// executable file runs.
    Direct,
}

/// The program word of a stored command.
#[derive(Debug, PartialEq, Eq)]
pub enum Program {
    /// The command has no words at all, or does not parse (an unterminated
    /// quote): bash would run nothing.
    Nothing,
    /// A form not judged: shell syntax this reader does not model, a `PATH`
    /// assignment, all prefix and no program, or an option to a wrapper.
    Unjudged,
    /// The word that names the program, and how it would be run.
    Word(String, Mode),
}

/// The words of `exec`, decoded as bash would: `None` for syntax this reader
/// does not model (expansions, operators, globs, redirections), `Some(Err)` for
/// an unterminated quote.
fn words(exec: &str) -> Option<Result<Vec<String>, ()>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            ' ' | '\t' => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Some(Err(())),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => match it.next() {
                            Some(c @ ('$' | '`' | '"' | '\\')) => cur.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Some(Err(())),
                        },
                        Some('$' | '`') => return None,
                        Some(c) => cur.push(c),
                        None => return Some(Err(())),
                    }
                }
            }
            '\\' => {
                started = true;
                match it.next() {
                    Some('\n') => {}
                    Some(c) => cur.push(c),
                    None => return None,
                }
            }
            '$' if it.peek() == Some(&'\'') => {
                it.next();
                started = true;
                if ansi_c(&mut it, &mut cur).is_err() {
                    return Some(Err(()));
                }
            }
            '$' | '`' | '|' | '&' | ';' | '<' | '>' | '(' | ')' | '\n' | '*' | '?' | '[' => {
                // `[` alone is the test builtin, not a glob.
                if c == '[' && !started && matches!(it.peek(), None | Some(' ' | '\t')) {
                    cur.push(c);
                    started = true;
                    continue;
                }
                return None;
            }
            '#' if !started => break,
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    Some(Ok(out))
}

/// The body of a `$'…'` word after its opening quote, decoded into `cur`:
/// the escapes [`crate::shell::quote_word`] writes, plus octal.
fn ansi_c(it: &mut std::iter::Peekable<std::str::Chars<'_>>, cur: &mut String) -> Result<(), ()> {
    loop {
        match it.next().ok_or(())? {
            '\'' => return Ok(()),
            '\\' => {
                let e = it.next().ok_or(())?;
                let decoded = match e {
                    'a' => '\x07',
                    'b' => '\x08',
                    't' => '\t',
                    'n' => '\n',
                    'v' => '\x0b',
                    'f' => '\x0c',
                    'r' => '\r',
                    'e' | 'E' => '\x1b',
                    '0'..='7' => {
                        let mut n = e.to_digit(8).ok_or(())?;
                        for _ in 0..2 {
                            match it.peek().and_then(|c| c.to_digit(8)) {
                                Some(d) => {
                                    n = n * 8 + d;
                                    it.next();
                                }
                                None => break,
                            }
                        }
                        char::from_u32(n & 0xff).ok_or(())?
                    }
                    other => other,
                };
                cur.push(decoded);
            }
            c => cur.push(c),
        }
    }
}

/// The argv a typed command line runs when it is plain words — quoted the
/// way [`crate::shell::quote_word`] writes, with no expansion, operator, glob,
/// assignment or builtin — and so can run without a shell. `None` when it
/// needs bash.
#[must_use]
pub fn plain_argv(text: &str) -> Option<Vec<String>> {
    let argv = words(text)?.ok()?;
    let first = argv.first()?;
    (!crate::shell::names_shell_syntax(first)).then_some(argv)
}

/// Find the program word of `exec`.
#[must_use]
pub fn program(exec: &str) -> Program {
    #[derive(PartialEq)]
    enum Reading {
        Shell,
        Env,
        Wrapper,
        Exec,
    }
    let words = match words(exec) {
        None => return Program::Unjudged,
        Some(Err(())) => return Program::Nothing,
        Some(Ok(w)) => w,
    };
    if words.is_empty() {
        return Program::Nothing;
    }
    let mut reading = Reading::Shell;
    let mut mode = Mode::Shell;
    for word in words {
        if reading == Reading::Exec && word == "--" {
            reading = Reading::Wrapper;
            continue;
        }
        if reading != Reading::Shell && word.starts_with('-') {
            return Program::Unjudged;
        }
        if reading != Reading::Wrapper && is_assignment(&word) {
            if word.starts_with("PATH=") {
                return Program::Unjudged;
            }
            continue;
        }
        if reading == Reading::Shell && word == "exec" {
            reading = Reading::Exec;
            mode = Mode::Direct;
            continue;
        }
        if is_wrapper_cmd(&word) {
            reading = if basename(&word) == "env" {
                Reading::Env
            } else {
                Reading::Wrapper
            };
            if basename(&word) != "command" {
                mode = Mode::Direct;
            }
            continue;
        }
        return Program::Word(word, mode);
    }
    Program::Unjudged
}

/// Whether `exec` would run under `bash -c` with this `PATH`.
#[must_use]
pub fn exec_is_ok(exec: &str, path: &str) -> bool {
    exec_is_ok_in(exec, path, Mode::Shell)
}

/// Whether `exec` would run with this `PATH` when run in `mode`: a desktop
/// launcher's canonical `Exec` is run as argv ([`Mode::Direct`]), so its
/// program must be a file even where the shell would find a builtin.
#[must_use]
pub fn exec_is_ok_in(exec: &str, path: &str, mode: Mode) -> bool {
    match program(exec) {
        Program::Nothing => false,
        Program::Unjudged => true,
        Program::Word(word, found) => {
            let mode = if mode == Mode::Direct {
                Mode::Direct
            } else {
                found
            };
            word_is_ok(&word, path, mode)
        }
    }
}

/// Whether the program word `word` would run in `mode`: in [`Mode::Shell`] a
/// bash builtin or keyword, and in either mode an executable regular file by
/// path or on `PATH`. The one judgement behind both [`exec_is_ok`] and a
/// launcher's argv ([`Mode::Direct`]).
#[must_use]
pub fn word_is_ok(word: &str, path: &str, mode: Mode) -> bool {
    if mode == Mode::Shell && BASH_BUILTINS.contains(&word) {
        return true;
    }
    if word.contains('/') {
        return relaunch_sys::fs::is_executable_file(Path::new(word));
    }
    path.split(':')
        .map(|dir| if dir.is_empty() { "." } else { dir })
        .any(|dir| relaunch_sys::fs::is_executable_file(&Path::new(dir).join(word)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Exec;

    #[test]
    fn the_program_word_is_past_assignments_and_wrappers() {
        let sh = |s: &str| Program::Word(s.to_owned(), Mode::Shell);
        let direct = |s: &str| Program::Word(s.to_owned(), Mode::Direct);
        assert_eq!(program("FOO=1 app --x"), sh("app"));
        assert_eq!(program("env A=1 B=2 app"), direct("app"));
        assert_eq!(program("setsid nice app"), direct("app"));
        assert_eq!(program("command cd"), sh("cd"));
        assert_eq!(program("nice A=1"), direct("A=1"));
        assert_eq!(program("exec -- cd /"), direct("cd"));
        assert_eq!(program("uwsm-app -- idonly"), Program::Unjudged);
        assert_eq!(program("FOO=bar"), Program::Unjudged);
        assert_eq!(program("env"), Program::Unjudged);
        assert_eq!(program(""), Program::Nothing);
        assert_eq!(program("'my app' --flag"), sh("my app"));
    }

    #[test]
    fn the_generators_quoting_is_decoded() {
        let sh = |s: &str| Program::Word(s.to_owned(), Mode::Shell);
        for argv in [
            vec!["/opt/my app/run", "x"],
            vec!["a\tb\x1bc", "y"],
            vec!["it's", "\"q\""],
            vec!["\u{e9}t\u{e9}"],
        ] {
            let exec = Exec::from_argv(&argv);
            assert_eq!(program(exec.as_str()), sh(argv[0]), "{}", exec.as_str());
        }
        assert_eq!(program(r"$'\101\102' x"), sh("AB"));
        assert_eq!(program("'unterminated"), Program::Nothing);
    }

    #[test]
    fn shell_syntax_it_does_not_model_is_unjudged_not_rejected() {
        for exec in [
            "PATH=/opt/tool/bin tool",
            "$HOME/bin/app",
            "app | tee log",
            "cd /x && app",
            "\"$APP\" --x",
            "`which app`",
            "app*",
            "(app)",
        ] {
            assert_eq!(program(exec), Program::Unjudged, "{exec}");
            assert!(exec_is_ok(exec, ""), "{exec}");
        }
        assert_eq!(
            program("[ -x /bin/sh ]"),
            Program::Word("[".into(), Mode::Shell)
        );
    }

    #[test]
    fn plain_words_are_an_argv_and_shell_syntax_is_not() {
        let w = |v: &[&str]| Some(v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(plain_argv("proton-mail"), w(&["proton-mail"]));
        assert_eq!(
            plain_argv("foot -e htop -d 5"),
            w(&["foot", "-e", "htop", "-d", "5"])
        );
        assert_eq!(
            plain_argv("'/opt/My App/run' --x"),
            w(&["/opt/My App/run", "--x"])
        );
        assert_eq!(plain_argv(r"a\ b c"), w(&["a b", "c"]));
        for shell in [
            "app | tee log",
            "cd /x && app",
            "$HOME/bin/app",
            "app *.txt",
            "FOO=1 app",
            "cd /tmp",
            "'unterminated",
            "",
            "   ",
        ] {
            assert_eq!(plain_argv(shell), None, "{shell:?}");
        }
        // The panel shows an argv with the generator's quoting; it reads back.
        for argv in [vec!["a b", "c"], vec!["it's", "\"q\""], vec!["x;y", "$z"]] {
            let argv: Vec<String> = argv.into_iter().map(String::from).collect();
            assert_eq!(
                plain_argv(&crate::shell::quote_argv(&argv)),
                Some(argv.clone()),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn ok_means_bash_would_run_it() {
        let path = "/usr/bin:/bin";
        assert!(exec_is_ok("sh -c true", path));
        assert!(exec_is_ok("/bin/sh", path));
        assert!(exec_is_ok("cd /tmp", path));
        assert!(exec_is_ok("uwsm-app -- anything", path));
        assert!(!exec_is_ok("definitely-not-a-command-xyz", path));
        assert!(!exec_is_ok("/tmp", path), "a directory is not a program");
        assert!(!exec_is_ok("", path));
        assert!(!exec_is_ok("sh", ""), "an empty PATH searches only the cwd");
    }

    #[test]
    fn a_builtin_runs_only_under_the_shell() {
        let path = "/usr/bin:/bin";
        assert!(word_is_ok("cd", path, Mode::Shell));
        assert!(
            !word_is_ok("cd", path, Mode::Direct),
            "execve cannot run a builtin"
        );
        assert!(!exec_is_ok("env cd /", path));
        assert!(!exec_is_ok("exec -- cd /", path));
        assert!(exec_is_ok(
            Exec::from_argv(&["sh", "-c", "x"]).as_str(),
            path
        ));
    }
}
