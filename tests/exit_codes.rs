// exit codes (0 to 4) and argument validation.
// run in the vm only (tests erase files): cargo test --locked --test exit_codes
mod common;

use common::*;

#[test]
fn one_invalid_path_aborts_whole_run() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write_file(dir.path(), "a.txt");
    let missing = dir.path().join("missing.txt");

    let out = run(&["-f", path_str(&valid), path_str(&missing)], "");

    assert_exit(&out, 1);
    assert!(valid.exists());
}

#[test]
fn missing_path_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.txt");

    let out = run(&["-f", path_str(&missing)], "");

    assert_exit(&out, 1);
}

#[test]
fn directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["-f", path_str(dir.path())], "");

    assert_exit(&out, 1);
    assert!(dir.path().exists());
}

#[test]
fn empty_path_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", path_str(&file), ""], "");
    assert_exit(&out, 2);
    assert!(file.exists());

    let wipe = run(&["--wipe-free", ""], "");
    assert_exit(&wipe, 2);
}

#[test]
fn zero_passes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-n", "0", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn no_paths_is_an_argument_error() {
    let out = run(&[], "");

    assert_exit(&out, 2);
}

#[test]
fn paths_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file), "--wipe-free", path_str(dir.path())], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn unknown_flag_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--no-such-flag", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn help_and_version_exit_with_zero() {
    assert_exit(&run(&["--help"], ""), 0);
    assert_exit(&run(&["--version"], ""), 0);
}

#[test]
fn incomplete_or_wrong_command_rejects_help_and_version() {
    for args in [
        vec!["--help"],
        vec!["apt", "--version"],
        vec!["apt", "opt", "gop", "stop", "--help"],
        vec!["apt", "opt", "gop", "stop", "wrong", "--help"],
    ] {
        let mut command = std::process::Command::new("sudo");
        #[cfg(unix)]
        command.arg("-n");
        let out = command.arg(BIN).args(args).output().unwrap();
        assert_exit(&out, 2);
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(
            text.contains("the command is `sudo mudo apt opt gop stop remove"),
            "{text}"
        );
        assert!(!text.contains("subcommand"), "{text}");
    }
}

#[test]
fn argument_errors_use_the_full_lowercase_usage() {
    let out = run(&["--no-such-flag"], "");
    assert_exit(&out, 2);
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("usage: sudo mudo apt opt gop stop remove [options] [paths]..."),
        "{text}"
    );
    assert!(
        !text.contains("Usage:") && !text.contains("For more"),
        "{text}"
    );
}

#[test]
fn help_lists_examples_and_exit_codes() {
    let out = run(&["--help"], "");

    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("examples:"));
    assert!(text.contains("exit codes: 0 done"));
    assert!(text.contains("usage: sudo mudo apt opt gop stop remove"));
    let short = run(&["-h"], "");
    let short = String::from_utf8_lossy(&short.stdout);
    for flag in ["-h, --help", "-V, --version"] {
        let line = short.lines().find(|line| line.contains(flag)).unwrap();
        assert!(line.contains("print"), "{line}");
    }
    assert!(
        !text.contains("Options:") && !text.contains("Print"),
        "{text}"
    );
}

#[cfg(not(windows))]
#[test]
fn windows_only_flags_are_rejected_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", "--kill-holders", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

/// a file that cannot be unlinked keeps its contents and name, and the run ends with exit code 4.
/// root ignores directory modes, so the directory is made immutable (`chattr +i`, linux
/// filesystems with attribute support such as ext4). without that the test is skipped locally and
/// fails under CI (where the variable `CI` is set) so it cannot pass unnoticed.
#[cfg(target_os = "linux")]
#[test]
fn file_in_unwritable_directory_exits_with_four() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    fn chattr(flag: &str, path: &std::path::Path) -> bool {
        Command::new("chattr")
            .arg(flag)
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    // clears the flag on drop, so a panic still lets `TempDir` remove the directory.
    struct Mutable(PathBuf);
    impl Drop for Mutable {
        fn drop(&mut self) {
            chattr("-i", &self.0);
        }
    }

    fn skip(reason: &str) {
        if std::env::var_os("CI").is_some() {
            panic!("{reason}: exit 4 was not tested");
        }
        eprintln!("skipped: {reason}");
    }

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    let file = write_file(&locked, "a.txt");
    let _mutable = Mutable(locked.clone());
    if !chattr("+i", &locked) {
        return skip("chattr +i is not available or not supported by this filesystem");
    }
    if fs::write(locked.join("probe"), b"").is_ok() {
        return skip("the immutable flag is not enforced for this process");
    }

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 4);
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
    assert_eq!(fs::read_dir(&locked).unwrap().count(), 1);
}
