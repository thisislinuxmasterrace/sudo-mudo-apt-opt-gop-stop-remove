//! erase tests across the whole path: files, trees, links, read-only entries, excludes and log failures.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::log::Log;
use crate::log::test_support::{Memory, Torn};

use super::Options;

fn erase_path(path: &Path, opts: &Options) -> io::Result<()> {
    super::erase_path(path, opts, &[], None).map(|_| ())
}

fn options(passes: u32) -> Options {
    Options {
        passes,
        verbose: false,
        force: true,
        kill_holders: false,
        allow_protected: false,
        quiet: true,
        verify: false,
    }
}

pub(super) fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    path
}

#[test]
fn erase_stops_after_a_log_line_cannot_be_written() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    for name in ["a", "b", "c"] {
        write_file(&tree, name, b"data");
    }
    let log = Log::new(
        Path::new("erase.log"),
        1,
        Box::new(Torn {
            memory: Memory::default(),
            room: 0,
        }),
    );

    let result = super::erase_path(&tree, &options(1), &[], Some(&log));

    assert!(result.is_err());
    assert!(log.failed());
    // the first object was erased before its line failed; the other two were not touched.
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 2);
}

#[test]
fn erase_removes_file_without_leaving_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "secret.txt", b"secret");

    erase_path(&path, &options(2)).unwrap();

    assert!(!path.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn erase_handles_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "empty.txt", b"");

    erase_path(&path, &options(1)).unwrap();

    assert!(!path.exists());
}

#[test]
fn erase_removes_directory_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("nested/deeper")).unwrap();
    write_file(&tree, "a.txt", b"a");
    write_file(&tree.join("nested/deeper"), "b.txt", b"b");

    erase_path(&tree, &options(1)).unwrap();

    assert!(!tree.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn erase_removes_read_only_file_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let file = write_file(&tree, "locked.txt", b"data");

    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&file, perms).unwrap();
    let mut perms = fs::metadata(&tree).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&tree, perms).unwrap();

    erase_path(&tree, &options(1)).unwrap();

    assert!(!tree.exists());
}

#[cfg(unix)]
#[test]
fn erase_removes_nested_read_only_directory() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let nested = tree.join("nested");
    fs::create_dir_all(&nested).unwrap();
    write_file(&nested, "data.txt", b"data");
    let mut perms = fs::metadata(&nested).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&nested, perms).unwrap();

    erase_path(&tree, &options(1)).unwrap();

    assert!(!tree.exists());
}

#[cfg(unix)]
#[test]
fn erase_removes_mode_000_file_and_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let nested = tree.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let file = write_file(&nested, "data.txt", b"data");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o000)).unwrap();

    erase_path(&tree, &options(1)).unwrap();

    assert!(!tree.exists());
}

#[cfg(unix)]
#[test]
fn symlink_is_removed_without_touching_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = write_file(dir.path(), "target.txt", b"keep me");
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    erase_path(&link, &options(1)).unwrap();

    assert!(!link.exists());
    assert_eq!(fs::read(&target).unwrap(), b"keep me");
}

#[cfg(unix)]
#[test]
fn symlink_inside_tree_does_not_erase_outside_target() {
    let outside = tempfile::tempdir().unwrap();
    let outside_file = write_file(outside.path(), "outside.txt", b"keep me");

    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.join("link")).unwrap();

    erase_path(&tree, &options(1)).unwrap();

    assert!(!tree.exists());
    assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
}

#[test]
fn excluded_directory_is_skipped_with_its_contents() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let skipped = tree.join("skip");
    fs::create_dir_all(&skipped).unwrap();
    let inside = write_file(&skipped, "inside.txt", b"keep me");
    let erased = write_file(&tree, "gone.txt", b"gone");
    let excludes = ["skip".to_owned()];

    let kept = super::erase_path(&tree, &options(1), &excludes, None).unwrap();

    assert!(kept);
    assert_eq!(fs::read(&inside).unwrap(), b"keep me");
    assert!(!erased.exists());
    assert!(tree.exists());
}
