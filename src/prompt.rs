//! prompts on stderr. a missing answer (eof, read error) always counts as no.

use std::io::{self, IsTerminal};

/// asks a yes/no question on stderr. anything other than "y" or "yes", including EOF, means no.
pub fn ask(question: &str) -> bool {
    read_line(&format!("{question} [y/N] "))
        .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// a signal can arrive while an answer is read; check it before a confirmed action has side effects.
#[cfg(any(windows, test))]
pub(crate) fn confirmed_action(
    confirm: impl FnOnce() -> bool,
    check_interrupt: impl FnOnce() -> io::Result<()>,
    action: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    let confirmed = confirm();
    check_interrupt()?;
    if confirmed {
        action()?;
    }
    Ok(confirmed)
}

/// asks for `expected` to be typed exactly, on stderr. anything else, including EOF, means no.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn ask_exact(question: &str, expected: &str) -> bool {
    read_answer(
        &format!("{question} "),
        "stdin is not a terminal: a device is erased only after its path is typed, run it from a terminal",
    )
    .is_some_and(|answer| answer == expected)
}

/// prints `prompt` on stderr and reads one line, without its surrounding spaces. none means EOF or
/// a read error, and both are reported here.
pub fn read_line(prompt: &str) -> Option<String> {
    read_answer(
        prompt,
        "stdin is not a terminal: use -f to erase without a prompt",
    )
}

fn read_answer(prompt: &str, eof_hint: &str) -> Option<String> {
    eprint!("{prompt}");

    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) => {
            eprintln!();
            if !io::stdin().is_terminal() {
                eprintln!("{eof_hint}");
            }
            None
        }
        Ok(_) => Some(answer.trim().to_owned()),
        Err(err) => {
            eprintln!();
            eprintln!("could not read the answer: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn interruption_during_confirmation_prevents_action() {
        let interrupted = Cell::new(false);
        let acted = Cell::new(false);
        let result = confirmed_action(
            || {
                interrupted.set(true);
                true
            },
            || {
                if interrupted.get() {
                    Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"))
                } else {
                    Ok(())
                }
            },
            || {
                acted.set(true);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!acted.get());
    }

    #[test]
    fn action_requires_confirmation() {
        for confirmed in [false, true] {
            let acted = Cell::new(false);
            let result = confirmed_action(
                || confirmed,
                || Ok(()),
                || {
                    acted.set(true);
                    Ok(())
                },
            );
            assert_eq!(result.unwrap(), confirmed);
            assert_eq!(acted.get(), confirmed);
        }
    }
}
