mod common;

use common::*;

/// smoke test only: this erase is shorter than the progress interval, so it would print nothing even on a
/// terminal. it does not prove that progress is absent; the rule itself is the `wanted` unit test in progress.rs.
#[test]
fn piped_stderr_erase_has_no_progress_line_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_bytes(dir.path(), "big.bin", &vec![0x5A; 8 * 1024 * 1024]);

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains('\r'));
    assert!(!stderr.contains("progress:"));
    assert!(!file.exists());
}

/// without a terminal nothing is drawn anyway, so this only checks that `-q` adds no output: every line
/// on stderr is a note or a warning.
#[test]
fn quiet_erase_prints_only_notes_and_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_bytes(dir.path(), "big.bin", &vec![0x5A; 8 * 1024 * 1024]);

    let out = run(&["-q", "-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        assert!(
            line.contains("note:") || line.contains("warning:"),
            "unexpected line: {line}"
        );
    }
}
