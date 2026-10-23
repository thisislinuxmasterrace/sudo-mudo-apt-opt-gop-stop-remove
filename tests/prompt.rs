mod common;

use common::*;

#[test]
fn accepted_prompt_erases_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "y\n");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[test]
fn declined_prompt_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "n\n");

    assert_exit(&out, 3);
    assert!(file.exists());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("stdin is not a terminal"));
}

#[test]
fn empty_answer_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "");

    assert_exit(&out, 3);
    assert!(file.exists());
}

#[test]
fn eof_without_terminal_points_to_force() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "");

    assert_exit(&out, 3);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("stdin is not a terminal: use -f to erase without a prompt")
    );
    assert!(file.exists());
}
