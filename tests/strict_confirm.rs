mod common;

use std::fs;

use common::*;

#[test]
fn large_recursive_erase_needs_the_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    for index in 0..1001 {
        write_file(&tree, &format!("f{index}.txt"));
    }

    let answered_yes = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&answered_yes, 3);
    let stderr = String::from_utf8_lossy(&answered_yes.stderr);
    assert!(stderr.contains("this is a large erase (1002 objects"));
    assert!(stderr.contains("type the name of the directory (tree)"));
    assert!(stderr.contains("aborted, nothing was erased."));
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1001);

    let answered_name = run(&["-r", path_str(&tree)], "tree\n");

    assert_exit(&answered_name, 0);
    assert!(!tree.exists());
}

#[test]
fn one_thousand_objects_take_yes() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 999);

    let out = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn one_thousand_one_objects_need_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1000);

    let answered_yes = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&answered_yes, 3);
    assert!(String::from_utf8_lossy(&answered_yes.stderr).contains("(1001 objects"));
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1000);

    let answered_name = run(&["-r", path_str(&tree)], "tree\n");

    assert_exit(&answered_name, 0);
    assert!(!tree.exists());
}

#[test]
fn mixed_run_asks_continue_after_the_large_name() {
    let dir = tempfile::tempdir().unwrap();
    let big = make_tree(dir.path(), "big", 1001);
    let small = make_tree(dir.path(), "small", 2);

    let without_continue = run(&["-r", path_str(&big), path_str(&small)], "big\n");

    assert_exit(&without_continue, 3);
    assert!(String::from_utf8_lossy(&without_continue.stderr).contains("continue?"));
    assert_eq!(fs::read_dir(&big).unwrap().count(), 1001);
    assert_eq!(fs::read_dir(&small).unwrap().count(), 2);

    let with_continue = run(&["-r", path_str(&big), path_str(&small)], "big\ny\n");

    assert_exit(&with_continue, 0);
    assert!(!big.exists());
    assert!(!small.exists());
}

#[test]
fn total_over_the_limit_asks_the_name_of_the_largest_path() {
    let dir = tempfile::tempdir().unwrap();
    // each path is under the limit (602 and 501 objects), the run is over it (1103).
    let big = make_tree(dir.path(), "big", 601);
    let small = make_tree(dir.path(), "small", 500);
    let args = ["-r", path_str(&big), path_str(&small)];

    let wrong_name = run(&args, "small\ny\n");

    assert_exit(&wrong_name, 3);
    assert_eq!(fs::read_dir(&big).unwrap().count(), 601);
    assert_eq!(fs::read_dir(&small).unwrap().count(), 500);

    let right_name = run(&args, "big\ny\n");

    assert_exit(&right_name, 0);
    assert!(!big.exists());
    assert!(!small.exists());
}

#[test]
fn small_recursive_erase_still_takes_yes() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.txt");

    let out = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}
