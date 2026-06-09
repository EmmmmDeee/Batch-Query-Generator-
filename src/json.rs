//! Tiny, correct JSON string helpers.
//!
//! We do not pull in a serialization crate. The only hard part of emitting
//! JSON by hand is escaping strings correctly, so that is the one thing we
//! centralize and test here. Everything else is plain `format!`.

/// Escape the contents of a string for inclusion inside JSON double quotes.
/// Does not add the surrounding quotes; see [`quote`].
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Return `s` as a quoted, escaped JSON string literal.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    out.push_str(&escape(s));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_control_and_specials() {
        assert_eq!(escape("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(escape("line1\nline2\t!"), "line1\\nline2\\t!");
        assert_eq!(escape("\u{0}\u{1f}"), "\\u0000\\u001f");
    }

    #[test]
    fn quote_wraps() {
        assert_eq!(quote("hi"), "\"hi\"");
        assert_eq!(quote("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn unicode_passes_through() {
        assert_eq!(escape("héllo→"), "héllo→");
    }
}
