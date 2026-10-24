//! The one Desktop Entry parser (INV-2). The login anchor uses it to verify a
//! launcher at boot and the engine uses it at capture, so the two can never
//! disagree about what a `.desktop` file says.
//!
//! Two layers:
//! - **File format** — groups, keys, locale variants and string escapes — is
//!   decoded by the [`freedesktop-desktop-entry`] crate (MPL-2.0, System76 /
//!   pop-os, used unmodified; see `THIRD-PARTY-NOTICES.md`). A file it cannot
//!   decode is refused as a whole.
//! - **Exec policy** is ours: quoting with `"` and the in-quote escapes of
//!   `"`, a backtick, `$` and `\`; the field codes `%f %F %u %U` and the
//!   deprecated `%d %D %n %N %v %m` expand to nothing and `%%` to `%`; `%c`,
//!   `%i` and `%k` are REFUSED (they would substitute file-derived text into the
//!   argv, and `%k` would differ between capture and boot), as is any unknown
//!   code, an unterminated quote, a backslash outside quotes and any other
//!   in-quote escape. The library's own Exec substitution is not used for that
//!   reason. A `Type` other than `Application`, and a `Terminal` or
//!   `DBusActivatable` that is not exactly `true` or `false`, is refused.
//!
//! It parses the launcher buffer the anchor hashed, so the argv that is
//! `execve`'d is exactly what was in the approved bytes — never shell, never a
//! `gio`/`gtk-launch` reopen. `DBusActivatable`/`Terminal` launchers are
//! refused by [`canonical_exec`].
//!
//! [`freedesktop-desktop-entry`]: https://github.com/pop-os/freedesktop-desktop-entry
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code: a failed unwrap/index is a failed assertion"
    )
)]

/// The parsed, relevant contents of a `[Desktop Entry]` group.
pub struct DesktopEntry {
    /// The `Exec` argv with field codes expanded.
    pub argv: Vec<String>,
    /// `DBusActivatable=true`: the entry must not be run as a command.
    pub dbus_activatable: bool,
    /// `Terminal=true`: the entry must not be run as a command.
    pub terminal: bool,
    /// `Path=`, unescaped: the working directory to launch in.
    pub path: Option<String>,
}

/// Parse the `[Desktop Entry]` group: `Exec` into argv (with field codes
/// expanded), plus the keys that change how it must be launched.
///
/// # Errors
///
/// A message for no `Exec`, an unterminated quote, a lone trailing
/// backslash, a refused field code, or an `Exec` that expands to nothing.
pub fn parse_entry(buffer: &str) -> Result<DesktopEntry, String> {
    let entry = decode(buffer)?;
    if !matches!(entry.desktop_entry("Type"), None | Some("Application")) {
        return Err("Type is not Application".into());
    }
    let flag = |key: &str| match entry.desktop_entry(key) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(format!("{key} is not true or false")),
    };
    let (dbus_activatable, terminal) = (flag("DBusActivatable")?, flag("Terminal")?);
    let raw = entry
        .desktop_entry("Exec")
        .ok_or_else(|| "no Exec in [Desktop Entry]".to_string())?;
    // The library has already undone the string escapes; quoting and field
    // codes are our policy (spec §8).
    let tokens = tokenize(raw)?;
    let argv = expand_field_codes(tokens)?;
    if argv.is_empty() {
        return Err("Exec expanded to nothing".into());
    }
    Ok(DesktopEntry {
        argv,
        dbus_activatable,
        terminal,
        path: entry
            .desktop_entry("Path")
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
    })
}

/// Parse just the argv (convenience / tests).
///
/// # Errors
///
/// As [`parse_entry`].
pub fn parse_exec(buffer: &str) -> Result<Vec<String>, String> {
    Ok(parse_entry(buffer)?.argv)
}

