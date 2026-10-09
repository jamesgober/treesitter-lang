//! Escaping and layout helpers shared by the emitters.
//!
//! The escapers scan bytes and copy the unescaped stretches between special
//! characters with one `push_str` each, so ordinary text is copied in bulk.
//! Every character they look for is ASCII except U+2028 and U+2029, which
//! JavaScript treats as line breaks; those start with the byte `0xE2`, which
//! is never a continuation byte, so every split point is a character
//! boundary.

use alloc::string::String;

/// Two spaces per indentation level.
const SPACES: &str = "                                                                ";

/// Appends `level` levels of two-space indentation.
pub(crate) fn indent(out: &mut String, level: usize) {
    let mut width = level * 2;
    while width > 0 {
        let chunk = width.min(SPACES.len());
        out.push_str(&SPACES[..chunk]);
        width -= chunk;
    }
}

/// Appends a decimal integer without going through `core::fmt`.
pub(crate) fn int(out: &mut String, value: i32) {
    if value < 0 {
        out.push('-');
    }
    let mut n = value.unsigned_abs();
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    loop {
        at -= 1;
        // `n % 10` is below 10, so the cast cannot truncate.
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &d in &digits[at..] {
        out.push(char::from(d));
    }
}

/// The JavaScript line separators U+2028 and U+2029 start with these bytes;
/// the third byte is `0xA8` or `0xA9`.
#[inline]
fn line_separator(bytes: &[u8], at: usize) -> Option<&'static str> {
    match bytes.get(at..at + 3) {
        Some([0xE2, 0x80, 0xA8]) => Some("\\u2028"),
        Some([0xE2, 0x80, 0xA9]) => Some("\\u2029"),
        _ => None,
    }
}

/// Appends `text` as a single-quoted JavaScript string literal.
pub(crate) fn js_string(out: &mut String, text: &str) {
    out.push('\'');
    let bytes = text.as_bytes();
    let mut from = 0;
    let mut at = 0;
    while at < bytes.len() {
        let (escape, width): (&str, usize) = match bytes[at] {
            b'\\' => ("\\\\", 1),
            b'\'' => ("\\'", 1),
            b'\n' => ("\\n", 1),
            b'\r' => ("\\r", 1),
            b'\t' => ("\\t", 1),
            b @ 0x00..=0x1F => {
                out.push_str(&text[from..at]);
                hex_escape(out, "\\x", b);
                at += 1;
                from = at;
                continue;
            }
            0xE2 => match line_separator(bytes, at) {
                Some(escape) => (escape, 3),
                None => {
                    at += 1;
                    continue;
                }
            },
            _ => {
                at += 1;
                continue;
            }
        };
        out.push_str(&text[from..at]);
        out.push_str(escape);
        at += width;
        from = at;
    }
    out.push_str(&text[from..]);
    out.push('\'');
}

/// Appends `pattern` as the body of a JavaScript regular expression literal,
/// the part between the slashes.
///
/// The pattern is regular-expression source, so its backslash escapes are
/// kept as they are. Only what a literal cannot (or, for JavaScript engines
/// that read source as C strings, should not) contain is changed: an
/// unescaped `/` becomes `\/`, and control characters and line breaks — raw
/// or after a backslash — become `\n`, `\r`, `\t`, `\xHH`, `\u2028`, and
/// `\u2029`, which match the same characters. A trailing lone backslash is
/// doubled so the literal still closes.
///
/// The literal must also end at its closing slash whatever the pattern
/// holds. JavaScript reads a `[` as opening a character class that runs to
/// the next unescaped `]`, and inside one a `/` does not end the literal, so
/// an unclosed `[` would carry the literal past its closing slash into the
/// text after it. Validation refuses such patterns; as a second line of
/// defence, every `[` from the first one that opens a class never closed is
/// written `\[`, so no class is left open at the end. A valid pattern has no
/// such `[` and is written unchanged.
pub(crate) fn js_regex(out: &mut String, pattern: &str) {
    let bytes = pattern.as_bytes();
    let unclosed = unclosed_class(bytes).unwrap_or(bytes.len());
    let mut from = 0;
    let mut at = 0;
    while at < bytes.len() {
        let (escape, width): (&str, usize) = match bytes[at] {
            b'/' => ("\\/", 1),
            // Past the first unclosed class there is no unescaped `]` at
            // all, so every `[` from there on would open a class that never
            // closes.
            b'[' if at >= unclosed => ("\\[", 1),
            b @ 0x00..=0x1F => {
                out.push_str(&pattern[from..at]);
                control(out, b);
                at += 1;
                from = at;
                continue;
            }
            b'\\' => match bytes.get(at + 1) {
                None => ("\\\\", 1),
                Some(&b @ 0x00..=0x1F) => {
                    // An escaped control character is the character itself.
                    out.push_str(&pattern[from..at]);
                    control(out, b);
                    at += 2;
                    from = at;
                    continue;
                }
                Some(0xE2) => match line_separator(bytes, at + 1) {
                    Some(escape) => (escape, 4),
                    None => {
                        at += 2;
                        continue;
                    }
                },
                // Any other escaped character is copied as written. Skipping
                // just the backslash and the next byte is enough: the
                // characters searched for are ASCII or start with `0xE2`, and
                // neither occurs inside a multi-byte character.
                Some(_) => {
                    at += 2;
                    continue;
                }
            },
            0xE2 => match line_separator(bytes, at) {
                Some(escape) => (escape, 3),
                None => {
                    at += 1;
                    continue;
                }
            },
            _ => {
                at += 1;
                continue;
            }
        };
        out.push_str(&pattern[from..at]);
        out.push_str(escape);
        at += width;
        from = at;
    }
    out.push_str(&pattern[from..]);
}

