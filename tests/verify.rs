mod common;

use std::fs;

use common::*;

#[test]
fn verify_erases_the_file_and_prints_the_note() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--verify", "-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("note: --verify reads the data back and compares it")
    );
}

#[test]
fn verify_erases_a_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("nested")).unwrap();
    write_file(&tree, "a.txt");
    write_file(&tree.join("nested"), "b.txt");

    let out = run(&["-r", "--verify", "-f", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn verify_with_several_passes_and_chunks_erases_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.bin");
    fs::write(&file, vec![0xAB; 2_500_000]).unwrap();

    let out = run(&["-n", "3", "--verify", "-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[test]
fn verify_dry_run_prints_no_note_and_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--verify", "--dry-run", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("--verify reads"));
    assert!(file.exists());
}

#[test]
fn verify_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["--wipe-free", path_str(dir.path()), "--verify"], "");

    assert_exit(&out, 2);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

// /dev/null is a character device: the device rules must refuse it before anything is done. the name
// contains `device` so that `--skip device` leaves this test out of the vm run.
#[cfg(unix)]
#[test]
fn verify_on_a_device_is_refused_before_anything_is_done() {
    use std::path::Path;

    let out = run(&["--verify", "-f", "/dev/null"], "");

    assert_exit(&out, 1);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--verify is not supported for devices yet")
    );
    assert!(Path::new("/dev/null").exists());
}
