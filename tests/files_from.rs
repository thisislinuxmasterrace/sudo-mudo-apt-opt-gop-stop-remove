mod common;

use std::fs;

use common::*;

#[test]
fn files_from_reads_one_path_per_line_and_adds_the_positional_ones() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let spaced = write_file(dir.path(), "a b.txt");
    let plain = write_file(dir.path(), "c.txt");
    let extra = write_file(dir.path(), "d.txt");
    let list = write_bytes(
        lists.path(),
        "list.txt",
        format!("{}\n\n{}\n", path_str(&spaced), path_str(&plain)).as_bytes(),
    );

    let out = run(
        &["-f", "--files-from", path_str(&list), path_str(&extra)],
        "",
    );

    assert_exit(&out, 0);
    assert!(!spaced.exists());
    assert!(!plain.exists());
    assert!(!extra.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("note: 2 paths read from"));
}

#[test]
fn files_from_dash_reads_the_list_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(
        &["-f", "--files-from", "-"],
        &format!("{}\n", path_str(&file)),
    );

    assert_exit(&out, 0);
    assert!(!file.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("note: 1 paths read from stdin"));
}

#[test]
fn null_separated_list_erases_each_path() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let spaced = write_file(dir.path(), "a b.txt");
    let plain = write_file(dir.path(), "c.txt");
    let list = write_bytes(
        lists.path(),
        "list.bin",
        format!("{}\0{}\0", path_str(&spaced), path_str(&plain)).as_bytes(),
    );

    let out = run(&["-f", "-0", "--files-from", path_str(&list)], "");

    assert_exit(&out, 0);
    assert!(!spaced.exists());
    assert!(!plain.exists());
}

#[test]
fn empty_files_from_erases_nothing_and_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let keep = write_file(dir.path(), "keep.txt");
    let blank = write_bytes(lists.path(), "blank.txt", b"\n\n");
    let empty = write_bytes(lists.path(), "empty.txt", b"");

    for list in [&blank, &empty] {
        let out = run(&["-f", "--files-from", path_str(list)], "");

        assert_exit(&out, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("no paths were given"));
        assert!(keep.exists());
    }
}

#[test]
fn unreadable_files_from_is_an_error_before_anything_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let keep = write_file(dir.path(), "keep.txt");
    let missing = dir.path().join("no-such-list.txt");

    let out = run(
        &["-f", "--files-from", path_str(&missing), path_str(&keep)],
        "",
    );

    assert_exit(&out, 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no-such-list.txt"));
    assert!(keep.exists());
}

#[test]
fn null_without_files_from_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-0", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn files_from_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(
        &["--files-from", "-", "--wipe-free", path_str(dir.path())],
        "",
    );

    assert_exit(&out, 2);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn null_separated_list_keeps_a_newline_inside_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let odd = write_file(dir.path(), "x\ny.txt");
    let list = write_bytes(
        lists.path(),
        "list.bin",
        format!("{}\0", path_str(&odd)).as_bytes(),
    );

    let out = run(&["-f", "-0", "--files-from", path_str(&list)], "");

    assert_exit(&out, 0);
    assert!(!odd.exists());
}

#[test]
fn files_from_dash_without_force_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--files-from", "-"], &format!("{}\n", path_str(&file)));

    assert_exit(&out, 2);
    assert!(file.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs -f"));
}
