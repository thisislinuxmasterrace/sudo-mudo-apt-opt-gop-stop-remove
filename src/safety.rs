use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// checks that a path given on the command line may be erased. returns `Ok(true)` when the path is
/// protected and `allow_protected` lets it through; the caller must warn about that.
///
/// symbolic links are accepted because only the link itself is removed.
/// directories require `recursive`. protected system locations are refused unless `allow_protected`.
pub fn validate_target(
    path: &Path,
    recursive: bool,
    allow_protected: bool,
) -> Result<bool, String> {
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
    if !is_protected(&location) {
        return Ok(false);
    }
    if allow_protected {
        return Ok(true);
    }
    Err(format!(
        "{}: refusing to erase a protected system path (--i-know-what-i-am-doing allows it)",
        path.display()
    ))
}

/// checks that `path` is a directory, and outside protected locations unless `allow_protected`
/// (then `Ok(true)` means the caller must warn). required by `--wipe-free`.
/// a symlink to a directory is accepted: only the filesystem behind it is used, nothing is removed.
pub fn validate_wipe_dir(path: &Path, allow_protected: bool) -> Result<bool, String> {
    let meta = fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if !meta.is_dir() {
        return Err(format!("{}: not a directory", path.display()));
    }
    let location = fs::canonicalize(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if !is_protected(&location) {
        return Ok(false);
    }
    if allow_protected {
        return Ok(true);
    }
    Err(format!(
        "{}: refusing to fill a protected system path (--i-know-what-i-am-doing allows it)",
        path.display()
    ))
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

/// returns true for filesystem roots, for anything under a protected system location, and for
/// the directories listed in `exact_roots` themselves.
fn is_protected(location: &Path) -> bool {
    let path = strip_verbatim_prefix(location);
    // filesystem roots ("/", "C:\") have no parent.
    if path.parent().is_none() {
        return true;
    }
    protected_roots().iter().any(|root| path.starts_with(root))
        || exact_roots().contains(&path)
        || is_protected_by_identity(&path)
}

/// catches a protected directory reached through another name, such as a bind mount of `/usr`
/// at `~/x`: the paths differ, the directory is the same. only the given directory is compared,
/// not the directories below it.
#[cfg(unix)]
fn is_protected_by_identity(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    // symlink_metadata: a link to a protected directory is not the directory and may be removed.
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    protected_roots()
        .iter()
        .chain(exact_roots().iter())
        .any(|root| {
            fs::metadata(root).is_ok_and(|root_meta| {
                root_meta.dev() == meta.dev() && root_meta.ino() == meta.ino()
            })
        })
}

#[cfg(not(unix))]
fn is_protected_by_identity(_path: &Path) -> bool {
    false
}

/// canonical forms of protected locations, comparable with `is_protected` input.
/// resolves symlinks (`/etc` is `/private/etc` on macos) and fixes casing on windows.
fn protected_roots() -> Vec<PathBuf> {
    canonical_forms(platform_protected_roots())
}

/// like `protected_roots`, but only the directory itself is protected: its contents stay erasable.
/// the top-level directories here hold user data, temp files or mounted volumes.
fn exact_roots() -> Vec<PathBuf> {
    canonical_forms(platform_exact_roots())
}

fn canonical_forms(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots
        .into_iter()
        .map(|root| {
            let canonical = fs::canonicalize(&root).unwrap_or(root);
            strip_verbatim_prefix(&canonical)
        })
        .collect()
}

#[cfg(unix)]
fn platform_exact_roots() -> Vec<PathBuf> {
    ["/var", "/opt", "/Volumes", "/private"]
        .iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(windows)]
fn platform_exact_roots() -> Vec<PathBuf> {
    Vec::new()
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
    let mut roots: Vec<PathBuf> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .collect();
    // without SystemRoot the windows directory is still protected through the system drive.
    if std::env::var_os("SystemRoot").is_none()
        && let Some(drive) = std::env::var_os("SystemDrive")
    {
        roots.push(PathBuf::from(drive).join("Windows"));
    }
    roots
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
        assert!(validate_target(&path, false, false).is_ok());
    }

    #[test]
    fn rejects_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_target(&dir.path().join("missing"), false, false).is_err());
    }

    #[test]
    fn directory_requires_recursive() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_target(dir.path(), false, false).is_err());
        assert!(validate_target(dir.path(), true, false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, b"data").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(validate_target(&link, false, false).is_ok());
        assert!(target.exists());
    }

    #[test]
    fn rejects_dot_and_dot_dot() {
        assert!(validate_target(Path::new("."), true, false).is_err());
        assert!(validate_target(Path::new(".."), true, false).is_err());
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
    fn link_to_a_protected_directory_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("usr-link");
        std::os::unix::fs::symlink("/usr", &link).unwrap();

        assert!(validate_target(&link, false, false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn protects_roots_and_system_locations() {
        assert!(is_protected(Path::new("/")));
        assert!(is_protected(Path::new("/usr/bin/env")));
        assert!(!is_protected(Path::new("/tmp/some-file")));
    }

    #[cfg(unix)]
    #[test]
    fn protects_exact_roots_but_not_their_contents() {
        let var = fs::canonicalize("/var").unwrap();
        assert!(is_protected(&var));
        assert!(!is_protected(
            &var.join("log").join("file-that-does-not-exist")
        ));
        assert!(!is_protected(Path::new("/Volumes/usb/file")));
    }

    #[cfg(unix)]
    #[test]
    fn protected_path_needs_the_override_flag() {
        assert!(validate_target(Path::new("/usr"), true, false).is_err());
        assert_eq!(validate_target(Path::new("/usr"), true, true), Ok(true));
        assert!(validate_wipe_dir(Path::new("/usr"), false).is_err());
        assert_eq!(validate_wipe_dir(Path::new("/usr"), true), Ok(true));
    }
}
