use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// checks that a path given on the command line may be erased.
///
/// symbolic links are accepted because only the link itself is removed.
/// directories require `recursive`. protected system locations are always refused.
pub fn validate_target(path: &Path, recursive: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let file_type = meta.file_type();

    if path.file_name().is_none() {
        return Err(format!(
            "{}: refusing to erase '.', '..' or a filesystem root",
            path.display()
        ));
    }
    if file_type.is_dir() && !recursive {
        return Err(format!(
            "{}: is a directory (use -r to erase directories)",
            path.display()
        ));
    }
    if !file_type.is_dir() && !file_type.is_file() && !file_type.is_symlink() {
        return Err(format!(
            "{}: not a regular file, directory or symbolic link",
            path.display()
        ));
    }

    let location = location_of(path, file_type.is_symlink())
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if is_protected(&location) {
        return Err(format!(
            "{}: refusing to erase a protected system path",
            path.display()
        ));
    }
    Ok(())
}

/// resolved location of the entry itself. for a symlink this is the link's own
/// location, not its target's, because the link is what gets removed.
fn location_of(path: &Path, is_symlink: bool) -> io::Result<PathBuf> {
    if is_symlink && let Some(name) = path.file_name() {
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        return fs::canonicalize(parent).map(|dir| dir.join(name));
    }
    fs::canonicalize(path)
}

/// returns true for filesystem roots and for anything under a protected system location.
fn is_protected(location: &Path) -> bool {
    let path = strip_verbatim_prefix(location);
    // filesystem roots ("/", "C:\") have no parent.
    if path.parent().is_none() {
        return true;
    }
    protected_roots().iter().any(|root| path.starts_with(root))
}

/// canonical forms of protected locations, comparable with `is_protected` input.
/// resolves symlinks (`/etc` is `/private/etc` on macos) and fixes casing on windows.
fn protected_roots() -> Vec<PathBuf> {
    platform_protected_roots()
        .into_iter()
        .map(|root| {
            let canonical = fs::canonicalize(&root).unwrap_or(root);
            strip_verbatim_prefix(&canonical)
        })
        .collect()
}

#[cfg(unix)]
fn platform_protected_roots() -> Vec<PathBuf> {
    [
        "/bin", "/sbin", "/usr", "/etc", "/boot", "/lib", "/lib64", "/dev", "/proc", "/sys",
        "/System", "/Library",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

#[cfg(windows)]
fn platform_protected_roots() -> Vec<PathBuf> {
    [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
    ]
    .iter()
    .filter_map(|name| std::env::var_os(name))
    .map(PathBuf::from)
    .collect()
}

/// `fs::canonicalize` returns `\\?\C:\...` on windows; strip it so paths compare with `C:\...`.
#[cfg(windows)]
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(not(windows))]
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, b"data").unwrap();
        assert!(validate_target(&path, false).is_ok());
    }

    #[test]
    fn rejects_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_target(&dir.path().join("missing"), false).is_err());
    }

    #[test]
    fn directory_requires_recursive() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_target(dir.path(), false).is_err());
        assert!(validate_target(dir.path(), true).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, b"data").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(validate_target(&link, false).is_ok());
        assert!(target.exists());
    }

    #[test]
    fn rejects_dot_and_dot_dot() {
        assert!(validate_target(Path::new("."), true).is_err());
        assert!(validate_target(Path::new(".."), true).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn protects_system_root_on_windows() {
        let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let canonical = fs::canonicalize(&system_root).unwrap();
        assert!(is_protected(&canonical));
    }

    #[cfg(unix)]
    #[test]
    fn protects_roots_and_system_locations() {
        assert!(is_protected(Path::new("/")));
        assert!(is_protected(Path::new("/usr/bin/env")));
        assert!(!is_protected(Path::new("/tmp/some-file")));
    }
}
