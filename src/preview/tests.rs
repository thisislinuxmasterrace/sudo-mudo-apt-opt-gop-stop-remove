use super::*;

fn count(paths: &[PathBuf], recursive: bool, excludes: &[String]) -> Summary {
    total(&summarize(paths, recursive, excludes, false))
}

#[test]
fn empty_summary_lines() {
    assert_eq!(
        Summary::default().lines(),
        ["files: 0, directories: 0, links: 0, size: 0 b"]
    );
}

#[test]
fn summary_lines_include_counters_and_incomplete_coverage() {
    let summary = Summary {
        files: 2,
        dirs: 3,
        links: 4,
        bytes: 1536,
        hard_linked: 5,
        skipped: 6,
        excluded: 7,
        kept_dirs: 8,
        refused: 9,
        unreadable: 10,
        other: 11,
        assessments: vec![EntryAssessment {
            path: PathBuf::from("entry"),
            assessment: Assessment::unknown(),
        }],
        ..Summary::default()
    };

    assert_eq!(
        summary.lines(),
        [
            "files: 2, directories: 3, links: 4, size: 1.5 kib",
            "hard-linked files: 5 (the other names lose their content too)",
            "skipped mount points or other filesystems: 6",
            "excluded by --exclude: 7",
            "directories kept because of --exclude: 8",
            "refused: mount point given as target: 9",
            "unreadable entries: 10",
            "special entries (fifo, sockets, devices, unsupported reparse points), which are refused: 11",
            "assessments: 1 weak (use --explain for risks or --dry-run --json for details)",
            "assessment coverage: incomplete (skipped, excluded, refused, special or unreadable entries are not fully assessed for overwrite)",
        ]
    );
}

#[test]
fn summary_lines_aggregate_assessments() {
    let summary = Summary {
        assessments: (0..23)
            .map(|index| EntryAssessment {
                path: PathBuf::from(format!("entry-{index:02}")),
                assessment: Assessment::unknown(),
            })
            .collect(),
        coverage_limited: true,
        ..Summary::default()
    };
    let mut expected = vec!["files: 0, directories: 0, links: 0, size: 0 b".to_owned()];
    expected.push(
        "assessments: 23 weak (use --explain for risks or --dry-run --json for details)".to_owned(),
    );
    expected.push("assessment coverage: incomplete (skipped, excluded, refused, special or unreadable entries are not fully assessed for overwrite)".to_owned());

    assert_eq!(summary.lines(), expected);
}

#[test]
fn counts_files_links_and_bytes_in_a_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();
    fs::write(tree.join("sub/b.txt"), [0u8; 20]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&tree, tree.join("link")).unwrap();

    let summary = count(std::slice::from_ref(&tree), true, &[]);

    assert_eq!(summary.files, 2);
    assert_eq!(summary.dirs, 2);
    assert_eq!(summary.bytes, 30);
    assert_eq!(summary.unreadable, 0);
    #[cfg(unix)]
    assert_eq!(summary.links, 1);
}

#[test]
fn without_recursive_only_the_top_level_is_counted() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();

    let summary = count(std::slice::from_ref(&tree), false, &[]);

    assert_eq!(summary.dirs, 1);
    assert_eq!(summary.files, 0);
    assert_eq!(summary.assessments.len(), 1);
}

#[test]
fn excluded_entries_are_counted_and_not_walked() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("skip")).unwrap();
    fs::write(tree.join("a.log"), [0u8; 10]).unwrap();
    fs::write(tree.join("skip/b.txt"), [0u8; 20]).unwrap();
    fs::write(tree.join("c.txt"), [0u8; 30]).unwrap();
    let excludes = ["*.log".to_owned(), "skip".to_owned()];

    let summary = count(std::slice::from_ref(&tree), true, &excludes);

    assert_eq!(summary.excluded, 2);
    assert_eq!(summary.kept_dirs, 1);
    assert_eq!(summary.files, 1);
    assert_eq!(summary.dirs, 1);
    assert_eq!(summary.bytes, 30);
    let lines = summary.lines();
    assert!(lines.iter().any(|line| line == "excluded by --exclude: 2"));
    assert!(
        lines
            .iter()
            .any(|line| line == "directories kept because of --exclude: 1")
    );
}

#[test]
fn command_line_argument_is_not_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();

    let summary = count(std::slice::from_ref(&tree), true, &["tree".to_owned()]);

    assert_eq!(summary.excluded, 0);
    assert_eq!(summary.dirs, 1);
    assert_eq!(summary.files, 1);
}

#[test]
fn without_exclusions_no_line_is_printed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, [0u8; 10]).unwrap();

    let summary = count(std::slice::from_ref(&file), false, &[]);

    assert!(
        summary
            .lines()
            .iter()
            .all(|line| !line.contains("excluded"))
    );
}

#[cfg(unix)]
#[test]
fn hard_links_add_their_size_once() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, [0u8; 100]).unwrap();
    fs::hard_link(&a, &b).unwrap();

    let summary = count(&[a, b], false, &[]);

    assert_eq!(summary.hard_linked, 2);
    assert_eq!(summary.bytes, 100);
}

#[cfg(unix)]
#[test]
fn hard_links_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, b"data").unwrap();
    fs::hard_link(&a, &b).unwrap();

    let summary = count(std::slice::from_ref(&a), false, &[]);

    assert_eq!(summary.hard_linked, 1);
}

#[cfg(target_os = "linux")]
#[test]
fn mount_point_given_as_target_is_refused() {
    let summary = count(&[PathBuf::from("/proc")], true, &[]);

    assert_eq!(summary.refused, 1);
    assert_eq!(summary.dirs, 0);
    assert_eq!(summary.files, 0);
    assert!(
        summary
            .lines()
            .iter()
            .any(|line| line.starts_with("refused"))
    );
}

#[test]
fn missing_path_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();

    let summary = count(&[dir.path().join("missing")], false, &[]);

    assert_eq!(summary.unreadable, 1);
}

#[test]
fn json_summary_is_one_object_with_every_count() {
    let total = Summary {
        files: 2,
        dirs: 1,
        bytes: 30,
        ..Summary::default()
    };

    let json = to_json(&[PathBuf::from("a\"b")], 1, &total);

    assert_eq!(
        json,
        r#"{"paths":["a\"b"],"assessments":[],"assessment_coverage":"visited_entries","passes":1,"files":2,"dirs":1,"links":0,"bytes":30,"hard_linked":0,"skipped":0,"refused":0,"unreadable":0,"other":0,"excluded":0,"kept_dirs":0,"objects":3}"#
    );
}

#[cfg(unix)]
#[test]
fn json_summary_keeps_non_utf8_paths_distinct_without_filesystem_access() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let paths = [
        PathBuf::from("plain.txt"),
        PathBuf::from(OsStr::from_bytes(b"a\xffb")),
        PathBuf::from(OsStr::from_bytes(b"a\xfeb")),
    ];

    let json = to_json(&paths, 1, &Summary::default());

    assert!(json.starts_with("{\"paths\":[\"plain.txt\",\"a\u{fffd}b\",\"a\u{fffd}b\"]"));
    assert!(json.contains("\"path_bytes\":[null,\"61ff62\",\"61fe62\"]"));
}

#[test]
fn human_bytes_picks_a_unit() {
    assert_eq!(human_bytes(512), "512 b");
    assert_eq!(human_bytes(1536), "1.5 kib");
    assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 mib");
}
