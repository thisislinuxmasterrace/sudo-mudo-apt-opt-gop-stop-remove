// --wipe-free argument handling and refusals; none of these tests fills a disk.
// run in the vm only: cargo test --locked --test wipe_free
mod common;

use std::fs;

use common::*;

#[test]
fn wipe_free_rejects_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--wipe-free", path_str(&file), "-f"], "");

    assert_exit(&out, 1);
    assert!(file.exists());
}

#[test]
fn wipe_free_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["--wipe-free", path_str(dir.path()), "--dry-run"], "");

    assert_exit(&out, 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("would fill"));
    assert!(stdout.contains("free space (including blocks reserved for root):"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn wipe_free_dry_run_follows_a_link_to_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let out = run(&["--wipe-free", path_str(&link), "--dry-run"], "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would fill"));
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

#[test]
fn wipe_free_declined_exits_with_three() {
    let dir = tempfile::tempdir().unwrap();

    let declined = run(&["--wipe-free", path_str(dir.path())], "n\n");
    assert_exit(&declined, 3);

    let eof = run(&["--wipe-free", path_str(dir.path())], "");
    assert_exit(&eof, 3);

    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// a small tmpfs: the refusal comes before any write, so nothing is filled. needs root to mount;
/// without it the test is skipped locally and fails under CI (where the variable `CI` is set).
#[cfg(target_os = "linux")]
#[test]
fn wipe_free_refuses_a_memory_filesystem() {
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
    let mount = dir.path().join("mnt");
    fs::create_dir(&mount).unwrap();
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "-o", "size=1m", "tmpfs"])
        .arg(&mount)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !mounted {
        if std::env::var_os("CI").is_some() {
            panic!("a tmpfs could not be mounted: the refusal was not tested");
        }
        eprintln!("skipped: a tmpfs could not be mounted (not root?)");
        return;
    }
    let _mounted = Mounted(mount.clone());

    let out = run(&["--wipe-free", path_str(&mount), "-f"], "");

    assert_exit(&out, 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("refusing to fill a memory filesystem"));
    assert_eq!(fs::read_dir(&mount).unwrap().count(), 0);
}
