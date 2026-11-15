//! an entry swapped between the check and the act: the hooks in `at::seam` swap a name at that point,
//! and the erase must neither follow the new entry nor delete or overwrite it.

use std::ffi::{CStr, OsStr};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::Options;
use super::at::seam::{self, Stage};
use super::tests::write_file;

fn options() -> Options {
    Options {
        passes: 1,
        zero: false,
        verbose: false,
        force: true,
        kill_holders: false,
        allow_protected: false,
        quiet: true,
        verify: false,
    }
}

fn erase(path: &Path) -> std::io::Result<bool> {
    super::erase_path(path, &options(), &[], None)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).unwrap().file_type().is_symlink()
}

/// moves `path` to `moved` in the same directory and puts what `make` creates in its place.
fn swap(
    path: PathBuf,
    moved: &'static str,
    make: impl FnOnce(&Path) + 'static,
) -> impl FnOnce(&CStr) {
    move |_| {
        fs::rename(&path, path.with_file_name(moved)).unwrap();
        make(&path);
    }
}

/// a `make` for `swap`: a symbolic link to `target`.
fn link_to(target: PathBuf) -> impl FnOnce(&Path) {
    move |path| symlink(&target, path).unwrap()
}

/// a `make` for `swap`: a new regular file holding `data`.
fn file_with(data: &'static [u8]) -> impl FnOnce(&Path) {
    move |path| fs::write(path, data).unwrap()
}

#[test]
fn file_swapped_for_a_symlink_before_hiding_is_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    let outside_file = write_file(outside.path(), "outside.txt", b"keep me");
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let make = link_to(outside_file.clone());
    let hook = swap(file.clone(), "moved", make);
    seam::set(Stage::Hide, Some("a.txt"), hook);

    let err = erase(&file).unwrap_err();

    assert!(err.to_string().contains("entry changed"), "{err}");
    assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
    assert!(is_link(&file));
    assert_eq!(names(dir.path()), ["a.txt", "moved"]);
}

#[test]
fn file_swapped_for_another_file_before_hiding_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let make = file_with(b"other");
    let hook = swap(file.clone(), "moved", make);
    seam::set(Stage::Hide, Some("a.txt"), hook);

    let err = erase(&file).unwrap_err();

    assert!(err.to_string().contains("entry changed"), "{err}");
    assert_eq!(fs::read(&file).unwrap(), b"other");
    assert_eq!(names(dir.path()), ["a.txt", "moved"]);
}

#[test]
fn hidden_name_swapped_for_a_symlink_before_the_overwrite_is_not_followed() {
    let outside = tempfile::tempdir().unwrap();
    let outside_file = write_file(outside.path(), "outside.txt", b"keep me");
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let (root, target) = (dir.path().to_path_buf(), outside_file.clone());
    seam::set(Stage::Open, None, move |hidden| {
        let hidden = root.join(OsStr::from_bytes(hidden.to_bytes()));
        fs::rename(&hidden, root.join("moved")).unwrap();
        symlink(&target, &hidden).unwrap();
    });

    assert!(erase(&file).is_err());

    assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
    assert_eq!(fs::read(dir.path().join("moved")).unwrap(), b"secret");
    // the symlink that took the hidden name is put back under the original name.
    assert!(is_link(&file));
    assert_eq!(names(dir.path()), ["a.txt", "moved"]);
}

#[test]
fn directory_swapped_for_a_symlink_is_not_entered() {
    let outside = tempfile::tempdir().unwrap();
    let outside_file = write_file(outside.path(), "outside.txt", b"keep me");
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    write_file(&tree.join("sub"), "inner.txt", b"secret");
    let make = link_to(outside.path().to_path_buf());
    let hook = swap(tree.join("sub"), "moved", make);
    seam::set(Stage::Open, Some("sub"), hook);

    assert!(erase(&tree).is_err());

    assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
    assert!(is_link(&tree.join("sub")));
    assert_eq!(fs::read(tree.join("moved/inner.txt")).unwrap(), b"secret");
}

#[test]
fn hard_link_added_before_open_is_refused_without_overwriting() {
    if !super::running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let outside = tempfile::tempdir().unwrap();
    let other = outside.path().join("other.txt");
    let (root, target) = (dir.path().to_path_buf(), other.clone());
    seam::set(Stage::Open, None, move |hidden| {
        fs::hard_link(root.join(OsStr::from_bytes(hidden.to_bytes())), &target).unwrap();
    });
    assert!(erase(&file).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"secret");
    assert_eq!(fs::read(other).unwrap(), b"secret");
}

#[test]
fn symlink_swapped_for_a_file_is_not_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let target = write_file(dir.path(), "target.txt", b"target");
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    let make = file_with(b"new data");
    seam::set(Stage::Hide, Some("link"), swap(link.clone(), "moved", make));

    let err = erase(&link).unwrap_err();

    assert!(err.to_string().contains("entry changed"), "{err}");
    assert_eq!(fs::read(&link).unwrap(), b"new data");
    assert_eq!(names(dir.path()), ["link", "moved", "target.txt"]);
}

#[test]
fn emptied_directory_swapped_before_removal_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.txt", b"secret");
    let make = |path: &Path| fs::create_dir(path).unwrap();
    seam::set(Stage::Hide, Some("tree"), swap(tree.clone(), "moved", make));

    let err = erase(&tree).unwrap_err();

    assert!(err.to_string().contains("entry changed"), "{err}");
    assert!(tree.is_dir());
    assert_eq!(names(dir.path()), ["moved", "tree"]);
    assert!(names(&dir.path().join("moved")).is_empty());
}

#[test]
fn put_back_does_not_replace_a_name_taken_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let make = file_with(b"swapped");
    let hook = swap(file.clone(), "moved", make);
    seam::set(Stage::Hide, Some("a.txt"), hook);
    let late = file.clone();
    seam::set(Stage::PutBack, Some("a.txt"), move |_| {
        fs::write(&late, b"late").unwrap();
    });

    let err = erase(&file).unwrap_err();

    assert!(err.to_string().contains("the entry is left as"), "{err}");
    assert_eq!(fs::read(&file).unwrap(), b"late");
    assert_eq!(fs::read(dir.path().join("moved")).unwrap(), b"secret");
    let hidden: Vec<String> = names(dir.path())
        .into_iter()
        .filter(|name| name.starts_with('.'))
        .collect();
    assert_eq!(hidden.len(), 1);
    assert_eq!(fs::read(dir.path().join(&hidden[0])).unwrap(), b"swapped");
}

/// linux uses RENAME_NOREPLACE; the planted entry must survive a hidden-name collision.
#[cfg(target_os = "linux")]
#[test]
fn hidden_name_taken_after_it_was_picked_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt", b"secret");
    let root = dir.path().to_path_buf();
    let planted = std::rc::Rc::new(std::cell::RefCell::new(PathBuf::new()));
    let seen = planted.clone();
    seam::set(Stage::HiddenName, None, move |hidden| {
        let path = root.join(OsStr::from_bytes(hidden.to_bytes()));
        fs::write(&path, b"planted").unwrap();
        *seen.borrow_mut() = path;
    });

    erase(&file).unwrap();

    let planted = planted.borrow();
    assert_eq!(fs::read(&*planted).unwrap(), b"planted");
    assert!(!file.exists());
    assert_eq!(names(dir.path()).len(), 1);
}