/// A deterministic, injective rendering of an argv used to compare the live
/// launcher's `Exec` against the signed `exec`. Not meant to be shell-parsed;
/// only compared for equality. Backslash and space are escaped so distinct
/// argvs never collide, and the result carries no control characters.
#[must_use]
pub fn canonical_exec_string(argv: &[String]) -> String {
    argv.iter()
        .map(|a| a.replace('\\', "\\\\").replace(' ', "\\ "))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The argv a [`canonical_exec_string`] was made from: split at each
/// unescaped space, exactly (an empty argument stays one), undoing `\ ` and
/// `\\`. `split_canonical_exec(&canonical_exec_string(a)) == a` for every
/// non-empty argv.
#[must_use]
pub fn split_canonical_exec(exec: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let (Some(next), Some(cur)) = (chars.next(), out.last_mut()) {
                    cur.push(next);
                }
            }
            ' ' => out.push(String::new()),
            c => {
                if let Some(cur) = out.last_mut() {
                    cur.push(c);
                }
            }
        }
    }
    out
}

/// The argv of a `.desktop` file's `Exec`, for capture to copy into a pin's
/// private launcher. The file is resolved, read bounded as a regular file,
/// parsed with [`parse_entry`], and refused if it is `DBusActivatable` or
/// `Terminal=true` (it cannot be relaunched as a command).
///
/// # Errors
///
/// A message if the path cannot be resolved or read, is not UTF-8, has no
/// usable `Exec`, or is a refused launcher kind.
pub fn exec_argv_file(path: &str) -> Result<Vec<String>, String> {
    let resolved = std::fs::canonicalize(path).map_err(|e| format!("resolve {path}: {e}"))?;
    let buf = relaunch_sys::fs::read_regular_capped(&resolved, relaunch_sys::fs::MAX_FILE_BYTES)?;
    let text = String::from_utf8(buf).map_err(|_| "launcher is not valid UTF-8".to_string())?;
    exec_argv(&text)
}

/// [`exec_argv_file`] over a buffer already read.
///
/// # Errors
///
/// As [`exec_argv_file`], for the parse and refusal steps.
pub fn exec_argv(buffer: &str) -> Result<Vec<String>, String> {
    let entry = parse_entry(buffer)?;
    if entry.dbus_activatable {
        return Err("DBusActivatable launcher cannot be relaunched as a command".into());
    }
    if entry.terminal {
        return Err("Terminal=true launcher cannot be relaunched as a command".into());
    }
    Ok(entry.argv)
}

/// The canonical `exec` of a `.desktop` file: [`exec_argv_file`], rendered.
///
/// # Errors
///
/// As [`exec_argv_file`].
pub fn canonical_exec_file(path: &str) -> Result<String, String> {
    exec_argv_file(path).map(|argv| canonical_exec_string(&argv))
}

/// [`canonical_exec_file`] over a buffer already read.
///
/// # Errors
///
/// As [`exec_argv_file`], for the parse and refusal steps.
pub fn canonical_exec(buffer: &str) -> Result<String, String> {
    exec_argv(buffer).map(|argv| canonical_exec_string(&argv))
}

/// The untranslated `Name` of a `.desktop` file (labels only), read with the
/// shared rules; `None` if unreadable or absent.
#[must_use]
pub fn name_from_file(path: &str) -> Option<String> {
    let buf = relaunch_sys::fs::read_regular_capped(
        std::path::Path::new(path),
        relaunch_sys::fs::MAX_FILE_BYTES,
    )
    .ok()?;
    let entry = decode(&String::from_utf8_lossy(&buf)).ok()?;
    entry
        .desktop_entry("Name")
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// The `[Desktop Entry]` keys the engine's desktop index uses, decoded by the
/// same file-format layer as [`parse_entry`] (string escapes undone, a
/// localized key such as `Name[de]=` kept apart from its base key). A file that
/// does not decode yields no keys.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexKeys {
    /// The raw `Exec` value, if any.
    pub exec: Option<String>,
    /// The untranslated `Name`.
    pub name: Option<String>,
    /// `StartupWMClass`: the window class the entry's windows carry.
    pub startup_wm_class: Option<String>,
    /// `Hidden=true`: the entry is deleted and must not be offered.
    pub hidden: bool,
    /// `Categories` names `TerminalEmulator`: the entry is a terminal.
    pub terminal_emulator: bool,
    /// `X-TerminalArgExec`: the option a terminal takes before the command
    /// it should run; empty when the command follows with no option.
    pub terminal_arg_exec: Option<String>,
}

