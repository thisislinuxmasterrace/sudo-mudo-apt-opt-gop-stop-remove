// hard link rule and mount point refusals. unix only: no windows counterpart.
// run in the vm only (tests erase files): cargo test --locked --test hard_links_and_mounts
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
fn user_data_directories_are_refused_as_erase_targets() {
    for root in ["/home", "/root", "/srv", "/mnt", "/media", "/run"] {
        if !std::path::Path::new(root).is_dir() {
            continue;
        }
        let refused = run(&["--dry-run", "-r", root], "");
        assert_exit(&refused, 1);
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("protected system path"),
            "{root}"
        );
    }
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

    let dry = run(&["-r", "--dry-run", "--json", path_str(&tree)], "");
    assert_exit(&dry, 0);
    assert!(
        String::from_utf8_lossy(&dry.stdout).contains("\"assessment_coverage\":\"incomplete\"")
    );
    let allowed = run(
        &[
            "-r",
            "--dry-run",
            "--json",
            "--i-know-what-i-am-doing",
            path_str(&tree),
        ],
        "",
    );
    assert_exit(&allowed, 0);
    assert!(
        String::from_utf8_lossy(&allowed.stdout)
            .contains("\"assessment_coverage\":\"visited_entries\"")
    );

    let out = run(&["-f", "-r", path_str(&tree)], "");

    assert_exit(&out, 4);
    assert!(String::from_utf8_lossy(&out.stderr).contains("hard-linked file under root"));
    assert!(file.exists());
    assert!(tree.exists());
}

#[test]
fn cargo_style_internal_hard_links_are_erased_recursively() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "project", 0);
    let deps = tree.join("target/debug/deps");
    fs::create_dir_all(&deps).unwrap();
    let file = write_file(&deps, "app-hash");
    fs::hard_link(&file, tree.join("target/debug/app")).unwrap();
    fs::hard_link(&file, deps.join("app-other-hash")).unwrap();
    let mut observer = fs::File::open(&file).unwrap();
    let log = dir.path().join("erase.log");
    let out = run(
        &[
            "-rf",
            "-n",
            "3",
            "--verify",
            "--log",
            path_str(&log),
            path_str(&tree),
        ],
        "",
    );
    assert_exit(&out, 0);
    assert!(!tree.exists());
    let mut contents = Vec::new();
    observer.read_to_end(&mut contents).unwrap();
    assert_eq!(contents.len(), b"sensitive data".len());
    assert_ne!(contents, b"sensitive data");
    let records = fs::read_to_string(log).unwrap();
    assert_eq!(
        records
            .matches("internal hard link removed; not overwritten here")
            .count(),
        2
    );
    for line in records
        .lines()
        .filter(|line| line.contains("internal hard link removed"))
    {
        assert!(line.contains("\"passes\":0"), "{line}");
    }
    assert_eq!(
        records
            .lines()
            .filter(|line| line.contains("\"kind\":\"file\"") && line.contains("\"passes\":3"))
            .count(),
        1
    );
}

#[test]
fn external_and_excluded_hard_links_keep_original_contents() {
    for excluded in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let tree = make_tree(dir.path(), "project", 0);
        let file = write_file(&tree, "app");
        let other = if excluded {
            tree.join("keep")
        } else {
            dir.path().join("outside")
        };
        fs::hard_link(&file, &other).unwrap();
        let mut args = vec!["-rf", path_str(&tree)];
        if excluded {
            args.extend(["--exclude", "keep"]);
        }
        let out = run(&args, "");
        assert_exit(&out, 4);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(path_str(&file)), "{stderr}");
        assert!(stderr.contains("hard-linked file under root"), "{stderr}");
        assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
        assert_eq!(fs::read(&other).unwrap(), b"sensitive data");
    }
}

/// a tmpfs mounted inside the tree: its content stays, the holding directory stays, the rest of the
/// tree is erased and the run ends with exit code 4. needs root to mount; without it the test is
/// skipped locally and fails under CI (where the variable `CI` is set).
#[cfg(target_os = "linux")]
#[test]
fn mount_point_inside_a_tree_is_skipped_and_its_content_kept() {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    // unmounts on drop, so a panic still lets `TempDir` remove the tree.
    struct Mounted(PathBuf);
    impl Drop for Mounted {
        fn drop(&mut self) {
            let unmounted = |extra: &[&str]| {
                Command::new("umount")
                    .args(extra)
                    .arg(&self.0)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            };
            if !unmounted(&[]) {
                unmounted(&["-l"]);
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 2);
    let mount = tree.join("mnt");
    let sub = tree.join("sub");
    fs::create_dir(&mount).unwrap();
    fs::create_dir(&sub).unwrap();
    write_file(&sub, "deep.txt");
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "tmpfs"])
        .arg(&mount)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !mounted {
        if std::env::var_os("CI").is_some() {
            panic!("a tmpfs could not be mounted: the mount point case was not tested");
        }
        eprintln!("skipped: a tmpfs could not be mounted (not root?)");
        return;
    }
    let _mounted = Mounted(mount.clone());
    let inner = write_file(&mount, "inner.txt");

    let out = run(&["-r", "-f", path_str(&tree)], "");

    assert_exit(&out, 4);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mount point or on another filesystem"),
        "{stderr}"
    );
    assert_eq!(fs::read(&inner).unwrap(), b"sensitive data");
    assert!(mount.is_dir() && tree.is_dir());
    assert!(!tree.join("f0.txt").exists() && !tree.join("f1.txt").exists());
    assert!(!sub.exists());
}
