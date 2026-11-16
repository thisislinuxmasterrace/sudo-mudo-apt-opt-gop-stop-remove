// --explain prints advice after the dry run and changes nothing.
mod common;

use common::*;
use std::fs;

#[test]
fn explain_prints_advice_and_erases_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "secret.txt");
    fs::write(dir.path().join("secret.txt~"), b"older").unwrap();
    let out = run(&["--explain", path_str(&file)], "");
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("would erase"), "{text}");
    assert!(text.contains("an overwrite is best effort"), "{text}");
    let backup = dir.path().join("secret.txt~");
    assert!(
        text.contains(&format!("editor backup: {} may hold", backup.display())),
        "{text}"
    );
    assert!(text.contains("nothing printed here proves"), "{text}");
    assert!(
        text.trim_end()
            .ends_with("nothing printed here proves that data is unrecoverable."),
        "the general limits come last: {text}"
    );
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
    assert!(dir.path().join("secret.txt~").exists());
}

#[test]
fn explain_does_not_ask_a_question() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    // stdin is empty: a question would be declined and end with code 3.
    let out = run(&["--explain", path_str(&file)], "");
    assert_exit(&out, 0);
    assert!(file.exists());
}

#[test]
fn explain_works_on_a_tree_and_names_each_condition_once() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 3);
    fs::create_dir(tree.join(".git")).unwrap();
    let out = run(&["--explain", "-r", path_str(&tree)], "");
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.matches("  - git:").count(), 1, "{text}");
    assert!(text.contains("is erased with its history"), "{text}");
    assert!(!text.contains("does not touch"), "{text}");
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 4);
}

#[test]
fn explain_keeps_git_history_advice_when_git_is_excluded_or_outside() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1);
    fs::create_dir(tree.join(".git")).unwrap();
    let out = run(
        &["--explain", "-r", "--exclude", ".git", path_str(&tree)],
        "",
    );
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("does not touch it"), "{text}");
    assert!(!text.contains("is erased with its history"), "{text}");

    let out = run(
        &["--explain", "-r", "--exclude", "*.pack", path_str(&tree)],
        "",
    );
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("erased with its history (`.git`), except entries that --exclude keeps"),
        "{text}"
    );

    let file = tree.join("f0.txt");
    let out = run(&["--explain", path_str(&file)], "");
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("does not touch it"), "{text}");
}

#[cfg(unix)]
#[test]
fn explain_tells_that_a_link_is_not_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = write_file(dir.path(), "target");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = run(&["--explain", path_str(&link)], "");
    assert_exit(&out, 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("symbolic link:"), "{text}");
    assert!(target.exists() && fs::symlink_metadata(&link).is_ok());
}

/// a file on tmpfs: the dry run warns that old pages may sit in swap, and --explain says so in one line.
/// needs root to mount; without it the test is skipped locally and fails under CI.
#[cfg(target_os = "linux")]
#[test]
fn tmpfs_target_gets_the_memory_filesystem_warning_and_advice() {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    // unmounts on drop, so a panic still lets `TempDir` remove the directory.
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
    let mount = dir.path().join("mem");
    fs::create_dir(&mount).unwrap();
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "tmpfs"])
        .arg(&mount)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !mounted {
        if std::env::var_os("CI").is_some() {
            panic!("a tmpfs could not be mounted: the memory filesystem case was not tested");
        }
        eprintln!("skipped: a tmpfs could not be mounted (not root?)");
        return;
    }
    let _mounted = Mounted(mount.clone());
    let file = write_file(&mount, "secret.txt");

    let out = run(&["--explain", path_str(&file)], "");

    assert_exit(&out, 0);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("memory filesystem (tmpfs, ramfs)"), "{text}");
    assert!(text.contains("old pages may sit in swap"), "{text}");
    assert!(stderr.contains("tmpfs is a memory filesystem"), "{stderr}");
    assert!(!text.contains("verdict\":\"effective"), "{text}");
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
}

#[test]
fn explain_conflicts_with_json_and_wipe_free() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    assert_exit(&run(&["--explain", "--json", path_str(&file)], ""), 2);
    assert_exit(
        &run(&["--explain", "--wipe-free", path_str(dir.path())], ""),
        2,
    );
    assert!(file.exists());
}
