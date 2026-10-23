//! helpers shared by the integration tests. each test file includes this module with `mod common;`
//! and uses only the part it needs, so unused items are allowed here.
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const BIN: &str = env!("CARGO_BIN_EXE_sudo-mudo-apt-opt-gop-stop-remove");

/// runs the binary with `stdin` as its input (empty means EOF).
pub fn run(args: &[&str], stdin: &str) -> Output {
    run_in(None, args, stdin)
}

/// like `run`, but from `cwd` when given, so relative paths resolve there.
pub fn run_in(cwd: Option<&Path>, args: &[&str], stdin: &str) -> Output {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().expect("failed to start binary");
    // a binary that rejects its arguments may exit before it reads stdin; the write then fails with a broken pipe.
    match child.stdin.take().unwrap().write_all(stdin.as_bytes()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(err) => panic!("failed to write stdin: {err}"),
    }
    child.wait_with_output().expect("failed to wait for binary")
}

pub fn write_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"sensitive data").unwrap();
    path
}

pub fn write_bytes(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

pub fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

pub fn assert_exit(out: &Output, expected: i32) {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// a tree of `files` small files plus the directory itself: `files + 1` objects.
pub fn make_tree(root: &Path, name: &str, files: usize) -> PathBuf {
    let tree = root.join(name);
    fs::create_dir(&tree).unwrap();
    for index in 0..files {
        write_file(&tree, &format!("f{index}.txt"));
    }
    tree
}

/// runs the binary with a file size limit of one block, so a log that grows past it cannot be written.
/// SIGXFSZ is ignored first, so the failed write is an error and does not end the process.
#[cfg(unix)]
pub fn run_with_one_block_file_limit(args: &[&str]) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg("trap '' XFSZ; ulimit -f 1 && exec \"$0\" \"$@\"")
        .arg(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to start sh")
}