/// Appends `text` as a double-quoted JSON string, escaped as `serde_json`
/// escapes it: quote, backslash, and control characters only.
pub(crate) fn json_string(out: &mut String, text: &str) {
    out.push('"');
    let bytes = text.as_bytes();
    let mut from = 0;
    for (at, &b) in bytes.iter().enumerate() {
        let escape = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0x08 => "\\b",
            0x0C => "\\f",
            0x00..=0x1F => {
                out.push_str(&text[from..at]);
                hex_escape(out, "\\u00", b);
                from = at + 1;
                continue;
            }
            _ => continue,
        };
        out.push_str(&text[from..at]);
        out.push_str(escape);
        from = at + 1;
    }
    out.push_str(&text[from..]);
    out.push('"');
}

/// Appends a regular-expression escape for a control character.
fn control(out: &mut String, byte: u8) {
    match byte {
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        b'\t' => out.push_str("\\t"),
        _ => hex_escape(out, "\\x", byte),
    }
}

/// Where the first character class that is never closed opens, reading the
/// pattern the way a JavaScript lexer reads the body of a regular expression
/// literal: a backslash escapes the next character, a `[` outside a class
/// opens one, and the next `]` inside it closes it.
///
/// The rewrites [`js_regex`] makes do not change this reading: they turn
/// single characters into escapes, which contain no unescaped bracket, and
/// keep every backslash paired with the character it escaped. Skipping one
/// byte after a backslash is enough: a skipped multi-byte character's other
/// bytes are continuation bytes, never `[`, `]`, or `\`.
fn unclosed_class(bytes: &[u8]) -> Option<usize> {
    let mut open = None;
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        match (b, open) {
            (b'\\', _) => at += 1,
            (b'[', None) => open = Some(at),
            (b']', Some(_)) => open = None,
            _ => {}
        }
        at += 1;
    }
    open
}

/// Whether [`js_regex`] would write `pattern` differently from how it reads.
pub(crate) fn js_regex_rewrites(pattern: &str) -> bool {
    pattern.ends_with('\\')
        || pattern.bytes().any(|b| b == b'/' || b == 0xE2 || b < 0x20)
        || unclosed_class(pattern.as_bytes()).is_some()
}

