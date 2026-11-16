use super::*;
use std::fs::OpenOptions;
use std::io::Write;

fn kept(path: &Path) -> ReadOnly {
    ReadOnly::clear(path).unwrap()
}

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

#[test]
fn hide_renames_through_the_handle_to_a_hidden_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();

    let hidden = hide(&file).unwrap();

    assert!(!path.exists());
    assert_eq!(fs::read(&hidden.hidden).unwrap(), b"data");
    let name = hidden.hidden.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with('.') && name.len() == 17, "{name}");
    // the handle still reaches the same file under its new name.
    assert_eq!(windows::link_count_of(&file).unwrap(), 1);
}

#[test]
fn name_goes_back_through_the_handle_when_a_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let mut keep = kept(&path);
    let file = windows::open_for_erase(&path).unwrap();
    let hidden = hide(&file).unwrap();

    let err = keep_under_name(Some(&hidden), &file, &mut keep, io::Error::other("boom"));

    assert_eq!(err.to_string(), "boom");
    assert_eq!(fs::read(&path).unwrap(), b"data");
    assert!(!hidden.hidden.exists());
}

#[test]
fn error_names_the_hidden_entry_when_the_name_cannot_be_given_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let mut keep = kept(&path);
    let file = windows::open_for_erase(&path).unwrap();
    let hidden = hide(&file).unwrap();
    // something else took the name meanwhile: it is not replaced.
    fs::write(&path, b"stranger").unwrap();

    let err = keep_under_name(Some(&hidden), &file, &mut keep, io::Error::other("boom"));

    assert!(err.to_string().contains("the entry is left as"), "{err}");
    assert_eq!(fs::read(&path).unwrap(), b"stranger");
    assert_eq!(fs::read(&hidden.hidden).unwrap(), b"data");
}

#[test]
fn delete_through_the_erase_handle_removes_the_name_while_it_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    let other = windows::open_for_erase(&path).unwrap();

    windows::posix_delete(&file).unwrap();
    drop(file);

    // the name went with the erase handle; another open handle does not keep it.
    assert!(!path.exists());
    fs::write(&path, b"new").unwrap();
    drop(other);
    assert_eq!(fs::read(&path).unwrap(), b"new");
}

#[test]
fn hide_follows_the_handle_when_the_name_is_taken_by_another_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    let moved = dir.path().join("moved");
    windows::rename_by_handle(&file, &moved).unwrap();
    fs::write(&path, b"stranger").unwrap();
    let expected = fs::canonicalize(&moved).unwrap();

    let hidden = hide(&file).unwrap();

    // the name to restore is the one the handle has now, not the stranger's.
    assert_eq!(hidden.original, expected);
    assert_eq!(fs::read(&path).unwrap(), b"stranger");
}

#[test]
fn classic_delete_runs_only_when_posix_delete_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    let refuse = |_: &File| -> io::Result<()> { Err(io::Error::other("no posix delete here")) };

    // posix delete works: the classic one is not touched.
    assert!(delete_through_with(&file, windows::posix_delete, refuse));
    drop(file);
    assert!(!path.exists());

    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    assert!(delete_through_with(&file, refuse, windows::delete_on_close));
    // the name goes when the handle closes.
    drop(file);
    assert!(!path.exists());

    fs::write(&path, b"data").unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    assert!(!delete_through_with(&file, refuse, refuse));
    drop(file);
    assert!(path.exists());
}

#[test]
fn path_delete_refuses_a_file_that_is_not_the_erased_one() {
    let dir = tempfile::tempdir().unwrap();
    let erased = dir.path().join("erased");
    let stranger = dir.path().join("stranger");
    fs::write(&erased, b"erased").unwrap();
    fs::write(&stranger, b"stranger").unwrap();
    let id = windows::file_id(&erased, true).unwrap();
    let mut keep = kept(&stranger);

    let err = remove_entry(&stranger, id, &options(), &mut keep).unwrap_err();

    assert!(err.to_string().contains("not the file"), "{err}");
    assert_eq!(fs::read(&stranger).unwrap(), b"stranger");

    let id = windows::file_id(&stranger, true).unwrap();
    remove_entry(&stranger, id, &options(), &mut keep).unwrap();
    assert!(!stranger.exists());
}

#[test]
fn error_names_the_hidden_entry() {
    let err = left_as(io::Error::other("boom"), Path::new(r"C:\dir\.0123"));

    assert!(err.to_string().starts_with("boom; the entry is left as "));
    assert!(err.to_string().contains(".0123"), "{err}");
}

#[test]
fn growth_after_the_last_pass_is_detected_through_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, vec![1; 64]).unwrap();
    let file = windows::open_for_erase(&path).unwrap();
    assert!(unchanged_len(&file, 64).is_ok());

    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"tail")
        .unwrap();

    let err = unchanged_len(&file, 64).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the file changed size during the overwrite, the file is kept"
    );
}

#[test]
fn read_only_guard_puts_the_flag_back_unless_the_entry_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms).unwrap();

    drop(kept(&path));
    assert!(fs::metadata(&path).unwrap().permissions().readonly());

    let mut guard = kept(&path);
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    guard.done();
    drop(guard);
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
}
