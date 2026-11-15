// resource-limited traversal must stop safely, preserving entries beyond the limit.
#![cfg(target_os = "linux")]
mod common;

use common::*;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn limited(args: &[&str], descriptors: u64) -> std::process::Output {
    let mut command = Command::new(BIN);
    command
        .args(["apt", "opt", "gop", "stop", "remove"])
        .args(args)
        .stdin(Stdio::null());
    // safety: the child only calls async-signal-safe setrlimit before exec; no shared state is touched.
    unsafe {
        command.pre_exec(move || {
            for (resource, limit) in [
                (libc::RLIMIT_STACK, 256 * 1024),
                (libc::RLIMIT_NOFILE, descriptors),
                (libc::RLIMIT_CORE, 0),
            ] {
                let limits = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(resource, &limits) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    command.output().unwrap()
}

#[test]
fn deep_tree_with_small_stack_is_kept_instead_of_aborting() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    let mut deepest = tree.clone();
    for _ in 0..160 {
        deepest.push("x");
    }
    fs::create_dir_all(&deepest).unwrap();
    let sentinel = write_file(&deepest, "data");
    let preview = limited(&["-r", "--dry-run", path_str(&tree)], 256);
    assert_exit(&preview, 0);
    assert!(
        String::from_utf8_lossy(&preview.stdout).contains("directory nesting exceeds 64 levels")
    );
    let erased = limited(&["-rf", path_str(&tree)], 256);
    assert_exit(&erased, 4);
    assert!(
        String::from_utf8_lossy(&erased.stderr).contains("directory nesting exceeds 64 levels")
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"sensitive data");
}

#[test]
fn descriptor_exhaustion_preserves_deeper_contents() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    let mut deepest = tree.clone();
    for _ in 0..40 {
        deepest.push("x");
    }
    fs::create_dir_all(&deepest).unwrap();
    let sentinel = write_file(&deepest, "data");
    let erased = limited(&["-rf", path_str(&tree)], 24);
    assert_exit(&erased, 4);
    assert_eq!(fs::read(sentinel).unwrap(), b"sensitive data");
}