/// Read the index keys of a `.desktop` buffer.
#[must_use]
pub fn index_keys(buffer: &str) -> IndexKeys {
    let Ok(entry) = decode(buffer) else {
        return IndexKeys::default();
    };
    let get = |key: &str| entry.desktop_entry(key).map(str::to_owned);
    IndexKeys {
        exec: get("Exec"),
        name: get("Name"),
        startup_wm_class: get("StartupWMClass"),
        hidden: entry.desktop_entry("Hidden") == Some("true"),
        terminal_emulator: entry
            .desktop_entry("Categories")
            .is_some_and(|c| c.split(';').any(|k| k == "TerminalEmulator")),
        terminal_arg_exec: get("X-TerminalArgExec"),
    }
}

/// Decode a `.desktop` buffer with `freedesktop-desktop-entry` (the file
/// format: groups, keys, locale variants, string escapes). A file it cannot
/// decode is refused as a whole.
fn decode(buffer: &str) -> Result<freedesktop_desktop_entry::DesktopEntry, String> {
    // The path only names the entry's app id; nothing is read from it.
    freedesktop_desktop_entry::DesktopEntry::from_str("entry.desktop", buffer, None::<&[&str]>)
        .map_err(|e| format!("not a valid desktop entry: {e}"))
}

/// Tokenize honoring `"`-quoting and the reserved in-quote escapes.
fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut in_quote = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quote {
            match c {
                '"' => in_quote = false,
                '\\' => match chars.next() {
                    Some(n @ ('"' | '`' | '$' | '\\')) => cur.push(n),
                    Some(_) => return Err("undefined escape in a quoted Exec argument".into()),
                    None => return Err("unterminated quote in Exec".into()),
                },
                other => cur.push(other),
            }
        } else {
            match c {
                '"' => {
                    in_quote = true;
                    have = true;
                }
                ' ' | '\t' | '\n' | '\r' => {
                    if have {
                        tokens.push(std::mem::take(&mut cur));
                        have = false;
                    }
                }
                '\\' => return Err("backslash outside a quoted Exec argument".into()),
                other => {
                    cur.push(other);
                    have = true;
                }
            }
        }
    }
    if in_quote {
        return Err("unterminated quote in Exec".into());
    }
    if have {
        tokens.push(cur);
    }
    Ok(tokens)
}

/// Expand field codes within each token (reviewer M6/I5). Relaunch starts apps
/// at login with no files/URLs, so the argument-producing codes expand to
/// nothing; `%%` becomes `%`; an unknown `%x` is rejected. A token that was
/// purely a field code (and so expands to empty) is dropped, but a token with
/// other literal text — e.g. `--file=%f` → `--file=` — is kept, rather than the
/// whole token being discarded.
fn expand_field_codes(tokens: Vec<String>) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        let mut expanded = String::with_capacity(t.len());
        let mut had_code = false;
        let mut chars = t.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                expanded.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => expanded.push('%'),
                // File/URL codes (no login file to inject) and the deprecated
                // codes the spec says to remove expand to nothing.
                Some('f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm') => had_code = true,
                // Informational codes (%c Name, %i Icon, %k location) are not
                // equivalent to an absent file argument; refuse them rather than
                // silently change meaning (reviewer #7).
                Some(c @ ('c' | 'i' | 'k')) => {
                    return Err(format!("unsupported field code %{c} in Exec"))
                }
                Some(other) => return Err(format!("invalid field code %{other} in Exec")),
                None => return Err("trailing % in Exec".into()),
            }
        }
        // Drop a token that was only a field code; keep anything with literal text.
        if expanded.is_empty() && had_code {
            continue;
        }
        out.push(expanded);
    }
    Ok(out)
}

