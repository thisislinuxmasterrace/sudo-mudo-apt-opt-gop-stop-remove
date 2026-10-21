use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_sudo-mudo-apt-opt-gop-stop-remove");

/// runs the binary with `stdin` as its input (empty means EOF).
fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start binary");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().expect("failed to wait for binary")
}

fn write_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"sensitive data").unwrap();
    path
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn dry_run_lists_files_and_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--dry-run", path_str(&file)], "");

    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("would erase"));
    assert!(file.exists());
}

#[test]
fn force_erases_without_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", path_str(&file)], "");

    assert!(out.status.success());
    assert!(!file.exists());
}

#[test]
fn accepted_prompt_erases_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "y\n");

    assert!(out.status.success());
    assert!(!file.exists());
}

#[test]
fn declined_prompt_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "n\n");

    assert!(!out.status.success());
    assert!(file.exists());
}

#[test]
fn empty_answer_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "");

    assert!(!out.status.success());
    assert!(file.exists());
}

#[test]
fn one_invalid_path_aborts_whole_run() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write_file(dir.path(), "a.txt");
    let missing = dir.path().join("missing.txt");

    let out = run(&["-f", path_str(&valid), path_str(&missing)], "");

    assert!(!out.status.success());
    assert!(valid.exists());
}

#[test]
fn directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["-f", path_str(dir.path())], "");

    assert!(!out.status.success());
    assert!(dir.path().exists());
}

#[test]
fn recursive_erases_directory_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("nested")).unwrap();
    write_file(&tree, "a.txt");
    write_file(&tree.join("nested"), "b.txt");

    let out = run(&["-r", "-f", path_str(&tree)], "");

    assert!(out.status.success());
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

    assert!(out.status.success());
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

    assert!(out.status.success());
    assert!(!file.exists());
}

#[test]
fn zero_passes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-n", "0", "-f", path_str(&file)], "");

    assert!(!out.status.success());
    assert!(file.exists());
}
