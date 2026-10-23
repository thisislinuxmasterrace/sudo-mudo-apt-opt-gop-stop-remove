mod common;

/// signal tests: the handlers are installed once the erase has started, so the tests wait for the
/// `erasing` line before they send anything.
#[cfg(unix)]
mod interrupt {
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Command, Stdio};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use crate::common::*;

    // the erase is still running when the signal arrives: the signal is sent right after the `erasing`
    // line, which is printed before the first pass starts, and a pass over 128 MiB with a sync takes far longer
    // than delivering a signal. the interrupt is checked before each object and in every 1 MiB write chunk.
    const BIG_FILE: u64 = 128 * 1024 * 1024;

    struct Run {
        code: Option<i32>,
        stderr: String,
        entries: Vec<String>,
        len: Option<u64>,
        path: String,
    }

    /// erases a large file with `-f -v -n 3`, sends `signals` one after the other, and returns
    /// what was left behind.
    fn erase_and_interrupt(signals: &[Signal]) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        File::create(&file).unwrap().set_len(BIG_FILE).unwrap();

        let mut child = Command::new(BIN)
            .args(["-f", "-v", "-n", "3", path_str(&file)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start binary");
        let mut reader = BufReader::new(child.stderr.take().unwrap());
        let mut stderr = String::new();
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "erase did not start"
            );
            stderr.push_str(&line);
            if line.starts_with("erasing ") {
                break;
            }
        }

        let pid = Pid::from_raw(child.id() as i32);
        for signal in signals {
            kill(pid, *signal).unwrap();
        }

        reader.read_to_string(&mut stderr).unwrap();
        let status = child.wait().unwrap();

        let mut entries: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        let len = entries
            .first()
            .and_then(|name| fs::metadata(dir.path().join(name)).ok())
            .map(|meta| meta.len());
        Run {
            code: status.code(),
            stderr,
            entries,
            len,
            path: path_str(&file).to_owned(),
        }
    }

    #[test]
    fn sigint_keeps_partly_erased_file_under_its_name() {
        let run = erase_and_interrupt(&[Signal::SIGINT]);

        assert_eq!(run.code, Some(4), "stderr:\n{}", run.stderr);
        assert!(run.stderr.contains(&format!("interrupted: {}: ", run.path)));
        assert!(run.stderr.contains("interrupted by signal"));
        assert_eq!(run.entries, ["big.bin"]);
        assert_eq!(run.len, Some(BIG_FILE));
    }

    #[test]
    fn sigterm_keeps_partly_erased_file_under_its_name() {
        let run = erase_and_interrupt(&[Signal::SIGTERM]);

        assert_eq!(run.code, Some(4), "stderr:\n{}", run.stderr);
        assert_eq!(run.entries, ["big.bin"]);
        assert_eq!(run.len, Some(BIG_FILE));
    }

    #[test]
    fn second_interrupt_stops_the_erase_without_success() {
        // the second signal usually arrives before the write loop has seen the first one, and the process
        // exits with 130. under load the first signal is handled alone and the run ends with 4. both are
        // interrupts, so the test accepts either; the file is not erased in both cases.
        let run = erase_and_interrupt(&[Signal::SIGINT, Signal::SIGTERM]);

        assert!(matches!(run.code, Some(4 | 130)), "stderr:\n{}", run.stderr);
        assert_eq!(run.entries.len(), 1);
    }
}