#[cfg(test)]
mod canonical_exec_tests {

    #[test]
    fn the_canonical_exec_splits_back_to_its_argv() {
        for argv in [
            vec!["a"],
            vec!["a", ""],
            vec!["", "b"],
            vec!["a", "", "", "b"],
            vec!["/opt/My App/run", "--x=1 2"],
            vec!["back\\slash", "\\", " ", "\\ "],
            vec!["\u{e9}t\u{e9}", "tab\there"],
        ] {
            let argv: Vec<String> = argv.into_iter().map(String::from).collect();
            assert_eq!(
                super::split_canonical_exec(&super::canonical_exec_string(&argv)),
                argv,
                "{argv:?}"
            );
        }
    }
    use super::canonical_exec as canonical_exec_from_buffer;

    #[test]
    fn drops_field_codes_and_canonicalizes() {
        let d = "[Desktop Entry]\nExec=libreoffice --calc %U\nTerminal=false\n";
        assert_eq!(canonical_exec_from_buffer(d).unwrap(), "libreoffice --calc");
    }

    #[test]
    fn escapes_spaces_so_argv_is_injective() {
        // A token containing a space must not collide with two tokens: canonical
        // form escapes the embedded space.
        let d = "[Desktop Entry]\nExec=app \"a b\"\n";
        assert_eq!(canonical_exec_from_buffer(d).unwrap(), "app a\\ b");
    }

    #[test]
    fn refuses_dbus_activatable() {
        let d = "[Desktop Entry]\nExec=foo %f\nDBusActivatable=true\n";
        assert!(canonical_exec_from_buffer(d)
            .unwrap_err()
            .contains("DBusActivatable"));
    }

    #[test]
    fn refuses_terminal() {
        let d = "[Desktop Entry]\nExec=htop\nTerminal=true\n";
        assert!(canonical_exec_from_buffer(d)
            .unwrap_err()
            .contains("Terminal"));
    }

    #[test]
    fn refuses_a_type_other_than_application() {
        for t in ["Link", "Directory", "application", ""] {
            let d = format!("[Desktop Entry]\nType={t}\nExec=foo\n");
            assert!(canonical_exec_from_buffer(&d).is_err(), "Type={t}");
        }
        let d = "[Desktop Entry]\nType=Application\nExec=foo\n";
        assert_eq!(canonical_exec_from_buffer(d).unwrap(), "foo");
    }

    #[test]
    fn refuses_a_boolean_that_is_not_true_or_false() {
        for key in ["Terminal", "DBusActivatable"] {
            for v in ["True", "1", "yes", ""] {
                let d = format!("[Desktop Entry]\nExec=foo\n{key}={v}\n");
                assert!(canonical_exec_from_buffer(&d).is_err(), "{key}={v}");
            }
            let d = format!("[Desktop Entry]\nExec=foo\n{key}=false\n");
            assert!(canonical_exec_from_buffer(&d).is_ok(), "{key}=false");
        }
    }

    #[test]
    fn refuses_escapes_the_exec_rules_do_not_define() {
        // After string-unescaping: an unquoted backslash, an unknown in-quote
        // escape, and a backslash ending a quoted argument.
        for exec in [r"foo a\\ b", r#"foo "a\\nb""#, r#"foo "a\\"#] {
            let d = format!("[Desktop Entry]\nExec={exec}\n");
            assert!(canonical_exec_from_buffer(&d).is_err(), "Exec={exec}");
        }
        let d = "[Desktop Entry]\nExec=foo \"a\\\\$b\"\n";
        assert_eq!(canonical_exec_from_buffer(d).unwrap(), "foo a$b");
    }

    #[test]
    fn errors_when_no_exec() {
        let d = "[Desktop Entry]\nName=NoExec\n";
        assert!(canonical_exec_from_buffer(d).is_err());
    }
}

