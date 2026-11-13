// advisor output uses the same facts before and after an erase.
mod common;

use common::*;
use std::fs;

fn assessments(text: &str) -> &str {
    text.split_once("\"assessments\":[").unwrap().1
}

#[test]
fn dry_run_and_completed_log_keep_the_same_file_facts() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let log = dir.path().join("erase.log");
    let dry = run(&["--dry-run", "--json", path_str(&file)], "");
    assert_exit(&dry, 0);
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
    let text = String::from_utf8(dry.stdout).unwrap();
    let entry = assessments(&text).split_once("}],").unwrap().0;
    let fields = entry.split_once(",\"verdict\":").unwrap().1;
    assert!(fields.starts_with("\"weak\""), "{text}");
    let erased = run(&["-f", "--log", path_str(&log), path_str(&file)], "");
    assert_exit(&erased, 0);
    assert!(!file.exists());
    let record = fs::read_to_string(log).unwrap();
    assert!(record.contains("\"result\":\"ok\""), "{record}");
    assert!(record.contains(fields), "dry: {text}\nlog: {record}");
    assert!(String::from_utf8_lossy(&erased.stderr).contains("weak"));
}

#[cfg(unix)]
#[test]
fn dangling_link_is_cosmetic_and_does_not_need_target_facts() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
    let dry = run(&["--dry-run", "--json", path_str(&link)], "");
    assert_exit(&dry, 0);
    let text = String::from_utf8(dry.stdout).unwrap();
    assert!(text.contains("\"verdict\":\"cosmetic\""), "{text}");
    assert!(text.contains("\"filesystem\":null"), "{text}");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    let log = dir.path().join("erase.log");
    let erased = run(&["-f", "--log", path_str(&log), path_str(&link)], "");
    assert_exit(&erased, 0);
    let record = fs::read_to_string(log).unwrap();
    assert!(record.contains("\"verdict\":\"cosmetic\""), "{record}");
}

#[test]
fn excluded_tree_has_explicitly_incomplete_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 2);
    let dry = run(
        &[
            "-r",
            "--dry-run",
            "--json",
            "--exclude",
            "f0.txt",
            path_str(&tree),
        ],
        "",
    );
    assert_exit(&dry, 0);
    let text = String::from_utf8(dry.stdout).unwrap();
    assert!(
        text.contains("\"assessment_coverage\":\"incomplete\""),
        "{text}"
    );
    let entries = assessments(&text);
    assert!(!entries.contains("f0.txt"), "{text}");
    assert!(entries.contains("f1.txt"), "{text}");
    assert!(!text.contains("\"verdict\":\"effective\""));
    assert_eq!(fs::read_dir(tree).unwrap().count(), 2);
}
