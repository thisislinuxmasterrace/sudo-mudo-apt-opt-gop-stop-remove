//! human terminal output. machine-readable stdout and log records bypass this module.

use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

// a heading is separated from earlier output by a blank line, but the first line of a stream is not blank.
static STDOUT_USED: AtomicBool = AtomicBool::new(false);
static STDERR_USED: AtomicBool = AtomicBool::new(false);

fn mark_used(stderr: bool) -> bool {
    let used = if stderr { &STDERR_USED } else { &STDOUT_USED };
    used.swap(true, Ordering::Relaxed)
}

macro_rules! message {
    () => { $crate::ui::diagnostic(format_args!("")) };
    ($($arg:tt)*) => { $crate::ui::diagnostic(format_args!($($arg)*)) };
}
pub(crate) use message;

macro_rules! failure {
    ($($arg:tt)*) => { $crate::ui::error(format_args!($($arg)*)) };
}
pub(crate) use failure;

#[derive(Clone, Copy)]
enum Tone {
    Plain,
    Heading,
    Info,
    Warning,
    Error,
    Success,
}

fn color_policy(terminal: bool, no_color: bool, dumb: bool) -> bool {
    terminal && !no_color && !dumb
}

fn colors(stderr: bool) -> bool {
    let terminal = if stderr {
        io::stderr().is_terminal()
    } else {
        io::stdout().is_terminal()
    };
    if !color_policy(
        terminal,
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var_os("TERM").is_some_and(|term| term == "dumb"),
    ) {
        return false;
    }
    enable_colors(stderr)
}

#[cfg(not(windows))]
fn enable_colors(_stderr: bool) -> bool {
    true
}

