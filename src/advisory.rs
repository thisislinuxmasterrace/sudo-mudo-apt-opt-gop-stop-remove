//! bounded read-only tool output. unavailable or incomplete output supplies no advisory fact.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 64 * 1024;

/// why a lookup supplied nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// the tool is missing, failed to start, timed out or could not be read.
    Unavailable,
    /// the tool printed more than the cap, so its output was dropped as incomplete.
    TooLarge,
}

pub(crate) fn output(command: &mut Command) -> Option<(ExitStatus, Vec<u8>)> {
    checked_output(command).ok()
}

/// like `output`, and tells an output over the cap from the other failures.
pub(crate) fn checked_output(command: &mut Command) -> Result<(ExitStatus, Vec<u8>), Failure> {
    output_with_timeout(command, Duration::from_secs(2))
}

fn output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>), Failure> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Failure::Unavailable)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::Unavailable);
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + timeout;
    let mut result = Err(Failure::Unavailable);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(left) = deadline.checked_duration_since(Instant::now())
                    && let Ok(Ok(bytes)) = receiver.recv_timeout(left)
                {
                    result = if bytes.len() as u64 <= OUTPUT_LIMIT {
                        Ok((status, bytes))
                    } else {
                        Err(Failure::TooLarge)
                    };
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    // reap even on timeout or output overflow; never leave the tool running after this lookup.
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn bounded_tool_output_distinguishes_success_failure_and_timeout() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf fact"]);
        let (status, bytes) = output(&mut command).unwrap();
        assert!(status.success());
        assert_eq!(bytes, b"fact");
        assert_eq!(
            output(Command::new("sh").args(["-c", "exit 7"]))
                .unwrap()
                .0
                .code(),
            Some(7)
        );
        assert_eq!(
            output_with_timeout(Command::new("sleep").arg("2"), Duration::from_millis(20))
                .unwrap_err(),
            Failure::Unavailable
        );
        assert_eq!(
            checked_output(&mut Command::new("mudo-missing-advisory-tool")).unwrap_err(),
            Failure::Unavailable
        );
        assert!(output(&mut Command::new("mudo-missing-advisory-tool")).is_none());
    }

    #[test]
    fn output_over_the_cap_is_told_apart_from_an_unavailable_tool() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 70000 /dev/zero"]);
        assert_eq!(checked_output(&mut command).unwrap_err(), Failure::TooLarge);
        assert!(output(&mut command).is_none());
    }
}
