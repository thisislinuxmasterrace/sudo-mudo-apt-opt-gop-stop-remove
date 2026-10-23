//! the walk of a tree given on the command line: the position of an entry in it, and the `--exclude` matcher.

use std::path::Path;

use crate::log::Log;

/// where an entry sits in a walk: `rel` is its path from the command-line argument (empty for the
/// argument itself), `excludes` are the `--exclude` patterns, and `log` is the `--log` file, if any.
#[derive(Clone, Copy)]
pub(super) struct Walk<'a> {
    pub(super) rel: &'a Path,
    pub(super) excludes: &'a [String],
    pub(super) log: Option<&'a Log>,
}

/// true when `rel`, the path of an entry from its command-line argument, matches an `--exclude`
/// pattern. a pattern without `/` is compared with the name of the entry, a pattern with `/` with the
/// whole path. the argument itself has an empty `rel`, so it never matches.
pub(crate) fn is_excluded(patterns: &[String], rel: &Path) -> bool {
    let Some(name) = rel.file_name() else {
        return false;
    };
    let parts: Vec<&[u8]> = rel
        .components()
        .map(|part| part.as_os_str().as_encoded_bytes())
        .collect();
    let whole = parts.join(&b'/');
    patterns.iter().any(|pattern| {
        let text = if pattern.contains('/') {
            whole.as_slice()
        } else {
            name.as_encoded_bytes()
        };
        glob_match(pattern.as_bytes(), text)
    })
}

/// `*` matches any run of characters except `/`, `?` matches one UTF-8 character except `/`, and every
/// other byte matches itself. there are no character classes. the text is walked bytewise, but a step
/// never stops inside a UTF-8 character.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // the last `*`: where its pattern resumes, and the text byte where it started matching.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    star = Some((p + 1, t));
                    p += 1;
                    continue;
                }
                b'?' if text[t] != b'/' => {
                    p += 1;
                    t = char_end(text, t);
                    continue;
                }
                literal if literal == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }
        // no direct match: the last `*` takes one more byte, unless that byte is a `/`.
        match star {
            Some((resume, taken)) if text[taken] != b'/' => {
                let next = char_end(text, taken);
                star = Some((resume, next));
                p = resume;
                t = next;
            }
            _ => return false,
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

/// index just past the UTF-8 character that starts at `at`: the continuation bytes are `0b10xxxxxx`.
fn char_end(text: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    while end < text.len() && (text[end] & 0xC0) == 0x80 {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_matches_any_run_but_not_a_slash() {
        assert!(glob_match(b"*.o", b"main.o"));
        assert!(glob_match(b"*", b"anything"));
        assert!(glob_match(b"a*c", b"ac"));
        assert!(glob_match(b"a*c", b"abbbc"));
        assert!(!glob_match(b"a*c", b"abd"));
        assert!(!glob_match(b"*.o", b"src/main.o"));
        assert!(glob_match(b"*a*b", b"xaab"));
        assert!(!glob_match(b"*a*b", b"xaba"));
    }

    #[test]
    fn glob_question_matches_one_byte_but_not_a_slash() {
        assert!(glob_match(b"?.txt", b"a.txt"));
        assert!(!glob_match(b"?.txt", b"ab.txt"));
        assert!(!glob_match(b"?.txt", b"/.txt"));
        assert!(!glob_match(b"a?b", b"a/b"));
    }

    #[test]
    fn glob_question_matches_one_utf8_character() {
        assert!(glob_match("?.txt".as_bytes(), "ä.txt".as_bytes()));
        assert!(!glob_match("??.txt".as_bytes(), "ä.txt".as_bytes()));
        assert!(glob_match("?".as_bytes(), "ö".as_bytes()));
        assert!(!glob_match("?".as_bytes(), "/".as_bytes()));
        assert!(glob_match("a*b".as_bytes(), "aäöb".as_bytes()));
        assert!(glob_match("*ö*".as_bytes(), "xöy".as_bytes()));
    }

    #[test]
    fn glob_other_bytes_match_literally() {
        assert!(glob_match(b"file.txt", b"file.txt"));
        assert!(!glob_match(b"file.txt", b"fileXtxt"));
        assert!(glob_match(b"a[b]", b"a[b]"));
        assert!(!glob_match(b"a[b]", b"ab"));
    }

    #[test]
    fn pattern_without_slash_matches_the_name_at_any_depth() {
        let patterns = ["*.log".to_owned()];

        assert!(is_excluded(&patterns, Path::new("a.log")));
        assert!(is_excluded(&patterns, Path::new("sub/deeper/a.log")));
        assert!(!is_excluded(&patterns, Path::new("sub/a.logx")));
    }

    #[test]
    fn pattern_with_slash_matches_the_path_from_the_argument() {
        let patterns = ["src/*.o".to_owned()];

        assert!(is_excluded(&patterns, Path::new("src/main.o")));
        assert!(!is_excluded(&patterns, Path::new("main.o")));
        assert!(!is_excluded(&patterns, Path::new("other/src/main.o")));
        assert!(!is_excluded(&patterns, Path::new("src/deeper/main.o")));
    }

    #[test]
    fn command_line_argument_is_never_excluded() {
        let patterns = ["*".to_owned(), "build".to_owned()];

        assert!(!is_excluded(&patterns, Path::new("")));
    }
}
