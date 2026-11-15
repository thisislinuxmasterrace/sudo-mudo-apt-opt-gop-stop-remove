// human output grouping and machine-readable stream boundaries.
mod common;

use common::*;
use std::fs;

#[test]
fn confirmation_groups_targets_summary_risks_and_question() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "data");
    let out = run(&[path_str(&file)], "n\n");
    assert_exit(&out, 3);
    let text = String::from_utf8_lossy(&out.stderr);
    let mut end = 0;
    for heading in ["\ntargets\n", "\nsummary\n", "\nrisks\n", "\nconfirm\n"] {
        let at = text
            .find(heading)
            .unwrap_or_else(|| panic!("missing {heading:?}: {text}"));
        assert!(at >= end, "{text}");
        end = at + heading.len();
    }
    assert!(!text.contains('\x1b'));
    assert!(file.exists());
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
    assert!(String::from_utf8_lossy(&out.stderr).contains(&format!(
        "done: 1 target {action}, 1 target kept by --exclude"
    )));
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
    assert!(text.contains("incomplete: 0 targets erased"), "{text}");
    assert!(!text.contains("done:"), "{text}");
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
