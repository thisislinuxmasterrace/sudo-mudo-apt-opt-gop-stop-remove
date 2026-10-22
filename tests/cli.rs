use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_sudo-mudo-apt-opt-gop-stop-remove");

/// runs the binary with `stdin` as its input (empty means EOF).
fn run(args: &[&str], stdin: &str) -> Output {
    run_in(None, args, stdin)
}

/// like `run`, but from `cwd` when given, so relative paths resolve there.
fn run_in(cwd: Option<&Path>, args: &[&str], stdin: &str) -> Output {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().expect("failed to start binary");
    // a binary that rejects its arguments may exit before it reads stdin; the write then fails with a broken pipe.
    match child.stdin.take().unwrap().write_all(stdin.as_bytes()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(err) => panic!("failed to write stdin: {err}"),
    }
    child.wait_with_output().expect("failed to wait for binary")
}

fn write_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"sensitive data").unwrap();
    path
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn assert_exit(out: &Output, expected: i32) {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn dry_run_lists_files_and_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--dry-run", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would erase"));
    assert!(file.exists());
}

#[test]
fn force_erases_without_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[test]
fn accepted_prompt_erases_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "y\n");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[test]
fn declined_prompt_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "n\n");

    assert_exit(&out, 3);
    assert!(file.exists());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("stdin is not a terminal"));
}

#[test]
fn empty_answer_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "");

    assert_exit(&out, 3);
    assert!(file.exists());
}

#[test]
fn eof_without_terminal_points_to_force() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file)], "");

    assert_exit(&out, 3);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("stdin is not a terminal: use -f to erase without a prompt")
    );
    assert!(file.exists());
}

#[test]
fn one_invalid_path_aborts_whole_run() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write_file(dir.path(), "a.txt");
    let missing = dir.path().join("missing.txt");

    let out = run(&["-f", path_str(&valid), path_str(&missing)], "");

    assert_exit(&out, 1);
    assert!(valid.exists());
}

#[test]
fn missing_path_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.txt");

    let out = run(&["-f", path_str(&missing)], "");

    assert_exit(&out, 1);
}

#[test]
fn directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["-f", path_str(dir.path())], "");

    assert_exit(&out, 1);
    assert!(dir.path().exists());
}

#[test]
fn recursive_erases_directory_tree() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("nested")).unwrap();
    write_file(&tree, "a.txt");
    write_file(&tree.join("nested"), "b.txt");

    let out = run(&["-r", "-f", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn exclude_keeps_matching_entries_and_their_directory() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("skip")).unwrap();
    let log = write_file(&tree, "a.log");
    let skipped = write_file(&tree.join("skip"), "b.txt");
    let erased = write_file(&tree, "c.txt");
    let args = ["-r", "--exclude", "*.log", "--exclude", "skip"];

    let mut dry_args = args.to_vec();
    dry_args.extend(["--dry-run", path_str(&tree)]);
    let dry = run(&dry_args, "");
    assert_exit(&dry, 0);
    let dry_stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(dry_stdout.contains("excluded by --exclude: 2"));
    assert!(dry_stdout.contains("directories kept because of --exclude: 1"));
    assert!(erased.exists());

    let mut erase_args = args.to_vec();
    erase_args.extend(["-f", path_str(&tree)]);
    let out = run(&erase_args, "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 entries excluded by --exclude"));
    assert!(log.exists());
    assert!(skipped.exists());
    assert!(!erased.exists());
    assert!(tree.exists());
}

#[test]
fn exclude_trailing_slash_means_the_name_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir_all(tree.join("build")).unwrap();
    fs::create_dir_all(tree.join("sub/build")).unwrap();
    let top = write_file(&tree.join("build"), "top.txt");
    let nested = write_file(&tree.join("sub/build"), "nested.txt");
    let erased = write_file(&tree, "c.txt");

    let out = run(&["-r", "-f", "--exclude", "build/", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(top.exists());
    assert!(nested.exists());
    assert!(!erased.exists());
    assert!(fs::symlink_metadata(tree.join("sub")).is_ok());
}

#[test]
fn exclude_refuses_an_absolute_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let file = write_file(&tree, "a.txt");

    let out = run(
        &["-r", "-f", "--exclude", "/tree/a.txt", path_str(&tree)],
        "",
    );

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn exclude_never_applies_to_the_argument() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.txt");

    let out = run(&["-r", "-f", "--exclude", "tree", path_str(&tree)], "");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn exclude_without_recursive_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", "--exclude", "*.log", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn large_recursive_erase_needs_the_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    for index in 0..1001 {
        write_file(&tree, &format!("f{index}.txt"));
    }

    let answered_yes = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&answered_yes, 3);
    let stderr = String::from_utf8_lossy(&answered_yes.stderr);
    assert!(stderr.contains("this is a large erase (1002 objects"));
    assert!(stderr.contains("type the name of the directory (tree)"));
    assert!(stderr.contains("aborted, nothing was erased."));
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1001);

    let answered_name = run(&["-r", path_str(&tree)], "tree\n");

    assert_exit(&answered_name, 0);
    assert!(!tree.exists());
}

