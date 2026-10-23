// every test here needs unix: hard links, file modes and the mount table have no windows counterpart here.
#![cfg(unix)]

mod common;

use std::fs;

use common::*;

#[cfg(unix)]
#[test]
fn protected_path_needs_the_override_flag() {
    let refused = run(&["--dry-run", "/usr/bin/env"], "");
    assert_exit(&refused, 1);
    assert!(String::from_utf8_lossy(&refused.stderr).contains("protected system path"));

    let allowed = run(
        &["--dry-run", "--i-know-what-i-am-doing", "/usr/bin/env"],
        "",
    );
    assert_exit(&allowed, 0);
    assert!(
        String::from_utf8_lossy(&allowed.stderr).contains("--i-know-what-i-am-doing was given")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn mount_point_target_is_refused_before_anything_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let dry = run(
        &["--dry-run", "-r", "--i-know-what-i-am-doing", "/proc"],
        "",
    );
    assert_exit(&dry, 1);
    assert!(
        String::from_utf8_lossy(&dry.stderr).contains("is a mount point or on another filesystem")
    );
    assert!(!String::from_utf8_lossy(&dry.stdout).contains("would erase"));

    let out = run(
        &[
            "-f",
            "-r",
            "--i-know-what-i-am-doing",
            path_str(&file),
            "/proc",
        ],
        "",
    );
    assert_exit(&out, 1);
    assert!(file.exists());

    let unflagged = run(&["-f", "-r", path_str(&file), "/proc"], "");
    assert_exit(&unflagged, 1);
    assert!(file.exists());
}

#[cfg(unix)]
#[test]
fn hard_linked_file_is_refused_only_under_root() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let other = dir.path().join("other.txt");
    fs::hard_link(&file, &other).unwrap();

    let out = run(&["-f", path_str(&file)], "");

    if nix::unistd::geteuid().is_root() {
        // refused before the confirmation, so nothing is erased and no question is asked.
        assert_exit(&out, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("hard-linked file under root"));
        assert!(file.exists());

        let allowed = run(&["-f", "--i-know-what-i-am-doing", path_str(&file)], "");
        assert_exit(&allowed, 0);
        assert!(!file.exists());
    } else {
        assert_exit(&out, 0);
        assert!(!file.exists());
    }
    assert!(other.exists());
}

/// under root a hard-linked file found inside a tree is refused during the walk, not before it: the
/// tree is kept and the run ends with exit code 4. skipped when not running as root.
#[cfg(unix)]
#[test]
fn hard_linked_file_inside_a_tree_under_root_exits_with_four() {
    if !nix::unistd::geteuid().is_root() {
        eprintln!("skipped: the hard link rule applies under root only");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 0);
    let file = write_file(&tree, "a.txt");
    fs::hard_link(&file, dir.path().join("other.txt")).unwrap();

    let out = run(&["-f", "-r", path_str(&tree)], "");

    assert_exit(&out, 4);
    assert!(String::from_utf8_lossy(&out.stderr).contains("hard-linked file under root"));
    assert!(file.exists());
    assert!(tree.exists());
}
