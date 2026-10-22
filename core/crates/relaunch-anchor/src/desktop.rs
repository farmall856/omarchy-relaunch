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

/// Parse the first `Exec=` in the `[Desktop Entry]` group into an argv vector.
pub fn parse_exec(buffer: &str) -> Result<Vec<String>, String> {
    let raw = exec_value(buffer).ok_or_else(|| "no Exec in [Desktop Entry]".to_string())?;
    let unescaped = unescape_value(&raw)?;
    let tokens = tokenize(&unescaped)?;
    let argv = strip_field_codes(tokens);
    if argv.is_empty() {
        return Err("Exec expanded to nothing".into());
    }
    Ok(argv)
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

fn exec_value(buffer: &str) -> Option<String> {
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
        if let Some(rest) = t.strip_prefix("Exec") {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.to_string());
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

/// Drop field-code tokens and turn `%%` into `%` in the rest.
fn strip_field_codes(tokens: Vec<String>) -> Vec<String> {
    let codes = [
        "%f", "%F", "%u", "%U", "%i", "%c", "%k", "%v", "%m", "%d", "%D", "%n", "%N",
    ];
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        if codes.contains(&t.as_str()) {
            continue;
        }
        out.push(t.replace("%%", "%"));
    }
    out
}