/// a tree of `files` small files plus the directory itself: `files + 1` objects.
fn make_tree(root: &Path, name: &str, files: usize) -> PathBuf {
    let tree = root.join(name);
    fs::create_dir(&tree).unwrap();
    for index in 0..files {
        write_file(&tree, &format!("f{index}.txt"));
    }
    tree
}

#[test]
fn one_thousand_objects_take_yes() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 999);

    let out = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[test]
fn one_thousand_one_objects_need_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let tree = make_tree(dir.path(), "tree", 1000);

    let answered_yes = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&answered_yes, 3);
    assert!(String::from_utf8_lossy(&answered_yes.stderr).contains("(1001 objects"));
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 1000);

    let answered_name = run(&["-r", path_str(&tree)], "tree\n");

    assert_exit(&answered_name, 0);
    assert!(!tree.exists());
}

#[test]
fn mixed_run_asks_continue_after_the_large_name() {
    let dir = tempfile::tempdir().unwrap();
    let big = make_tree(dir.path(), "big", 1001);
    let small = make_tree(dir.path(), "small", 2);

    let without_continue = run(&["-r", path_str(&big), path_str(&small)], "big\n");

    assert_exit(&without_continue, 3);
    assert!(String::from_utf8_lossy(&without_continue.stderr).contains("continue?"));
    assert_eq!(fs::read_dir(&big).unwrap().count(), 1001);
    assert_eq!(fs::read_dir(&small).unwrap().count(), 2);

    let with_continue = run(&["-r", path_str(&big), path_str(&small)], "big\ny\n");

    assert_exit(&with_continue, 0);
    assert!(!big.exists());
    assert!(!small.exists());
}

#[test]
fn total_over_the_limit_asks_the_name_of_the_largest_path() {
    let dir = tempfile::tempdir().unwrap();
    // each path is under the limit (602 and 501 objects), the run is over it (1103).
    let big = make_tree(dir.path(), "big", 601);
    let small = make_tree(dir.path(), "small", 500);
    let args = ["-r", path_str(&big), path_str(&small)];

    let wrong_name = run(&args, "small\ny\n");

    assert_exit(&wrong_name, 3);
    assert_eq!(fs::read_dir(&big).unwrap().count(), 601);
    assert_eq!(fs::read_dir(&small).unwrap().count(), 500);

    let right_name = run(&args, "big\ny\n");

    assert_exit(&right_name, 0);
    assert!(!big.exists());
    assert!(!small.exists());
}

#[test]
fn small_recursive_erase_still_takes_yes() {
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    write_file(&tree, "a.txt");

    let out = run(&["-r", path_str(&tree)], "y\n");

    assert_exit(&out, 0);
    assert!(!tree.exists());
}

#[cfg(unix)]
#[test]
fn symlink_argument_removes_only_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = write_file(dir.path(), "target.txt");
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let out = run(&["-f", path_str(&link)], "");

    assert_exit(&out, 0);
    assert!(!link.exists());
    assert!(target.exists());
}

#[test]
fn read_only_file_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&file, perms).unwrap();

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());
}

#[cfg(not(windows))]
#[test]
fn windows_only_flags_are_rejected_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", "--kill-holders", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[cfg(unix)]
#[test]
fn symlink_with_trailing_slash_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let kept = write_file(&target, "keep.txt");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let with_slash = format!("{}/", path_str(&link));

    let out = run(&["-r", "-f", with_slash.as_str()], "");

    assert_exit(&out, 0);
    assert!(kept.exists());
    assert!(fs::symlink_metadata(&link).is_err());
}

