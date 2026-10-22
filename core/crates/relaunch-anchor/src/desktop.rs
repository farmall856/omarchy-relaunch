//! A focused Desktop Entry `Exec` parser (freedesktop Desktop Entry spec, Exec
//! variables). It parses the launcher buffer the anchor hashed, so the argv
//! that is `execve`'d is exactly what was in the approved bytes — never shell,
//! never a `gio`/`gtk-launch` reopen.
//!
//! Supported: the `[Desktop Entry]` group's `Exec`, string-level escapes
//! (`\s \n \t \r \\`), quoting with `"` and the in-quote escapes `\" \` \$ \\`,
//! and the field codes `%f %F %u %U %i %c %k %v %m %d %D %n %N` (dropped) and
//! `%%` (→ `%`). An unterminated quote or a lone trailing backslash is a parse
//! error (refused, not guessed). `DBusActivatable` is handled by the caller
//! (refused); this module only parses `Exec`.

/// The parsed, relevant contents of a `[Desktop Entry]` group.
pub struct DesktopEntry {
    pub argv: Vec<String>,
    pub dbus_activatable: bool,
    pub terminal: bool,
    pub path: Option<String>,
}

/// Parse the `[Desktop Entry]` group: `Exec` into argv (with field codes
/// expanded), plus the keys that change how it must be launched.
pub fn parse_entry(buffer: &str) -> Result<DesktopEntry, String> {
    let raw = value_of(buffer, "Exec").ok_or_else(|| "no Exec in [Desktop Entry]".to_string())?;
    let unescaped = unescape_value(&raw)?;
    let tokens = tokenize(&unescaped)?;
    let argv = expand_field_codes(tokens)?;
    if argv.is_empty() {
        return Err("Exec expanded to nothing".into());
    }
    // Path= is a string value and must be unescaped by the same rules as Exec
    // before use as a working directory (reviewer #7); an escaped space would
    // otherwise become the wrong cwd.
    let path = match value_of(buffer, "Path").filter(|p| !p.is_empty()) {
        Some(raw) => Some(unescape_value(&raw)?),
        None => None,
    };
    Ok(DesktopEntry {
        argv,
        dbus_activatable: value_of(buffer, "DBusActivatable").as_deref() == Some("true"),
        terminal: value_of(buffer, "Terminal").as_deref() == Some("true"),
        path,
    })
}

/// Parse just the argv (convenience / tests).
pub fn parse_exec(buffer: &str) -> Result<Vec<String>, String> {
    Ok(parse_entry(buffer)?.argv)
}

/// A deterministic, injective rendering of an argv used to compare the live
/// launcher's `Exec` against the signed `exec`. Not meant to be shell-parsed;
/// only compared for equality. Backslash and space are escaped so distinct
/// argvs never collide, and the result carries no control characters.
pub fn canonical_exec_string(argv: &[String]) -> String {
    argv.iter()
        .map(|a| a.replace('\\', "\\\\").replace(' ', "\\ "))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first `<key>=` value in the `[Desktop Entry]` group (raw, pre-unescape).
fn value_of(buffer: &str, key: &str) -> Option<String> {
    let mut in_group = false;
    for line in buffer.lines() {
        let t = line.trim_start_matches('\u{feff}'); // tolerate a BOM
        if t.starts_with('[') {
            in_group = t.trim() == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some(rest) = t.strip_prefix(key) {
            // Only an exact key, optionally with spaces before '=' (no locale
            // suffix like Name[de]). Reject `ExecFoo=`.
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// Desktop Entry string unescaping on the raw value.
fn unescape_value(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => return Err("trailing backslash in Exec".into()),
        }
    }
    Ok(out)
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
                    Some(other) => {
                        cur.push('\\');
                        cur.push(other);
                    }
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
