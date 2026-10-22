//! json written by hand for `--log` and `--dry-run --json`. strings are escaped here, in one place.

use std::path::Path;

/// appends `text` as a json string, quotes included. `"` and `\` are escaped, control characters
/// become `\u00XX`; everything else, including non-ascii text, is written as it is.
pub fn push_str(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if u32::from(ch) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// appends a path as a json string. bytes that are not utf-8 become `U+FFFD`.
pub fn push_path(out: &mut String, path: &Path) {
    push_str(out, &path.to_string_lossy());
}

pub fn push_u64(out: &mut String, value: u64) {
    out.push_str(&value.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(text: &str) -> String {
        let mut out = String::new();
        push_str(&mut out, text);
        out
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn control_characters_become_unicode_escapes() {
        assert_eq!(
            quoted("\u{0}\u{1}\n\t\u{1f}"),
            r#""\u0000\u0001\u000a\u0009\u001f""#
        );
    }

    #[test]
    fn non_ascii_text_is_written_as_it_is() {
        assert_eq!(quoted("ä€😀"), "\"ä€😀\"");
    }

    #[test]
    fn delete_character_is_not_escaped() {
        assert_eq!(quoted("\u{7f}"), "\"\u{7f}\"");
    }

    #[test]
    fn empty_text_is_two_quotes() {
        assert_eq!(quoted(""), "\"\"");
    }

    #[cfg(unix)]
    #[test]
    fn names_that_are_not_utf8_are_lossy() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let mut out = String::new();
        push_path(&mut out, Path::new(OsStr::from_bytes(b"a\xffb")));

        assert_eq!(out, "\"a\u{fffd}b\"");
    }

    #[test]
    fn integers_are_plain_digits() {
        let mut out = String::new();
        push_u64(&mut out, 0);
        out.push(',');
        push_u64(&mut out, u64::MAX);

        assert_eq!(out, "0,18446744073709551615");
    }
}
