//! Web apps: the identity a launcher's `Exec` opens, and the window-class
//! identity a Chromium-based browser derives from it (github issue #11).
//!
//! A Chromium-based browser names an `--app=URL` window
//! `<browser>-<host>_<path, / as _>-Default` (`GenerateApplicationNameFromURL`)
//! and an installed app's window `<browser>-<app id>-Default`, whichever
//! browser it is. Identities are compared forward-encoded and never decoded,
//! because `/` -> `_` is lossy. No program is named: any `Exec` that opens a
//! URL or an app id is read the same way.

use crate::argv::{is_assignment_like, is_wrapper_cmd};
use crate::shell::split_stored;

/// The URL an `Exec` opens as an app: its `--app=URL`, else its first
/// positional `http://` or `https://` argument. `None` when it opens none.
#[must_use]
pub fn url_from_exec(raw: &str) -> Option<String> {
    let words = split_stored(raw);
    let args: Vec<&String> = words
        .iter()
        .skip_while(|w| is_wrapper_cmd(w) || *w == "--" || is_assignment_like(w))
        .skip(1)
        .collect();
    if let Some(url) = args.iter().find_map(|a| a.strip_prefix("--app=")) {
        return (!url.is_empty()).then(|| url.to_owned());
    }
    args.iter()
        .find(|a| !a.starts_with('-') && !is_field_code(a))
        .filter(|a| a.starts_with("http://") || a.starts_with("https://"))
        .map(|a| (*a).clone())
}

/// The installed-app id an `Exec` opens (`--app-id=ID`), when it is one
/// lowercase alphanumeric word.
#[must_use]
pub fn app_id_from_exec(raw: &str) -> Option<String> {
    split_stored(raw)
        .iter()
        .find_map(|a| a.strip_prefix("--app-id="))
        .filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        .map(str::to_owned)
}

/// The web-app identity an `Exec` opens: its app id, else its URL's.
#[must_use]
pub fn identity_for_exec(raw: &str) -> Option<Identity> {
    match app_id_from_exec(raw) {
        Some(id) => Some(Identity::Known(id)),
        None => identity(&url_from_exec(raw)?),
    }
}

/// `%?`: a percent sign and exactly one character.
fn is_field_code(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next() == Some('%') && chars.next().is_some() && chars.next().is_none()
}

/// A web app's identity, or why it has none that can be compared.
#[derive(Debug, PartialEq, Eq)]
pub enum Identity {
    /// Chromium's identity, exactly as its window class carries it.
    Known(String),
    /// A host Chromium canonicalizes in ways not reproduced here (IDNA,
    /// percent-decoding): it names some identity, but which is unknown.
    Uncanonical,
}

/// The identity of `url`: [`identity_from_url`], or [`Identity::Uncanonical`]
/// for a host that is not plain ASCII without percent-encoding.
#[must_use]
pub fn identity(url: &str) -> Option<Identity> {
    let id = identity_from_url(url)?;
    let host = id.split('_').next().unwrap_or("");
    if host.is_ascii() && !host.contains('%') {
        Some(Identity::Known(id))
    } else {
        Some(Identity::Uncanonical)
    }
}

/// Chromium's identity for a URL: lowercase host (no userinfo, no port; a
/// bracketed IPv6 literal kept whole), `_`, then the dot-segment-normalized
/// path with every `/` as `_`. Query and fragment are discarded. `None` without
/// a scheme or a host. Exact only for the hosts [`identity`] calls known.
#[must_use]
pub fn identity_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_owned()),
    };
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if host.starts_with('[') && host.get(1..).is_some_and(|h| h.contains(']')) {
        format!("{}]", host.split(']').next().unwrap_or(""))
    } else {
        host.split(':').next().unwrap_or("").to_owned()
    };
    if host.is_empty() {
        return None;
    }
    let path = remove_dot_segments(&path);
    Some(format!(
        "{}_{}",
        host.to_lowercase(),
        path.replace('/', "_")
    ))
}

/// RFC 3986 §5.2.4, transcribed literally (a segment-stack approximation drops
/// empty segments and the trailing slash a final `.` or `..` leaves, which are
/// different Chromium identities).
#[must_use]
pub fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_owned();
    let mut output = String::new();
    let pop = |out: &mut String| {
        if let Some(k) = out.rfind('/') {
            out.truncate(k);
        }
    };
    while !input.is_empty() {
        if let Some(r) = input
            .strip_prefix("../")
            .or_else(|| input.strip_prefix("./"))
        {
            input = r.to_owned();
        } else if let Some(r) = input.strip_prefix("/./") {
            input = format!("/{r}");
        } else if input == "/." {
            "/".clone_into(&mut input);
        } else if let Some(r) = input.strip_prefix("/../") {
            input = format!("/{r}");
            pop(&mut output);
        } else if input == "/.." {
            "/".clone_into(&mut input);
            pop(&mut output);
        } else if input == "." || input == ".." {
            input.clear();
        } else {
            let seg = match input.strip_prefix('/') {
                Some(rest) => format!("/{}", rest.split('/').next().unwrap_or("")),
                None => input.split('/').next().unwrap_or("").to_owned(),
            };
            output.push_str(&seg);
            input = input.get(seg.len()..).unwrap_or("").to_owned();
        }
    }
    output
}
