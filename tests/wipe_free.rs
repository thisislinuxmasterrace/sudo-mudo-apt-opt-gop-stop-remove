mod common;

use std::fs;

use common::*;

#[test]
fn wipe_free_rejects_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--wipe-free", path_str(&file), "-f"], "");

    assert_exit(&out, 1);
    assert!(file.exists());
}

#[test]
fn wipe_free_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["--wipe-free", path_str(dir.path()), "--dry-run"], "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would fill"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn wipe_free_declined_exits_with_three() {
    let dir = tempfile::tempdir().unwrap();

    let declined = run(&["--wipe-free", path_str(dir.path())], "n\n");
    assert_exit(&declined, 3);

    let eof = run(&["--wipe-free", path_str(dir.path())], "");
    assert_exit(&eof, 3);

    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}
