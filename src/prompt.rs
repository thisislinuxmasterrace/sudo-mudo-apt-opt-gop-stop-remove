//! prompts on stderr. a missing answer (eof, read error) always counts as no.

use std::io::{self, IsTerminal};

/// asks a yes/no question on stderr. anything other than "y" or "yes", including EOF, means no.
pub fn ask(question: &str) -> bool {
    read_line(&format!("{question} [y/N] "))
        .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// asks during a windows erase, allowing an interrupt to end a blocked read.
#[cfg(windows)]
pub(crate) fn ask_interruptible(question: &str) -> io::Result<bool> {
    use std::sync::mpsc;

    crate::erase::check_interrupted()?;
    eprint!("{question} [y/N] ");
    let (sender, receiver) = mpsc::channel();
    // the worker only reads; an interrupt ends the erase before another prompt can start.
    std::thread::spawn(move || {
        let mut answer = String::new();
        let result = io::stdin().read_line(&mut answer).map(|n| (n, answer));
        let _ = sender.send(result);
    });
    let result = wait_for_answer(&receiver, crate::erase::check_interrupted)?;
    Ok(report_answer(
        result,
        "stdin is not a terminal: use -f to erase without a prompt",
    )
    .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")))
}

#[cfg(any(windows, test))]
fn wait_for_answer<T>(
    receiver: &std::sync::mpsc::Receiver<T>,
    check_interrupt: impl Fn() -> io::Result<()>,
) -> io::Result<T> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    loop {
        check_interrupt()?;
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(answer) => {
                check_interrupt()?;
                return Ok(answer);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("could not read the answer"));
            }
        }
    }
}

/// a signal can arrive while an answer is read; check it before a confirmed action has side effects.
#[cfg(any(windows, test))]
pub(crate) fn confirmed_action(
    confirm: impl FnOnce() -> io::Result<bool>,
    check_interrupt: impl FnOnce() -> io::Result<()>,
    action: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    let confirmed = confirm()?;
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
    let result = io::stdin().read_line(&mut answer).map(|n| (n, answer));
    report_answer(result, eof_hint)
}

fn report_answer(result: io::Result<(usize, String)>, eof_hint: &str) -> Option<String> {
    match result {
        Ok((0, _)) => {
            eprintln!();
            if !io::stdin().is_terminal() {
                eprintln!("{eof_hint}");
            }
            None
        }
        Ok((_, answer)) => Some(answer.trim().to_owned()),
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
                Ok(true)
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
                || Ok(confirmed),
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

    #[test]
    fn blocked_answer_is_interrupted_without_waiting_for_sender() {
        let (sender, receiver) = std::sync::mpsc::channel::<bool>();
        let checks = Cell::new(0);
        let result = wait_for_answer(&receiver, || {
            checks.set(checks.get() + 1);
            if checks.get() > 1 {
                Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(sender.send(true).is_ok());
    }

    #[test]
    fn interrupt_after_answer_prevents_action() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(true).unwrap();
        let checks = Cell::new(0);
        let result = wait_for_answer(&receiver, || {
            checks.set(checks.get() + 1);
            if checks.get() > 1 {
                Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
}
