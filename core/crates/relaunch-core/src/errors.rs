//! Error text that crosses a process boundary (relaunchd → relaunchctl → the
//! panel): bounded, and never carrying request bytes.

/// Longest error message a binary hands to a client.
pub const MAX_PUBLIC_ERROR: usize = 256;

/// `message` cut to at most [`MAX_PUBLIC_ERROR`] bytes, on a character
/// boundary.
#[must_use]
pub fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_PUBLIC_ERROR {
        return message.to_owned();
    }
    let mut end = MAX_PUBLIC_ERROR;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.get(..end).unwrap_or_default().to_owned()
}

/// A JSON parse failure described without any of the input: its category and
/// position only (serde's own message can quote unknown fields and values).
#[must_use]
pub fn json_error(e: &serde_json::Error) -> String {
    let category = match e.classify() {
        serde_json::error::Category::Io => "unreadable",
        serde_json::error::Category::Syntax => "malformed JSON",
        serde_json::error::Category::Data => "unexpected JSON shape",
        serde_json::error::Category::Eof => "truncated JSON",
    };
    format!("{category} at line {} column {}", e.line(), e.column())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_error_quotes_nothing_from_the_input() {
        let secret = "x".repeat(10_000) + "SECRET";
        let bad = format!(r#"{{"v":2,"{secret}":1}}"#);
        let e = serde_json::from_str::<crate::ConfigFile>(&bad).unwrap_err();
        let text = json_error(&e);
        assert!(!text.contains("SECRET") && !text.contains("xxx"), "{text}");
        assert!(text.starts_with("unexpected JSON shape"), "{text}");
    }

    #[test]
    fn a_message_is_bounded_on_a_char_boundary() {
        let long = "é".repeat(MAX_PUBLIC_ERROR);
        let b = bounded_message(&long);
        assert!(b.len() <= MAX_PUBLIC_ERROR && b.chars().all(|c| c == 'é'));
        assert_eq!(bounded_message("short"), "short");
    }
}
