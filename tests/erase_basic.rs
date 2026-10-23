mod common;

use std::fs;

use common::*;

#[test]
fn dry_run_lists_files_and_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--dry-run", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would erase"));
    assert!(file.exists());
}

#[test]
fn force_erases_without_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[test]
fn recursive_erases_directory_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("nested")).unwrap();
    write_file(&tree, "a.txt");
    write_file(&tree.join("nested"), "b.txt");

    let out = run(&["-r", "-f", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[cfg(unix)]
#[test]
fn symlink_argument_removes_only_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = write_file(dir.path(), "target.txt");
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let out = run(&["-f", path_str(&link)], "");

    assert_exit(&out, 0);
    assert!(!link.exists());
    assert!(target.exists());
}

#[test]
fn read_only_file_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&file, perms).unwrap();

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[cfg(unix)]
#[test]
fn symlink_with_trailing_slash_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let kept = write_file(&target, "keep.txt");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let with_slash = format!("{}/", path_str(&link));

    let out = run(&["-r", "-f", with_slash.as_str()], "");

    assert_exit(&out, 0);
    assert!(kept.exists());
    assert!(fs::symlink_metadata(&link).is_err());
}

#[test]
fn repeated_path_is_erased_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let dry = run(&["--dry-run", path_str(&file), path_str(&file)], "");
    assert_exit(&dry, 0);
    assert_eq!(
        String::from_utf8_lossy(&dry.stdout)
            .matches("would erase")
            .count(),
        1
    );

    let out = run(&["-f", path_str(&file), path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());

    let other = write_file(dir.path(), "b.txt");
    let out = run_in(Some(dir.path()), &["-f", "./b.txt", "b.txt"], "");

    assert_exit(&out, 0);
    assert!(!other.exists());
}

#[test]
fn erase_prints_the_note_once_and_dry_run_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let note = "note: overwriting does not guarantee erasure";

    let dry = run(&["--dry-run", path_str(&file)], "");
    assert!(!String::from_utf8_lossy(&dry.stderr).contains(note));

    let out = run(&["-f", path_str(&file)], "");
    assert_exit(&out, 0);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).matches(note).count(),
        1
    );
}