#[cfg(windows)]
fn enable_colors(stderr: bool) -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    let mut mode = 0;
    // safety: standard handles are borrowed; `mode` is a valid output pointer.
    unsafe {
        let handle = GetStdHandle(if stderr {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        });
        GetConsoleMode(handle, &mut mode) != 0
            && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

fn styled(text: &str, tone: Tone, color: bool) -> String {
    if !color {
        return text.to_owned();
    }
    let code = match tone {
        Tone::Plain => return text.to_owned(),
        Tone::Heading => "1;36",
        Tone::Info => "2",
        Tone::Warning => "33",
        Tone::Error => "1;31",
        Tone::Success => "32",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn tone_of(text: &str) -> Tone {
    if text.starts_with("note:")
        || text.starts_with("warning:")
        || text.contains(": warning:")
        || text.starts_with("kept ")
        || text.starts_with("aborted")
    {
        Tone::Warning
    } else if text.starts_with("erasing ")
        || text.starts_with("  pass ")
        || text.starts_with("    ")
    {
        Tone::Info
    } else if text.is_empty() || text.starts_with("  ") || text.starts_with("the following ") {
        Tone::Plain
    } else if text.starts_with("erased ")
        || text.starts_with("processed ")
        || text.starts_with("free space of ")
    {
        Tone::Success
    } else {
        Tone::Error
    }
}

/// os error texts start with a capital and end with ` (os error N)`; the messages here are lowercase and
/// the number does not help a reader.
fn tidy(text: &str) -> String {
    const MARK: &str = " (os error ";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        let tail = &rest[at + MARK.len()..];
        let Some(end) = tail
            .find(')')
            .filter(|&end| end > 0 && tail[..end].bytes().all(|b| b.is_ascii_digit() || b == b'-'))
        else {
            break;
        };
        let head = &rest[..at];
        let start = head.rfind(": ").map_or(0, |colon| colon + 2);
        out.push_str(&head[..start]);
        let reason = head[start..].trim_end_matches('.');
        let mut chars = reason.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_lowercase());
            out.push_str(chars.as_str());
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn write_stderr(text: &str, tone: Tone) {
    mark_used(true);
    let _ = writeln!(
        io::stderr().lock(),
        "{}",
        styled(&tidy(text), tone, colors(true))
    );
}

pub(crate) fn diagnostic(args: fmt::Arguments<'_>) {
    let text = args.to_string();
    write_stderr(&text, tone_of(&text));
}

pub(crate) fn error(args: fmt::Arguments<'_>) {
    write_stderr(&args.to_string(), Tone::Error);
}

pub(crate) fn section(title: &str, stderr: bool) {
    let gap = if mark_used(stderr) { "\n" } else { "" };
    let title = styled(title, Tone::Heading, colors(stderr));
    if stderr {
        let _ = writeln!(io::stderr().lock(), "{gap}{title}");
    } else {
        let _ = writeln!(io::stdout().lock(), "{gap}{title}");
    }
}

pub(crate) fn stdout_line(args: fmt::Arguments<'_>) {
    mark_used(false);
    let _ = writeln!(io::stdout().lock(), "{args}");
}

pub(crate) fn prompt(text: &str) {
    mark_used(true);
    let mut out = io::stderr().lock();
    let _ = write!(out, "{}", styled(text, Tone::Heading, colors(true)));
    let _ = out.flush();
}

pub(crate) fn stderr_colors() -> bool {
    colors(true)
}

pub(crate) fn progress(text: &str, color: bool) -> String {
    mark_used(true);
    styled(text, Tone::Heading, color)
}

/// `1 pass`, `3 passes`: a count with its noun in the right number.
pub(crate) fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// the notes before an erase. under a heading they are indented like the other sections.
pub(crate) fn risks(notes: &[String], heading: bool) {
    if notes.is_empty() {
        return;
    }
    if heading {
        section("risks", true);
    }
    let indent = if heading { "  " } else { "" };
    for note in notes {
        write_stderr(&format!("{indent}{note}"), Tone::Warning);
    }
}

/// the last line of a path erase. one target is named; several are counted.
pub(crate) fn result(
    targets: &[PathBuf],
    erased: usize,
    kept: usize,
    failed: bool,
    quiet: bool,
    heading: bool,
) {
    if quiet && !failed {
        return;
    }
    if heading {
        section("result", true);
    }
    // windows can accept deletion queued for reboot; successful handling is not immediate removal.
    let action = if cfg!(windows) { "processed" } else { "erased" };
    let total = targets.len() as u64;
    let (text, tone) = if failed {
        (
            format!(
                "incomplete: {erased} of {} {action}, see the errors above",
                count(total, "target", "targets")
            ),
            Tone::Error,
        )
    } else if kept == 0 {
        let what = match targets {
            [only] => only.display().to_string(),
            _ => count(total, "target", "targets"),
        };
        (format!("{action} {what}"), Tone::Success)
    } else {
        (
            format!(
                "{action} {}; {} kept because of --exclude",
                count(erased as u64, "target", "targets"),
                kept
            ),
            Tone::Success,
        )
    };
    write_stderr(&text, tone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_require_a_terminal_and_respect_opt_outs() {
        assert!(color_policy(true, false, false));
        assert!(!color_policy(false, false, false));
        assert!(!color_policy(true, true, false));
        assert!(!color_policy(true, false, true));
        assert!(!color_policy(true, true, true));
    }

    #[test]
    fn plain_output_preserves_messages_and_styling_resets() {
        let text = "mudo: file: permission denied";
        assert_eq!(styled(text, Tone::Error, false), text);
        assert_eq!(
            styled(text, Tone::Error, true),
            format!("\x1b[1;31m{text}\x1b[0m")
        );
    }

    #[test]
    fn os_error_numbers_are_dropped_and_the_reason_is_lowercase() {
        assert_eq!(
            tidy("mudo: Some/Path: No such file or directory (os error 2)"),
            "mudo: Some/Path: no such file or directory"
        );
        assert_eq!(
            tidy("mudo: x: Access is denied. (os error 5); the rest was kept"),
            "mudo: x: access is denied; the rest was kept"
        );
        assert_eq!(tidy("mudo: x (os error y)"), "mudo: x (os error y)");
        assert_eq!(tidy("plain text"), "plain text");
    }

    #[test]
    fn listed_paths_are_plain_and_pass_details_are_dim() {
        assert!(matches!(tone_of("  venom.exe"), Tone::Plain));
        assert!(matches!(tone_of("  pass 1/2: zeros"), Tone::Info));
        assert!(matches!(tone_of("erased venom.exe"), Tone::Success));
        assert!(matches!(tone_of("note: x"), Tone::Warning));
    }

    #[test]
    fn counts_use_the_right_number() {
        assert_eq!(count(1, "pass", "passes"), "1 pass");
        assert_eq!(count(0, "file", "files"), "0 files");
        assert_eq!(count(3, "pass", "passes"), "3 passes");
    }

    #[test]
    fn all_terminal_tones_match_golden_sequences() {
        let tones = [
            Tone::Heading,
            Tone::Info,
            Tone::Warning,
            Tone::Error,
            Tone::Success,
        ];
        let actual = tones.map(|tone| styled("text", tone, true)).join("\n");
        assert_eq!(
            actual,
            "\x1b[1;36mtext\x1b[0m\n\x1b[2mtext\x1b[0m\n\x1b[33mtext\x1b[0m\n\x1b[1;31mtext\x1b[0m\n\x1b[32mtext\x1b[0m"
        );
        assert!(
            tones
                .into_iter()
                .all(|tone| styled("text", tone, false) == "text")
        );
    }
}
