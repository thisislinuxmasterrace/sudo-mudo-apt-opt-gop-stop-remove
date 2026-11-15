//! human terminal output. machine-readable stdout and log records bypass this module.

use std::fmt;
use std::io::{self, IsTerminal, Write};

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
        Tone::Heading => "1;36",
        Tone::Info => "2",
        Tone::Warning => "33",
        Tone::Error => "1;31",
        Tone::Success => "32",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

pub(crate) fn diagnostic(args: fmt::Arguments<'_>) {
    let text = args.to_string();
    let tone = if text.starts_with("note:")
        || text.starts_with("warning:")
        || text.contains(": warning:")
        || text.starts_with("kept ")
        || text.starts_with("aborted")
    {
        Tone::Warning
    } else if text.starts_with("erasing ")
        || text.starts_with("  ")
        || text.starts_with("the following ")
        || text.starts_with("the free space of ")
    {
        Tone::Info
    } else if text.starts_with("erased ") || text.starts_with("free space of ") {
        Tone::Success
    } else {
        Tone::Error
    };
    let _ = writeln!(io::stderr().lock(), "{}", styled(&text, tone, colors(true)));
}

pub(crate) fn error(args: fmt::Arguments<'_>) {
    let _ = writeln!(
        io::stderr().lock(),
        "{}",
        styled(&args.to_string(), Tone::Error, colors(true))
    );
}

pub(crate) fn section(title: &str, stderr: bool) {
    let title = styled(title, Tone::Heading, colors(stderr));
    if stderr {
        let _ = writeln!(io::stderr().lock(), "\n{title}");
    } else {
        let _ = writeln!(io::stdout().lock(), "\n{title}");
    }
}

pub(crate) fn stdout_line(args: fmt::Arguments<'_>) {
    let _ = writeln!(
        io::stdout().lock(),
        "{}",
        styled(&args.to_string(), Tone::Info, colors(false))
    );
}

pub(crate) fn prompt(text: &str) {
    let mut out = io::stderr().lock();
    let _ = write!(out, "{}", styled(text, Tone::Heading, colors(true)));
    let _ = out.flush();
}

pub(crate) fn stderr_colors() -> bool {
    colors(true)
}

pub(crate) fn progress(text: &str, color: bool) -> String {
    styled(text, Tone::Heading, color)
}

pub(crate) fn risks(notes: &[String]) {
    if notes.is_empty() {
        return;
    }
    section("risks", true);
    for note in notes {
        diagnostic(format_args!("{note}"));
    }
}

pub(crate) fn result(erased: usize, kept: usize, failed: bool, quiet: bool) {
    if quiet && !failed {
        return;
    }
    section("result", true);
    let (label, tone) = if failed {
        ("incomplete", Tone::Error)
    } else {
        ("done", Tone::Success)
    };
    let noun = if erased == 1 { "target" } else { "targets" };
    // windows can accept deletion queued for reboot; successful handling is not immediate removal.
    let action = if cfg!(windows) { "processed" } else { "erased" };
    let mut text = format!("{label}: {erased} {noun} {action}");
    if kept > 0 {
        let noun = if kept == 1 { "target" } else { "targets" };
        text.push_str(&format!(", {kept} {noun} kept by --exclude"));
    }
    let _ = writeln!(io::stderr().lock(), "{}", styled(&text, tone, colors(true)));
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
