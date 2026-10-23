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

#[cfg(not(windows))]
#[test]
fn windows_only_flags_are_rejected_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", "--kill-holders", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

/// a file that cannot be unlinked (its directory is not writable) keeps its contents and name,
/// and the run ends with exit code 4. root ignores directory permissions: the test is skipped
/// locally in that case, and fails under CI (where the variable `CI` is set) so it cannot pass unnoticed.
#[cfg(unix)]
#[test]
fn file_in_unwritable_directory_exits_with_four() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    // restores the mode on drop, so a panic still lets `TempDir` remove the directory.
    struct RestoreMode(PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    let file = write_file(&locked, "a.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let _restore = RestoreMode(locked.clone());

    if fs::write(locked.join("probe"), b"").is_ok() {
        if std::env::var_os("CI").is_some() {
            panic!(
                "directory permissions are not enforced here (running as root?): exit 4 was not tested"
            );
        }
        eprintln!("skipped: directory permissions are not enforced (running as root?)");
        return;
    }

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 4);
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
}
