mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::*;

/// the lines of a log file, without the line ends.
fn log_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// a rough check of one json object per line: braces at the ends, and quotes that pair up.
fn is_json_object(line: &str) -> bool {
    if !line.starts_with('{') || !line.ends_with('}') {
        return false;
    }
    let mut quotes = 0;
    let mut escaped = false;
    for ch in line.chars() {
        match (escaped, ch) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, '"') => quotes += 1,
            _ => {}
        }
    }
    quotes % 2 == 0 && !escaped
}

#[test]
fn log_has_one_line_per_object_and_no_contents() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 3);
    let log = dir.path().join("erase.log");

    let out = run(&["-r", "-f", "--log", path_str(&log), path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
    let text = fs::read_to_string(&log).unwrap();
    assert!(!text.contains("sensitive data"));
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines.iter().all(|line| is_json_object(line)), "{text}");
    assert!(lines.iter().all(|line| line.contains("\"result\":\"ok\"")));
    let files: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("\"kind\":\"file\""))
        .collect();
    assert_eq!(files.len(), 3);
    assert!(
        files
            .iter()
            .all(|line| line.contains("\"size\":14,\"passes\":1,"))
    );
    // a directory is logged after the entries inside it.
    assert!(lines.last().unwrap().contains("\"kind\":\"dir\""));
}

#[test]
fn log_records_excluded_entries_as_skipped_and_their_directory_as_kept() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.log");
    write_file(&tree, "c.txt");
    let log = dir.path().join("erase.log");

    let out = run(
        &[
            "-r",
            "-f",
            "--exclude",
            "*.log",
            "--log",
            path_str(&log),
            path_str(&tree),
        ],
        "",
    );

    assert_exit(&out, 0);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("a.log\",\"kind\":\"file\"")
                && line.contains("\"result\":\"skipped\""))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("c.txt\"") && line.contains("\"result\":\"ok\""))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"kind\":\"dir\"") && line.contains("\"result\":\"kept\""))
    );
}

#[cfg(unix)]
#[test]
fn log_is_created_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let log = dir.path().join("erase.log");

    let out = run(&["-f", "--log", path_str(&log), path_str(&file)], "");

    assert_exit(&out, 0);
    assert_eq!(
        fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("note: the log contains file names; it is created with mode 0600")
    );
}

#[cfg(unix)]
#[test]
fn log_escapes_quotes_and_backslashes_in_names() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), r#"a"b\c.txt"#);
    let log = dir.path().join("erase.log");

    let out = run(&["-f", "--log", path_str(&log), path_str(&file)], "");

    assert_exit(&out, 0);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1);
    assert!(is_json_object(&lines[0]), "{}", lines[0]);
    assert!(lines[0].contains(r#"a\"b\\c.txt"#), "{}", lines[0]);
}

#[test]
fn existing_log_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let log = write_file(dir.path(), "erase.log");

    let out = run(&["-f", "--log", path_str(&log), path_str(&file)], "");

    assert_exit(&out, 1);
    assert!(file.exists());
    assert_eq!(fs::read(&log).unwrap(), b"sensitive data");
}

#[test]
fn log_inside_the_erased_tree_is_refused_before_anything_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1);
    let log = tree.join("erase.log");

    let out = run(&["-r", "-f", "--log", path_str(&log), path_str(&tree)], "");

    assert_exit(&out, 1);
    assert!(!log.exists());
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1);
}

#[test]
fn log_inside_the_erased_tree_is_refused_for_other_spellings() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1);
    let spellings = [
        PathBuf::from(format!("{}//./erase.log", path_str(&tree))),
        PathBuf::from(format!("{}/../tree/erase.log", path_str(&tree))),
    ];

    for log in &spellings {
        let out = run(&["-r", "-f", "--log", path_str(log), path_str(&tree)], "");

        assert_exit(&out, 1);
        assert!(!tree.join("erase.log").exists());
    }
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn log_write_failure_stops_the_erase_with_exit_four() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 40);
    let log = dir.path().join("erase.log");

    let out =
        run_with_one_block_file_limit(&["-r", "-f", "--log", path_str(&log), path_str(&tree)]);

    assert_exit(&out, 4);
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not write the log"));
    let left = fs::read_dir(&tree).unwrap().count();
    assert!(left > 0 && left < 40, "{left} entries left");
    // the line that failed is cut off, so the log ends with a complete line or is empty.
    let text = fs::read_to_string(&log).unwrap();
    assert!(text.is_empty() || text.ends_with("}\n"), "{text}");
}

#[test]
fn dry_run_does_not_write_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let log = dir.path().join("erase.log");

    let out = run(&["--dry-run", "--log", path_str(&log), path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!log.exists());
    assert!(file.exists());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("note: --log is not written by --dry-run")
    );
}

#[test]
fn log_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("erase.log");

    let out = run(
        &["--wipe-free", path_str(dir.path()), "--log", path_str(&log)],
        "",
    );

    assert_exit(&out, 2);
    assert!(!log.exists());
}

#[test]
fn dry_run_json_is_one_line_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 2);

    let out = run(&["-r", "--dry-run", "--json", path_str(&tree)], "");

    assert_exit(&out, 0);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert!(is_json_object(lines[0]), "{stdout}");
    assert!(lines[0].starts_with("{\"paths\":[\""));
    assert!(lines[0].contains(",\"files\":2,"), "{stdout}");
    assert!(lines[0].contains(",\"dirs\":1,"), "{stdout}");
    assert!(lines[0].ends_with(",\"objects\":3}"), "{stdout}");
    assert!(tree.exists());
}

#[test]
fn json_without_dry_run_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--json", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn json_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(
        &["--wipe-free", path_str(dir.path()), "--dry-run", "--json"],
        "",
    );

    assert_exit(&out, 2);
}
