//! bounded read-only tool output. unavailable or incomplete output supplies no advisory fact.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 64 * 1024;

pub(crate) fn output(command: &mut Command) -> Option<(ExitStatus, Vec<u8>)> {
    output_with_timeout(command, Duration::from_secs(2))
}

fn output_with_timeout(command: &mut Command, timeout: Duration) -> Option<(ExitStatus, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
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
    let mut result = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(left) = deadline.checked_duration_since(Instant::now())
                    && let Ok(Ok(bytes)) = receiver.recv_timeout(left)
                    && bytes.len() as u64 <= OUTPUT_LIMIT
                {
                    result = Some((status, bytes));
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
        assert!(
            output_with_timeout(Command::new("sleep").arg("2"), Duration::from_millis(20))
                .is_none()
        );
        assert!(output(&mut Command::new("mudo-missing-advisory-tool")).is_none());
    }
}