#[test]
fn repeated_path_is_erased_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let dry = run(&["--dry-run", path_str(&file), path_str(&file)], "");
    assert_exit(&dry, 0);
    assert_eq!(
        String::from_utf8_lossy(&dry.stdout)
            .matches("would erase")
            .count(),
        1
    );

    let out = run(&["-f", path_str(&file), path_str(&file)], "");

    assert_exit(&out, 0);
    assert!(!file.exists());

    let other = write_file(dir.path(), "b.txt");
    let out = run_in(Some(dir.path()), &["-f", "./b.txt", "b.txt"], "");

    assert_exit(&out, 0);
    assert!(!other.exists());
}

#[test]
fn empty_path_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-f", path_str(&file), ""], "");
    assert_exit(&out, 2);
    assert!(file.exists());

    let wipe = run(&["--wipe-free", ""], "");
    assert_exit(&wipe, 2);
}

#[test]
fn wipe_free_rejects_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--wipe-free", path_str(&file), "-f"], "");

    assert_exit(&out, 1);
    assert!(file.exists());
}

#[test]
fn wipe_free_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(&["--wipe-free", path_str(dir.path()), "--dry-run"], "");

    assert_exit(&out, 0);
    assert!(String::from_utf8_lossy(&out.stdout).contains("would fill"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn wipe_free_declined_exits_with_three() {
    let dir = tempfile::tempdir().unwrap();

    let declined = run(&["--wipe-free", path_str(dir.path())], "n\n");
    assert_exit(&declined, 3);

    let eof = run(&["--wipe-free", path_str(dir.path())], "");
    assert_exit(&eof, 3);

    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn zero_passes_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-n", "0", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn no_paths_is_an_argument_error() {
    let out = run(&[], "");

    assert_exit(&out, 2);
}

#[test]
fn paths_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&[path_str(&file), "--wipe-free", path_str(dir.path())], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn unknown_flag_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--no-such-flag", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn help_and_version_exit_with_zero() {
    assert_exit(&run(&["--help"], ""), 0);
    assert_exit(&run(&["--version"], ""), 0);
}

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

/// runs the binary with a file size limit of one block, so a log that grows past it cannot be written.
/// SIGXFSZ is ignored first, so the failed write is an error and does not end the process.
#[cfg(unix)]
fn run_with_one_block_file_limit(args: &[&str]) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg("trap '' XFSZ; ulimit -f 1 && exec \"$0\" \"$@\"")
        .arg(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to start sh")
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

#[test]
fn erase_prints_the_note_once_and_dry_run_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let note = "note: overwriting does not guarantee erasure";

    let dry = run(&["--dry-run", path_str(&file)], "");
    assert!(!String::from_utf8_lossy(&dry.stderr).contains(note));

    let out = run(&["-f", path_str(&file)], "");
    assert_exit(&out, 0);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).matches(note).count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn protected_path_needs_the_override_flag() {
    let refused = run(&["--dry-run", "/usr/bin/env"], "");
    assert_exit(&refused, 1);
    assert!(String::from_utf8_lossy(&refused.stderr).contains("protected system path"));

    let allowed = run(
        &["--dry-run", "--i-know-what-i-am-doing", "/usr/bin/env"],
        "",
    );
    assert_exit(&allowed, 0);
    assert!(
        String::from_utf8_lossy(&allowed.stderr).contains("--i-know-what-i-am-doing was given")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn mount_point_target_is_refused_before_anything_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let dry = run(
        &["--dry-run", "-r", "--i-know-what-i-am-doing", "/proc"],
        "",
    );
    assert_exit(&dry, 1);
    assert!(
        String::from_utf8_lossy(&dry.stderr).contains("is a mount point or on another filesystem")
    );
    assert!(!String::from_utf8_lossy(&dry.stdout).contains("would erase"));

    let out = run(
        &[
            "-f",
            "-r",
            "--i-know-what-i-am-doing",
            path_str(&file),
            "/proc",
        ],
        "",
    );
    assert_exit(&out, 1);
    assert!(file.exists());

    let unflagged = run(&["-f", "-r", path_str(&file), "/proc"], "");
    assert_exit(&unflagged, 1);
    assert!(file.exists());
}

/// a file that cannot be unlinked (its directory is not writable) keeps its contents and name,
/// and the run ends with exit code 4. root ignores directory permissions: the test is skipped
/// locally in that case, and fails under CI (where the variable `CI` is set) so it cannot pass unnoticed.
#[cfg(unix)]
#[test]
fn file_in_unwritable_directory_exits_with_four() {
    use std::os::unix::fs::PermissionsExt;

    // restores the mode on drop, so a panic still lets `TempDir` remove the directory.
    struct RestoreMode(PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    let file = write_file(&locked, "a.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let _restore = RestoreMode(locked.clone());

    if fs::write(locked.join("probe"), b"").is_ok() {
        if std::env::var_os("CI").is_some() {
            panic!(
                "directory permissions are not enforced here (running as root?): exit 4 was not tested"
            );
        }
        eprintln!("skipped: directory permissions are not enforced (running as root?)");
        return;
    }

    let out = run(&["-f", path_str(&file)], "");

    assert_exit(&out, 4);
    assert_eq!(fs::read(&file).unwrap(), b"sensitive data");
}

#[cfg(unix)]
#[test]
fn hard_linked_file_is_refused_only_under_root() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");
    let other = dir.path().join("other.txt");
    fs::hard_link(&file, &other).unwrap();

    let out = run(&["-f", path_str(&file)], "");

    if nix::unistd::geteuid().is_root() {
        assert_exit(&out, 4);
        assert!(String::from_utf8_lossy(&out.stderr).contains("hard-linked file under root"));
        assert!(file.exists());

        let allowed = run(&["-f", "--i-know-what-i-am-doing", path_str(&file)], "");
        assert_exit(&allowed, 0);
        assert!(!file.exists());
    } else {
        assert_exit(&out, 0);
        assert!(!file.exists());
    }
    assert!(other.exists());
}

/// signal tests: the handlers are installed once the erase has started, so the tests wait for the
/// `erasing` line before they send anything.
#[cfg(unix)]
mod interrupt {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Read};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::*;

    // the erase is still running when the signal arrives: the signal is sent right after the `erasing`
    // line, which is printed before the first pass starts, and a pass over 128 MiB with a sync takes far longer
    // than delivering a signal. the interrupt is checked before each object and in every 1 MiB write chunk.
    const BIG_FILE: u64 = 128 * 1024 * 1024;

    struct Run {
        code: Option<i32>,
        stderr: String,
        entries: Vec<String>,
        len: Option<u64>,
        path: String,
    }

    /// erases a large file with `-f -v -n 3`, sends `signals` one after the other, and returns
    /// what was left behind.
    fn erase_and_interrupt(signals: &[Signal]) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.bin");
        File::create(&file).unwrap().set_len(BIG_FILE).unwrap();

        let mut child = Command::new(BIN)
            .args(["-f", "-v", "-n", "3", path_str(&file)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start binary");
        let mut reader = BufReader::new(child.stderr.take().unwrap());
        let mut stderr = String::new();
        loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "erase did not start"
            );
            stderr.push_str(&line);
            if line.starts_with("erasing ") {
                break;
            }
        }

        let pid = Pid::from_raw(child.id() as i32);
        for signal in signals {
            kill(pid, *signal).unwrap();
        }

        reader.read_to_string(&mut stderr).unwrap();
        let status = child.wait().unwrap();

        let mut entries: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        let len = entries
            .first()
            .and_then(|name| fs::metadata(dir.path().join(name)).ok())
            .map(|meta| meta.len());
        Run {
            code: status.code(),
            stderr,
            entries,
            len,
            path: path_str(&file).to_owned(),
        }
    }

    #[test]
    fn sigint_keeps_partly_erased_file_under_its_name() {
        let run = erase_and_interrupt(&[Signal::SIGINT]);

        assert_eq!(run.code, Some(4), "stderr:\n{}", run.stderr);
        assert!(run.stderr.contains(&format!("interrupted: {}: ", run.path)));
        assert!(run.stderr.contains("interrupted by signal"));
        assert_eq!(run.entries, ["big.bin"]);
        assert_eq!(run.len, Some(BIG_FILE));
    }

    #[test]
    fn sigterm_keeps_partly_erased_file_under_its_name() {
        let run = erase_and_interrupt(&[Signal::SIGTERM]);

        assert_eq!(run.code, Some(4), "stderr:\n{}", run.stderr);
        assert_eq!(run.entries, ["big.bin"]);
        assert_eq!(run.len, Some(BIG_FILE));
    }

    #[test]
    fn second_interrupt_stops_the_erase_without_success() {
        // the second signal usually arrives before the write loop has seen the first one, and the process
        // exits with 130. under load the first signal is handled alone and the run ends with 4. both are
        // interrupts, so the test accepts either; the file is not erased in both cases.
        let run = erase_and_interrupt(&[Signal::SIGINT, Signal::SIGTERM]);

        assert!(matches!(run.code, Some(4 | 130)), "stderr:\n{}", run.stderr);
        assert_eq!(run.entries.len(), 1);
    }
}

fn write_bytes(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn files_from_reads_one_path_per_line_and_adds_the_positional_ones() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let spaced = write_file(dir.path(), "a b.txt");
    let plain = write_file(dir.path(), "c.txt");
    let extra = write_file(dir.path(), "d.txt");
    let list = write_bytes(
        lists.path(),
        "list.txt",
        format!("{}\n\n{}\n", path_str(&spaced), path_str(&plain)).as_bytes(),
    );

    let out = run(
        &["-f", "--files-from", path_str(&list), path_str(&extra)],
        "",
    );

    assert_exit(&out, 0);
    assert!(!spaced.exists());
    assert!(!plain.exists());
    assert!(!extra.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("note: 2 paths read from"));
}

#[test]
fn files_from_dash_reads_the_list_from_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(
        &["-f", "--files-from", "-"],
        &format!("{}\n", path_str(&file)),
    );

    assert_exit(&out, 0);
    assert!(!file.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("note: 1 paths read from stdin"));
}

