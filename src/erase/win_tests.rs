use super::locked::erase_locked_with;
use super::*;
use crate::erase::{link_count, validate_hard_link};
use crate::platform::windows;
use std::fs::File;
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;

fn options() -> Options {
    Options {
        passes: 1,
        zero: false,
        verbose: false,
        force: true,
        kill_holders: false,
        allow_protected: false,
        quiet: true,
        verify: true,
    }
}

fn set_readonly(path: &Path, readonly: bool) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(readonly);
    fs::set_permissions(path, perms).unwrap();
}

fn is_readonly(path: &Path) -> bool {
    fs::metadata(path).unwrap().permissions().readonly()
}

fn holder() -> windows::Holder {
    windows::Holder {
        pid: 0,
        name: "test holder".to_owned(),
        start: 0,
    }
}

#[test]
fn retry_uses_size_after_holder_changes_file_before_closing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let mut opts = options();
    opts.kill_holders = true;
    for (before, after) in [(8, 4096), (4096, 8)] {
        fs::write(&path, vec![42; before]).unwrap();
        let mut observer = File::open(&path).unwrap();
        let listing = Ok(vec![holder()]);
        erase_locked_with(
            &path,
            &opts,
            io::Error::from_raw_os_error(32),
            listing,
            |_| {
                fs::write(&path, vec![42; after])?;
                Ok(())
            },
            &mut ReadOnly::clear(&path).unwrap(),
        )
        .unwrap();
        assert!(!path.exists());
        let mut contents = Vec::new();
        observer.read_to_end(&mut contents).unwrap();
        assert_eq!(contents.len(), after);
        let changed = if after > before {
            &contents[before..]
        } else {
            &contents[..]
        };
        assert!(changed.iter().any(|&byte| byte != 42));
    }
}

#[test]
fn holders_that_changed_since_the_listing_keep_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let mut opts = options();
    opts.kill_holders = true;

    let err = erase_locked_with(
        &path,
        &opts,
        io::Error::from_raw_os_error(32),
        Ok(vec![holder()]),
        |shown| {
            assert_eq!(shown.len(), 1);
            Err(io::Error::other("the processes holding the file changed"))
        },
        &mut ReadOnly::clear(&path).unwrap(),
    )
    .unwrap_err();

    assert!(err.to_string().contains("changed"));
    assert_eq!(fs::read(&path).unwrap(), b"data");
}

#[test]
fn named_stream_contents_are_overwritten_before_unlink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"main").unwrap();
    let stream = dir.path().join("data:secret");
    fs::write(&stream, vec![42; 2_500_017]).unwrap();
    let mut observer = File::open(&stream).unwrap();
    let mut opts = options();
    opts.passes = 2;
    opts.zero = true;
    erase_file(&path, &opts).unwrap();
    assert!(!path.exists());
    let mut bytes = Vec::new();
    observer.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len(), 2_500_017);
    assert!(bytes.iter().all(|&byte| byte == 0));
}

#[test]
fn locked_stream_keeps_main_contents_and_directory_streams_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"main").unwrap();
    let stream = dir.path().join("data:secret");
    fs::write(&stream, b"secret").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&stream)
        .unwrap();
    assert!(erase_file(&path, &options()).is_err());
    drop(locked);
    assert_eq!(fs::read(&path).unwrap(), b"main");
    assert_eq!(fs::read(&stream).unwrap(), b"secret");

    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let child = tree.join("child");
    fs::write(&child, b"child").unwrap();
    fs::write(dir.path().join("tree:secret"), b"directory secret").unwrap();
    assert!(erase_path(&tree, &options(), &[], None).is_err());
    assert_eq!(fs::read(child).unwrap(), b"child");
}

#[test]
fn erased_file_leaves_no_hidden_name_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, vec![7; 4096]).unwrap();
    let observer = File::open(&path).unwrap();

    erase_file(&path, &options()).unwrap();

    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    drop(observer);
}

#[test]
fn read_only_file_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    set_readonly(&path, true);

    erase_file(&path, &options()).unwrap();

    assert!(!path.exists());
}

#[test]
fn read_only_flag_is_restored_when_a_locked_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"keep me").unwrap();
    set_readonly(&path, true);
    let holder = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();

    assert!(erase_file(&path, &options()).is_err());

    drop(holder);
    assert!(is_readonly(&path));
    assert_eq!(fs::read(&path).unwrap(), b"keep me");
    set_readonly(&path, false);
}

#[test]
fn read_only_flag_is_restored_on_a_directory_that_stays() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("keep.txt"), b"kept").unwrap();
    fs::write(tree.join("gone.txt"), b"gone").unwrap();
    set_readonly(&tree, true);

    let kept = erase_path(&tree, &options(), &["keep.txt".to_owned()], None).unwrap();

    assert!(kept);
    assert!(!tree.join("gone.txt").exists());
    assert!(tree.join("keep.txt").exists());
    assert!(is_readonly(&tree));
    set_readonly(&tree, false);
}

#[test]
fn walk_keeps_a_protected_directory_and_its_tree_unless_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let guarded = tree.join("guarded");
    fs::create_dir_all(&guarded).unwrap();
    fs::write(guarded.join("inner.txt"), b"inner").unwrap();
    fs::write(tree.join("plain.txt"), b"plain").unwrap();
    let protected = Protected::of([guarded.clone()]);

    let err = erase_path_with(&tree, &options(), &[], None, &protected).unwrap_err();

    assert!(err.to_string().contains("kept"), "{err}");
    assert_eq!(fs::read(guarded.join("inner.txt")).unwrap(), b"inner");
    assert!(!tree.join("plain.txt").exists());

    let mut allowed = options();
    allowed.allow_protected = true;
    assert!(!erase_path_with(&tree, &allowed, &[], None, &protected).unwrap());
    assert!(!tree.exists());
}

#[test]
fn hard_links_are_counted_and_never_refuse_an_erase() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, [7u8; 100]).unwrap();
    assert_eq!(link_count(&a, &fs::metadata(&a).unwrap()), 1);
    fs::hard_link(&a, &b).unwrap();
    assert_eq!(link_count(&a, &fs::metadata(&a).unwrap()), 2);

    let sums = crate::preview::summarize(&[a.clone(), b.clone()], false, &[], false);
    assert_eq!(sums.iter().map(|sum| sum.hard_linked).sum::<u64>(), 2);
    assert_eq!(sums.iter().map(|sum| sum.bytes).sum::<u64>(), 100);

    assert!(validate_hard_link(&a, false).is_ok());
    erase_file(&a, &options()).unwrap();
    assert!(!a.exists());
    let rest = fs::read(&b).unwrap();
    assert_eq!(rest.len(), 100);
    assert!(rest.iter().any(|&byte| byte != 7));
    assert_eq!(link_count(&b, &fs::metadata(&b).unwrap()), 1);
}
