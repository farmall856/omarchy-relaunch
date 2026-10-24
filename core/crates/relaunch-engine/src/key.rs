//! The desktop-index key a class, id or name is normalized to (github issue
//! #13): ASCII-lowercased (not locale-aware, so distinct entries never fold
//! together), `' '`, `'_'` and `'.'` turned into `'-'`, and dash runs squeezed.

/// Normalize `s` to its index key.
#[must_use]
pub fn nkey(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            ' ' | '_' | '.' => '-',
            c => c.to_ascii_lowercase(),
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out
}