#[test]
fn null_separated_list_erases_each_path() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let spaced = write_file(dir.path(), "a b.txt");
    let plain = write_file(dir.path(), "c.txt");
    let list = write_bytes(
        lists.path(),
        "list.bin",
        format!("{}\0{}\0", path_str(&spaced), path_str(&plain)).as_bytes(),
    );

    let out = run(&["-f", "-0", "--files-from", path_str(&list)], "");

    assert_exit(&out, 0);
    assert!(!spaced.exists());
    assert!(!plain.exists());
}

#[test]
fn empty_files_from_erases_nothing_and_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let keep = write_file(dir.path(), "keep.txt");
    let blank = write_bytes(lists.path(), "blank.txt", b"\n\n");
    let empty = write_bytes(lists.path(), "empty.txt", b"");

    for list in [&blank, &empty] {
        let out = run(&["-f", "--files-from", path_str(list)], "");

        assert_exit(&out, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("no paths were given"));
        assert!(keep.exists());
    }
}

#[test]
fn unreadable_files_from_is_an_error_before_anything_is_erased() {
    let dir = tempfile::tempdir().unwrap();
    let keep = write_file(dir.path(), "keep.txt");
    let missing = dir.path().join("no-such-list.txt");

    let out = run(
        &["-f", "--files-from", path_str(&missing), path_str(&keep)],
        "",
    );

    assert_exit(&out, 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no-such-list.txt"));
    assert!(keep.exists());
}

#[test]
fn null_without_files_from_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["-0", "-f", path_str(&file)], "");

    assert_exit(&out, 2);
    assert!(file.exists());
}

#[test]
fn files_from_with_wipe_free_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(
        &["--files-from", "-", "--wipe-free", path_str(dir.path())],
        "",
    );

    assert_exit(&out, 2);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

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

#[cfg(unix)]
#[test]
fn null_separated_list_keeps_a_newline_inside_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let lists = tempfile::tempdir().unwrap();
    let odd = write_file(dir.path(), "x\ny.txt");
    let list = write_bytes(
        lists.path(),
        "list.bin",
        format!("{}\0", path_str(&odd)).as_bytes(),
    );

    let out = run(&["-f", "-0", "--files-from", path_str(&list)], "");

    assert_exit(&out, 0);
    assert!(!odd.exists());
}

#[test]
fn files_from_dash_without_force_is_an_argument_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "a.txt");

    let out = run(&["--files-from", "-"], &format!("{}\n", path_str(&file)));

    assert_exit(&out, 2);
    assert!(file.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs -f"));
}
