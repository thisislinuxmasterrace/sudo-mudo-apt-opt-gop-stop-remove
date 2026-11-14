// ctrl+c and sigterm during an erase (unix).
// run in the vm only (tests erase files): cargo test --locked --test signals
mod common;

/// signal tests: the handlers are installed once the erase has started, so the tests wait for the
/// `erasing` line before they send anything.
#[cfg(unix)]
mod interrupt {
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::Path;
    use std::process::Stdio;

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use crate::common::*;

    // the erase is still running when the signal arrives: the signal is sent right after the `erasing`
    // line, which is printed before the first pass starts, and a pass over 128 MiB with a sync takes far longer
    // than delivering a signal. the interrupt is checked before each object and in every 1 MiB write chunk.
    const BIG_FILE: u64 = 128 * 1024 * 1024;

    fn create_big_file(path: &Path) {
        let mut file = File::create(path).unwrap();
        // an all-hole file can finish without writes, before the signal reaches the process.
        let data = vec![0x55; 1024 * 1024];
        for _ in 0..BIG_FILE / data.len() as u64 {
            file.write_all(&data).unwrap();
        }
        file.sync_all().unwrap();
    }

    struct Run {
        code: Option<i32>,
        stderr: String,
        entries: Vec<String>,
        len: Option<u64>,
        path: String,
    }

    fn erase_and_interrupt(signals: &[Signal]) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        create_big_file(&file);

        let mut child = command()
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
    fn sighup_keeps_partly_erased_file_under_its_name() {
        let run = erase_and_interrupt(&[Signal::SIGHUP]);

        assert_eq!(run.code, Some(4), "stderr:\n{}", run.stderr);
        assert!(run.stderr.contains("interrupted by signal"));
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

    #[test]
    fn interrupt_inside_a_tree_keeps_the_tree_and_the_big_file() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        create_big_file(&tree.join("sub/big.bin"));

        let mut child = command()
            .args(["-f", "-r", "-v", "-n", "3", path_str(&tree)])
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
            if line.starts_with("  pass 1/3:") {
                break;
            }
        }
        kill(Pid::from_raw(child.id() as i32), Signal::SIGINT).unwrap();
        reader.read_to_string(&mut stderr).unwrap();
        let status = child.wait().unwrap();

        // the walk stops: the tree and the directory holding the big file stay, the big file keeps its
        // name and size, and no hidden name is left behind. the pass line is printed after hiding the
        // only file in the tree, so the signal interrupts an active recursive erase.
        assert_eq!(status.code(), Some(4), "stderr:\n{stderr}");
        assert!(stderr.contains("interrupted"), "stderr:\n{stderr}");
        assert!(tree.is_dir());
        assert_eq!(
            fs::metadata(tree.join("sub/big.bin")).unwrap().len(),
            BIG_FILE
        );
        for dir in [&tree, &tree.join("sub")] {
            for entry in fs::read_dir(dir).unwrap() {
                let name = entry.unwrap().file_name();
                assert!(!name.to_string_lossy().starts_with('.'), "{name:?} left");
            }
        }
    }
}