/// Appends `prefix` and the byte as two lowercase hex digits.
fn hex_escape(out: &mut String, prefix: &str, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str(prefix);
    out.push(char::from(HEX[usize::from(byte >> 4)]));
    out.push(char::from(HEX[usize::from(byte & 0x0F)]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: fn(&mut String, &str), input: &str) -> String {
        let mut out = String::new();
        f(&mut out, input);
        out
    }

    #[test]
    fn test_js_string_escapes() {
        assert_eq!(run(js_string, "plain"), "'plain'");
        assert_eq!(run(js_string, ""), "''");
        assert_eq!(run(js_string, "it's"), r"'it\'s'");
        assert_eq!(run(js_string, r"a\b"), r"'a\\b'");
        assert_eq!(run(js_string, "a\nb\r\tc"), r"'a\nb\r\tc'");
        assert_eq!(run(js_string, "\u{0}\u{1b}"), r"'\x00\x1b'");
        assert_eq!(run(js_string, "x\u{2028}y\u{2029}"), r"'x\u2028y\u2029'");
        assert_eq!(run(js_string, "é→\"✓"), "'é→\"✓'");
    }

    #[test]
    fn test_js_regex_escapes() {
        assert_eq!(run(js_regex, r"\d+"), r"\d+");
        assert_eq!(run(js_regex, "a/b"), r"a\/b");
        assert_eq!(run(js_regex, r"a\/b"), r"a\/b");
        assert_eq!(run(js_regex, "[/]"), r"[\/]");
        assert_eq!(run(js_regex, "a\nb"), r"a\nb");
        assert_eq!(run(js_regex, "a\\\nb"), r"a\nb");
        assert_eq!(run(js_regex, "x\u{2028}"), r"x\u2028");
        assert_eq!(run(js_regex, "x\\\u{2029}"), r"x\u2029");
        assert_eq!(run(js_regex, r"end\"), r"end\\");
        assert_eq!(run(js_regex, "a\tb\0"), r"a\tb\x00");
        assert_eq!(run(js_regex, "\\\0\\\t\u{1}"), r"\x00\t\x01");
        assert!(js_regex_rewrites("a/b"));
        assert!(js_regex_rewrites("a\tb"));
        assert!(js_regex_rewrites(r"a\"));
        assert!(!js_regex_rewrites(r"\d+[a-z]é"));
        assert_eq!(run(js_regex, r"\\/"), r"\\\/");
        assert_eq!(run(js_regex, "\\é/"), "\\é\\/");
        assert_eq!(run(js_regex, "\\\u{2192}"), "\\\u{2192}");
    }

    #[test]
    fn test_js_regex_never_leaves_a_class_open() {
        // A class that closes is written as it is.
        assert_eq!(run(js_regex, "[a]"), "[a]");
        assert_eq!(run(js_regex, "[]"), "[]");
        assert_eq!(run(js_regex, "[[]"), "[[]");
        assert_eq!(run(js_regex, r"[\]/]"), r"[\]\/]");
        assert!(!js_regex_rewrites("[a][[]"));
        // From the first `[` whose class never closes, every unescaped `[`
        // is escaped; escaped ones are left alone.
        assert_eq!(run(js_regex, "["), r"\[");
        assert_eq!(run(js_regex, "[/"), r"\[\/");
        assert_eq!(run(js_regex, "a[b"), r"a\[b");
        assert_eq!(run(js_regex, "[a]["), r"[a]\[");
        assert_eq!(run(js_regex, "[[a"), r"\[\[a");
        assert_eq!(run(js_regex, r"[\[a"), r"\[\[a");
        assert_eq!(run(js_regex, r"[\]"), r"\[\]");
        assert_eq!(run(js_regex, r"[a\"), r"\[a\\");
        assert!(js_regex_rewrites("["));
        assert!(js_regex_rewrites("[a]["));
        assert_eq!(unclosed_class(b"[a]["), Some(3));
        assert_eq!(unclosed_class(b"[a\\]"), Some(0));
        assert_eq!(unclosed_class(b"\\[a"), None);
    }

    #[test]
    fn test_json_string_escapes() {
        assert_eq!(run(json_string, "plain"), "\"plain\"");
        assert_eq!(run(json_string, r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(run(json_string, "\n\r\t\u{8}\u{c}"), r#""\n\r\t\b\f""#);
        assert_eq!(run(json_string, "\u{1}\u{1f}"), r#""\u0001\u001f""#);
        assert_eq!(run(json_string, "é\u{2028}/"), "\"é\u{2028}/\"");
    }

    #[test]
    fn test_int_formats_extremes() {
        let cases = [(0, "0"), (7, "7"), (-1, "-1"), (1234, "1234")];
        for (value, text) in cases {
            let mut out = String::new();
            int(&mut out, value);
            assert_eq!(out, text);
        }
        let mut out = String::new();
        int(&mut out, i32::MIN);
        assert_eq!(out, "-2147483648");
        out.clear();
        int(&mut out, i32::MAX);
        assert_eq!(out, "2147483647");
    }

    #[test]
    fn test_indent_handles_deep_levels() {
        let mut out = String::new();
        indent(&mut out, 0);
        assert_eq!(out, "");
        indent(&mut out, 3);
        assert_eq!(out, "      ");
        out.clear();
        indent(&mut out, 100);
        assert_eq!(out.len(), 200);
        assert!(out.bytes().all(|b| b == b' '));
    }
}
