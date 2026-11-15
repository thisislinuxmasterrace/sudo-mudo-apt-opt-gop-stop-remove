//! optional git detail for explicitly requested explanations, once per command-line target.

use std::path::Path;
use std::process::Command;

fn query(
    root: &Path,
    relative: &Path,
    args: &[&str],
) -> Option<(std::process::ExitStatus, Vec<u8>)> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
        ])
        .arg("-c")
        .arg(format!("safe.directory={}", root.display()))
        .args(["-c", "log.showSignature=false"])
        .args(args)
        .arg("--")
        .arg(relative)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    crate::advisory::output(&mut command)
}

pub(crate) fn explain(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() && !meta.is_dir() {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let dir = if meta.is_dir() {
        absolute.as_path()
    } else {
        absolute.parent()?
    };
    let root = super::scan(dir).git?;
    let relative = absolute.strip_prefix(&root).ok()?;
    let relative = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    let index = match query(
        &root,
        relative,
        &["ls-files", "--cached", "--error-unmatch", "-z"],
    ) {
        Some((status, _)) if status.success() => "tracked in the index",
        Some((status, _)) if status.code() == Some(1) => "not tracked in the index",
        _ => "index unknown",
    };
    let history = match query(
        &root,
        relative,
        &[
            "log",
            "--all",
            "--format=%H",
            "-1",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
        ],
    ) {
        Some((status, bytes))
            if status.success() && bytes.iter().any(|b| !b.is_ascii_whitespace()) =>
        {
            "reachable history contains this path"
        }
        Some((status, _)) if status.success() => "no reachable history found for this path",
        _ => "history unknown",
    };
    Some(format!(
        "  - {}: git: {index}; {history}. directory results include descendants. renamed paths, reflogs, unreachable objects, clones and remotes are not checked.",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_invalid_git_metadata_stays_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"data").unwrap();
        let text = explain(&path).unwrap();
        assert!(text.contains("index unknown; history unknown"), "{text}");
    }
}