/// The file-format behaviour this crate relies on from the library. A version
/// bump that changes any of it fails here, not silently at boot.
#[cfg(test)]
mod file_format_tests {
    use super::*;

    fn exec(buffer: &str) -> Result<Vec<String>, String> {
        parse_exec(buffer)
    }

    #[test]
    fn string_escapes_are_undone_before_exec_quoting() {
        // `\s` is a space by the time Exec is split, so it separates words;
        // only `"…"` keeps a space inside one.
        assert_eq!(
            exec("[Desktop Entry]\nExec=a\\sb c\n").unwrap(),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            exec("[Desktop Entry]\nExec=\"a\\sb\" c\n").unwrap(),
            vec!["a b", "c"]
        );
        assert_eq!(
            index_keys("[Desktop Entry]\nName=Tab\\there\n")
                .name
                .as_deref(),
            Some("Tab\there")
        );
    }

    #[test]
    fn whitespace_around_the_separator_is_ignored() {
        assert_eq!(
            exec("[Desktop Entry]\nExec = app  \n").unwrap(),
            vec!["app"]
        );
    }

    #[test]
    fn crlf_line_endings_decode() {
        assert_eq!(
            exec("[Desktop Entry]\r\nExec=app\r\n").unwrap(),
            vec!["app"]
        );
    }

    #[test]
    fn a_localized_key_is_not_its_base_key() {
        let k = index_keys("[Desktop Entry]\nName=Base\nName[de]=Basis\n");
        assert_eq!(k.name.as_deref(), Some("Base"));
        assert!(exec("[Desktop Entry]\nExec[de]=app\n").is_err());
    }

    #[test]
    fn only_the_desktop_entry_group_is_read() {
        let k = index_keys("[Desktop Action x]\nName=Action\n[Desktop Entry]\nName=Main\n");
        assert_eq!(k.name.as_deref(), Some("Main"));
    }

    #[test]
    fn a_duplicate_key_takes_the_last_value() {
        assert_eq!(
            exec("[Desktop Entry]\nExec=first\nExec=second\n").unwrap(),
            vec!["second"]
        );
    }

    #[test]
    fn hidden_is_exactly_the_key_hidden_with_value_true() {
        assert!(index_keys("[Desktop Entry]\nHidden=true\n").hidden);
        let term = index_keys(
            "[Desktop Entry]\nExec=foot\nCategories=System;TerminalEmulator;\nX-TerminalArgExec=-e\n",
        );
        assert!(term.terminal_emulator);
        assert_eq!(term.terminal_arg_exec.as_deref(), Some("-e"));
        let bare = index_keys(
            "[Desktop Entry]\nExec=k\nCategories=TerminalEmulator\nX-TerminalArgExec=\n",
        );
        assert_eq!(
            bare.terminal_arg_exec.as_deref(),
            Some(""),
            "no option before the command"
        );
        for not_terminal in [
            "Categories=System;Terminal;",
            "Categories=TerminalEmulators;",
            "",
        ] {
            let k = index_keys(&format!("[Desktop Entry]\nExec=x\n{not_terminal}\n"));
            assert!(!k.terminal_emulator, "{not_terminal}");
            assert_eq!(k.terminal_arg_exec, None);
        }
        for not_hidden in ["Hidden=True", "Hidden=TRUE", "hidden=true", "Hidden=1"] {
            let buf = format!("[Desktop Entry]\n{not_hidden}\n");
            assert!(!index_keys(&buf).hidden, "{not_hidden}");
        }
    }

    #[test]
    fn a_malformed_file_is_refused_whole() {
        for bad in [
            "\u{feff}[Desktop Entry]\nExec=app\n",
            "Exec=app\n[Desktop Entry]\nExec=app\n",
            "[Desktop Entry]\nExec=app\nnot a key line\n",
            "[Desktop Entry]\nExec=app\nName=bad \\q escape\n",
        ] {
            assert!(exec(bad).is_err(), "{bad:?}");
            assert_eq!(index_keys(bad), IndexKeys::default(), "{bad:?}");
        }
    }
}
