mod common;

use std::fs;

use common::*;

#[test]
fn exclude_keeps_matching_entries_and_their_directory() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("skip")).unwrap();
    let log = write_file(&tree, "a.log");
    let skipped = write_file(&tree.join("skip"), "b.txt");
    let erased = write_file(&tree, "c.txt");
    let args = ["-r", "--exclude", "*.log", "--exclude", "skip"];

    let mut dry_args = args.to_vec();
    dry_args.extend(["--dry-run", path_str(&tree)]);
    let dry = run(&dry_args, "");
    assert_exit(&dry, 0);
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(dry_stdout.contains("excluded by --exclude: 2"));
    assert!(dry_stdout.contains("directories kept because of --exclude: 1"));
    assert!(erased.exists());

    let mut erase_args = args.to_vec();
    erase_args.extend(["-f", path_str(&tree)]);
    let out = run(&erase_args, "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 entries excluded by --exclude"));
    assert!(log.exists());
    assert!(skipped.exists());
    assert!(!erased.exists());
    assert!(tree.exists());
}

#[test]
fn exclude_trailing_slash_means_the_name_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("build")).unwrap();
    fs::create_dir_all(tree.join("sub/build")).unwrap();
    let top = write_file(&tree.join("build"), "top.txt");
    let nested = write_file(&tree.join("sub/build"), "nested.txt");
    let erased = write_file(&tree, "c.txt");

    let out = run(&["-r", "-f", "--exclude", "build/", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(top.exists());
    assert!(nested.exists());
    assert!(!erased.exists());
    assert!(fs::symlink_metadata(tree.join("sub")).is_ok());
}

#[test]
fn exclude_refuses_an_absolute_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let file = write_file(&tree, "a.txt");

    let out = run(
        &["-r", "-f", "--exclude", "/tree/a.txt", path_str(&tree)],
        "",
    );

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn exclude_never_applies_to_the_argument() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.txt");

    let out = run(&["-r", "-f", "--exclude", "tree", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn exclude_without_recursive_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", "--exclude", "*.log", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}
