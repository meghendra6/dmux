//! Minimal JSON serialization helpers for the `--format json` list output.
//!
//! dmux keeps a deliberately small dependency footprint, so rather than pull in
//! a serializer we hand-roll the tiny subset needed to emit flat objects with
//! string and integer fields.

/// Serialize a string as a JSON string literal, including the surrounding
/// quotes and escaping control characters per RFC 8259.
pub fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Serialize a server-provided field as a JSON number when it parses as a
/// non-negative integer, falling back to a quoted JSON string otherwise. The
/// list-format tokens emit unsigned counts and timestamps, so this keeps
/// numeric fields unquoted without trusting the input blindly.
pub fn json_u64_or_string(value: &str) -> String {
    match value.parse::<u64>() {
        Ok(number) => number.to_string(),
        Err(_) => json_string(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_string_escapes_quotes_backslashes_and_controls() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json_string("line\nbreak\ttab"), "\"line\\nbreak\\ttab\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn json_u64_or_string_keeps_integers_unquoted() {
        assert_eq!(json_u64_or_string("0"), "0");
        assert_eq!(json_u64_or_string("42"), "42");
        // Non-integer or signed values fall back to a quoted string.
        assert_eq!(json_u64_or_string("-1"), "\"-1\"");
        assert_eq!(json_u64_or_string("n/a"), "\"n/a\"");
    }
}
