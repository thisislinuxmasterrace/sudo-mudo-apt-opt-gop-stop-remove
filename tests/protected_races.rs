#![cfg(unix)]

mod common;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn command(home: &Path) -> Command {
    let mut command = Command::new("sudo");
    command
        .args(["-n", "env"])
        .arg(format!("HOME={}", home.display()))
        .arg(common::BIN)
        .args(["apt", "opt", "gop", "stop", "remove"]);
    command
}

fn confirm_after(mut command: Command, change: impl FnOnce()) -> std::process::ExitStatus {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let mut byte = [0];
        while stderr.read(&mut byte).unwrap() == 1 {
            text.push(byte[0]);
            if text.ends_with(b"continue? [y/N] ") {
                send.send(()).unwrap();
            }
        }
        text
    });
    if receive.recv_timeout(Duration::from_secs(15)).is_err() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!(
            "confirmation was not reached: {}",
            String::from_utf8_lossy(&reader.join().unwrap())
        );
    }
    change();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let status = child.wait().unwrap();
    reader.join().unwrap();
    status
}

#[test]
fn parent_symlink_change_cannot_redirect_confirmed_erase() {
    let dir = tempfile::tempdir().unwrap();
    let ordinary = dir.path().join("ordinary");
    let protected = dir.path().join("protected");
    fs::create_dir_all(ordinary.join("home")).unwrap();
    fs::create_dir_all(protected.join("home")).unwrap();
    let home = protected.join("home");
    fs::write(home.join("sentinel"), b"keep").unwrap();
    fs::write(ordinary.join("home/data"), b"erase").unwrap();
    let link = dir.path().join("link");
    symlink(&ordinary, &link).unwrap();
    let mut command = command(&home);
    command.args(["-r", "--zero"]).arg(link.join("home"));
    let status = confirm_after(command, || {
        let replacement = dir.path().join("replacement");
        symlink(&protected, &replacement).unwrap();
        fs::rename(replacement, &link).unwrap();
    });
    assert!(status.success());
    assert_eq!(fs::read(home.join("sentinel")).unwrap(), b"keep");
    assert!(!ordinary.join("home").exists());
}

#[test]
fn replaced_leaf_is_kept_after_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selected");
    fs::write(&path, b"original").unwrap();
    let saved = dir.path().join("saved");
    let mut command = common::command();
    command.arg(&path);
    let status = confirm_after(command, || {
        fs::rename(&path, &saved).unwrap();
        fs::write(&path, b"replacement").unwrap();
    });
    assert_eq!(status.code(), Some(4));
    assert_eq!(fs::read(saved).unwrap(), b"original");
    assert_eq!(fs::read(path).unwrap(), b"replacement");
}

#[test]
fn recursive_ancestor_keeps_protected_home_unless_overridden() {
    for allow in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let home = tree.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("sentinel"), b"keep").unwrap();
        fs::write(tree.join("ordinary"), b"erase").unwrap();
        let mut command = command(&home);
        command.args(["-r", "-f", "--zero"]);
        if allow {
            command.arg("--i-know-what-i-am-doing");
        }
        let output = command.arg(&tree).output().unwrap();
        common::assert_exit(&output, if allow { 0 } else { 4 });
        if allow {
            assert!(!tree.exists());
        } else {
            assert_eq!(fs::read(home.join("sentinel")).unwrap(), b"keep");
            assert!(!tree.join("ordinary").exists());
        }
    }
}

#[test]
fn hard_link_into_a_protected_home_below_the_tree_is_external() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let home = tree.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("shared"), b"keep").unwrap();
    fs::hard_link(home.join("shared"), tree.join("linked")).unwrap();
    let mut command = command(&home);
    command.args(["-r", "-f", "--zero"]).arg(&tree);

    let output = command.output().unwrap();

    // the home is kept, so its name is not selected: the other name is an external link and stays.
    common::assert_exit(&output, 4);
    assert_eq!(fs::read(home.join("shared")).unwrap(), b"keep");
    assert_eq!(fs::read(tree.join("linked")).unwrap(), b"keep");
}

#[test]
fn renamed_home_stays_protected_after_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let home = tree.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("sentinel"), b"keep").unwrap();
    let moved = tree.join("moved");
    let mut command = command(&home);
    command.arg("-r").arg(&tree);
    let status = confirm_after(command, || fs::rename(&home, &moved).unwrap());
    assert_eq!(status.code(), Some(4));
    assert_eq!(fs::read(moved.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn selection_descriptor_exhaustion_keeps_every_target() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..40)
        .map(|index| {
            let path = dir.path().join(format!("file{index}"));
            fs::write(&path, b"keep").unwrap();
            path
        })
        .collect();
    let output = Command::new("sudo")
        .args([
            "-n",
            "sh",
            "-c",
            "ulimit -n 32; exec \"$@\"",
            "sh",
            common::BIN,
            "apt",
            "opt",
            "gop",
            "stop",
            "remove",
            "-f",
        ])
        .args(&paths)
        .output()
        .unwrap();
    common::assert_exit(&output, 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not retain selected paths"));
    for path in paths {
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
}
