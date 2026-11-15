//! optional git detail for explicitly requested explanations, once per command-line target.

use std::path::{Path, PathBuf};
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

/// what git says about one command-line target.
pub(crate) struct Detail {
    /// the work tree root, absolute.
    pub root: PathBuf,
    /// whether the index tracks the path; `None` when git could not tell.
    pub tracked: Option<bool>,
    /// whether reachable history contains the path; `None` when git could not tell. directories
    /// include their descendants. renames, reflogs and unreachable objects are not looked at.
    pub in_history: Option<bool>,
}

pub(crate) fn inspect(path: &Path) -> Option<Detail> {
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
    let tracked = match query(
        &root,
        relative,
        &["ls-files", "--cached", "--error-unmatch", "-z"],
    ) {
        Some((status, _)) if status.success() => Some(true),
        Some((status, _)) if status.code() == Some(1) => Some(false),
        _ => None,
    };
    let in_history = match query(
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
            Some(true)
        }
        Some((status, _)) if status.success() => Some(false),
        _ => None,
    };
    Some(Detail {
        root,
        tracked,
        in_history,
    })
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
        let detail = inspect(&path).unwrap();
        assert_eq!(detail.tracked, None);
        assert_eq!(detail.in_history, None);
    }
}
