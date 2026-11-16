use super::*;

fn holder(pid: u32, start: u64) -> Holder {
    Holder {
        pid,
        name: String::new(),
        start,
    }
}

#[test]
fn holders_may_only_shrink_matched_by_pid_and_start_time() {
    let shown = [holder(1, 10), holder(2, 20)];

    assert!(only_shown(&shown, &[holder(2, 20), holder(1, 10)]));
    // a listed program that was closed meanwhile is fine.
    assert!(only_shown(&shown, &[holder(1, 10)]));
    assert!(only_shown(&shown, &[]));
    // a new holder was never shown.
    assert!(!only_shown(&shown, &[holder(1, 10), holder(3, 30)]));
    assert!(!only_shown(&[], &[holder(1, 10)]));
    // a reused pid has another start time.
    assert!(!only_shown(&shown, &[holder(1, 10), holder(2, 21)]));
}

#[test]
fn rename_buffer_is_nul_terminated_for_every_name_length_residue() {
    for len in 1..=9usize {
        let name: Vec<u16> = (0..len).map(|i| 0x61 + i as u16).collect();
        let (buf, size) = rename_info(&name);
        let header = offset_of!(FILE_RENAME_INFO, FileName);

        assert!(size >= header + len * 2 + 2 && size <= buf.len() * 8);
        // safety: buf is a live, zeroed and aligned FILE_RENAME_INFO buffer of at least `size` bytes.
        let (declared, units) = unsafe {
            let info = buf.as_ptr().cast::<FILE_RENAME_INFO>();
            let units = std::slice::from_raw_parts(
                buf.as_ptr().cast::<u8>().add(header).cast::<u16>(),
                len + 1,
            );
            ((*info).FileNameLength, units.to_vec())
        };
        assert_eq!(declared as usize, len * 2);
        assert_eq!(&units[..len], &name[..]);
        assert_eq!(units[len], 0);
    }
}

#[test]
fn rename_by_handle_round_trips_names_of_every_length() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = open_for_erase(&path).unwrap();

    // the full path length takes every residue modulo 4 as the name grows.
    for len in 1..=9 {
        let moved = dir.path().join("n".repeat(len));
        rename_by_handle(&file, &moved).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&moved).unwrap(), b"data");
        rename_by_handle(&file, &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"data");
        assert!(!moved.exists());
    }
}

#[test]
fn delete_on_close_removes_the_file_when_the_handle_closes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = open_for_erase(&path).unwrap();

    delete_on_close(&file).unwrap();
    drop(file);

    assert!(!path.exists());
}

#[test]
fn final_path_names_the_entry_the_handle_holds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = open_for_erase(&path).unwrap();
    let moved = dir.path().join("moved");

    assert_eq!(
        final_path_of(&file).unwrap(),
        fs::canonicalize(&path).unwrap()
    );
    rename_by_handle(&file, &moved).unwrap();
    fs::write(&path, b"stranger").unwrap();

    assert_eq!(
        final_path_of(&file).unwrap(),
        fs::canonicalize(&moved).unwrap()
    );
}

#[test]
fn reparse_points_are_told_apart_from_plain_entries() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    fs::create_dir(&plain).unwrap();

    assert!(!is_reparse_point(&open_entry(&plain).unwrap()).unwrap());
}

#[test]
fn rename_by_handle_changes_the_name_and_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let moved = dir.path().join(".moved");
    let other = dir.path().join("other");
    fs::write(&path, b"data").unwrap();
    fs::write(&other, b"other").unwrap();
    let file = open_for_erase(&path).unwrap();

    rename_by_handle(&file, &moved).unwrap();
    assert!(!path.exists());
    assert_eq!(fs::read(&moved).unwrap(), b"data");

    assert!(rename_by_handle(&file, &other).is_err());
    assert_eq!(fs::read(&other).unwrap(), b"other");
    assert_eq!(fs::read(&moved).unwrap(), b"data");

    rename_by_handle(&file, &path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"data");
}

#[test]
fn rename_by_handle_accepts_a_verbatim_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"data").unwrap();
    let file = open_for_erase(&path).unwrap();
    let moved = fs::canonicalize(dir.path()).unwrap().join(".moved");

    rename_by_handle(&file, &moved).unwrap();

    assert_eq!(fs::read(dir.path().join(".moved")).unwrap(), b"data");
}

#[test]
fn file_identity_and_link_count_come_from_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let c = dir.path().join("c");
    fs::write(&a, b"a").unwrap();
    fs::write(&c, b"c").unwrap();
    assert_eq!(link_count_at(&a), Some(1));
    fs::hard_link(&a, &b).unwrap();

    assert_eq!(link_count_at(&a), Some(2));
    assert_eq!(link_count_at(&dir.path().join("missing")), None);
    assert_eq!(file_id(&a, false).unwrap(), file_id(&b, false).unwrap());
    assert_ne!(file_id(&a, false).unwrap(), file_id(&c, false).unwrap());
    assert_eq!(
        file_id_of(&open_for_erase(&a).unwrap()).unwrap(),
        file_id(&a, true).unwrap()
    );
}
