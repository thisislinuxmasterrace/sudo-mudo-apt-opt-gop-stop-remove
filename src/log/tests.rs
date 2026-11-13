use super::*;
use test_support::{Memory, Torn};

#[test]
fn utc_time_matches_known_values() {
    assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339_utc(1_709_251_199), "2024-02-29T23:59:59Z");
    assert_eq!(rfc3339_utc(1_709_251_200), "2024-03-01T00:00:00Z");
    assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z");
}

#[test]
fn line_has_the_fields_in_order_and_escapes_them() {
    let line = line(Path::new("a\"b"), Kind::File, 7, 3, Outcome::Error, "x\\y");

    let tail =
        r#","path":"a\"b","kind":"file","size":7,"passes":3,"result":"error","detail":"x\\y"}"#;
    assert!(line.starts_with("{\"time\":\""));
    assert!(line.ends_with(&format!("{tail}\n")), "{line}");
}

#[test]
fn interrupted_is_written_once_per_run() {
    let memory = Memory::default();
    let log = Log::new(Path::new("erase.log"), 1, Box::new(memory.clone()));

    log.record_interrupted(Path::new("dir/a.txt"), Kind::File, 5);
    log.record_interrupted(Path::new("dir"), Kind::Dir, 0);

    let text = memory.text();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("\"path\":\"dir/a.txt\""));
    assert!(text.contains("\"result\":\"error\",\"detail\":\"interrupted\""));
}

#[test]
fn a_line_that_fails_half_way_is_cut_off() {
    let memory = Memory::default();
    // room for the first line and part of the second one.
    let log = Log::new(
        Path::new("erase.log"),
        1,
        Box::new(Torn {
            memory: memory.clone(),
            room: 150,
        }),
    );

    log.record(Path::new("a"), Kind::File, 5, Outcome::Ok, "");
    log.record(Path::new("b"), Kind::File, 5, Outcome::Ok, "");

    let text = memory.text();
    assert!(log.failed());
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.ends_with("}\n"), "{text:?}");
}

#[test]
fn log_inside_a_target_is_refused_however_the_path_is_spelled() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    let targets = std::slice::from_ref(&tree);
    let spellings = [
        tree.join("erase.log"),
        tree.join("sub").join("erase.log"),
        PathBuf::from(format!("{}//./erase.log", tree.display())),
        tree.join("sub").join("..").join("erase.log"),
    ];

    for spelling in &spellings {
        assert!(
            check_location(spelling, targets).is_err(),
            "{}",
            spelling.display()
        );
    }
    assert!(check_location(&dir.path().join("erase.log"), targets).is_ok());
}

#[test]
fn a_directory_with_the_same_name_prefix_is_not_a_target() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let other = dir.path().join("tree2");
    fs::create_dir_all(&tree).unwrap();
    fs::create_dir_all(&other).unwrap();

    assert!(check_location(&other.join("erase.log"), std::slice::from_ref(&tree)).is_ok());
}

#[cfg(unix)]
#[test]
fn a_link_to_the_log_directory_is_not_a_target() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    let link = dir.path().join("link");
    fs::create_dir(&tree).unwrap();
    std::os::unix::fs::symlink(&tree, &link).unwrap();

    assert!(check_location(&tree.join("erase.log"), std::slice::from_ref(&link)).is_ok());
}

#[test]
fn existing_log_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("erase.log");
    fs::write(&log, b"").unwrap();

    assert!(check_path(&log, &[]).is_err());
}
