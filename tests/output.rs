// human output grouping and machine-readable stream boundaries.
mod common;

use common::*;
use std::fs;

/// the temporary directory may sit on copy-on-write or unrecognized storage (apfs on macos, btrfs, overlay
/// in containers); those path-specific warnings are tested elsewhere.
fn without_storage_warnings(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| !line.trim_start().starts_with("warning:"))
        .map(|line| format!("{line}\n"))
        .collect()
}

#[test]
fn dry_run_layout_matches_golden_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let out = run(&["--dry-run", path_str(&file)], "");
    assert_exit(&out, 0);
    let actual = String::from_utf8(out.stdout)
        .unwrap()
        .replace(path_str(&file), "<file>");
    assert_eq!(
        actual,
        "dry run: would erase\n  <file>\n\nsummary\n  1 file, 14 bytes, 1 pass\n"
    );
    assert_eq!(fs::read(file).unwrap(), b"sensitive data");
}

#[test]
fn forced_erase_prints_the_note_and_names_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let out = run(&["-f", path_str(&file)], "");
    assert_exit(&out, 0);
    let action = if cfg!(windows) { "processed" } else { "erased" };
    assert_eq!(
        without_storage_warnings(&out.stderr),
        format!(
            "note: overwriting is best effort: ssds, snapshots, copy-on-write filesystems and backups can keep old data. see --explain.\n{action} {}\n",
            file.display()
        )
    );
}

#[test]
fn quiet_forced_erase_prints_nothing_without_specific_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let out = run(&["-fq", "--verify", path_str(&file)], "");
    assert_exit(&out, 0);
    assert_eq!(without_storage_warnings(&out.stderr), "");
    assert!(!file.exists());
}

#[test]
fn confirmation_matches_golden_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let out = run(&[path_str(&file)], "n\n");
    assert_exit(&out, 3);
    let text = without_storage_warnings(&out.stderr).replace(path_str(&file), "<file>");
    assert_eq!(
        text,
        "targets, erased permanently\n  <file>\n\nsummary\n  1 file, 14 bytes, 1 pass\n\nrisks\n  note: overwriting is best effort: ssds, snapshots, copy-on-write filesystems and backups can keep old data. see --explain.\n\ncontinue? [y/N] aborted, nothing was erased.\n"
    );
    assert!(file.exists());
}

#[test]
fn excluded_entries_alone_do_not_mark_the_summary_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 2);
    let out = run(
        &["--dry-run", "-r", "--exclude", "f0.txt", path_str(&tree)],
        "",
    );
    assert_exit(&out, 0);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1 entry excluded by --exclude"), "{text}");
    assert!(!text.contains("not checked"), "{text}");
}

#[test]
fn result_distinguishes_erased_and_excluded_targets() {
    let dir = tempfile::tempdir().unwrap();
    let erased = write_file(dir.path(), "erase");
    let tree = make_tree(dir.path(), "tree", 1);
    let out = run(
        &[
            "-rf",
            "--exclude",
            "f0.txt",
            path_str(&erased),
            path_str(&tree),
        ],
        "",
    );
    assert_exit(&out, 0);
    let action = if cfg!(windows) { "processed" } else { "erased" };
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains(&format!("{action} 1 target; 1 kept because of --exclude"))
    );
    assert!(!erased.exists());
    assert!(tree.join("f0.txt").exists());
}

#[cfg(unix)]
#[test]
fn incomplete_tree_does_not_get_a_success_footer() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1);
    nix::unistd::mkfifo(&tree.join("pipe"), nix::sys::stat::Mode::S_IRUSR).unwrap();
    let out = run(&["-rf", path_str(&tree)], "");
    assert_exit(&out, 4);
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("incomplete: 0 of 1 target erased, see the errors above"),
        "{text}"
    );
    assert!(
        !text.contains(&format!("erased {}", tree.display())),
        "{text}"
    );
    assert!(tree.join("pipe").exists());
}

#[test]
fn json_and_log_have_no_headings_or_ansi() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let dry = run(&["--dry-run", "--json", path_str(&file)], "");
    assert_exit(&dry, 0);
    let text = String::from_utf8_lossy(&dry.stdout);
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with('{'));
    assert!(!text.contains('\x1b'));
    let log = dir.path().join("erase.log");
    let out = run(&["-f", "--log", path_str(&log), path_str(&file)], "");
    assert_exit(&out, 0);
    let text = fs::read_to_string(log).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with('{'));
    assert!(!text.contains('\x1b'));
}
