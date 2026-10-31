//! paths given to the tool: the list read with `--files-from` and the normal form of every path.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// how the source of `--files-from` is named in messages: `-` is stdin.
pub fn list_label(source: &Path) -> String {
    if source.as_os_str() == "-" {
        "stdin".to_owned()
    } else {
        source.display().to_string()
    }
}

/// reads the paths of `--files-from`: one per line, or one per nul byte with `--null`. entries are
/// taken as they are, so spaces stay in the path. a line ends at `\n`, and one `\r` before it is dropped
/// (crlf lists); with `--null` nothing is dropped. empty entries are skipped.
pub fn read_paths(source: &Path, null: bool) -> Result<Vec<PathBuf>, String> {
    let label = list_label(source);
    let bytes = if source.as_os_str() == "-" {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes).map(|_| bytes)
    } else {
        fs::read(source)
    }
    .map_err(|err| format!("{label}: could not read the list of paths: {err}"))?;
    let separator = if null { b'\0' } else { b'\n' };
    bytes
        .split(|&byte| byte == separator)
        .map(|entry| match entry.strip_suffix(b"\r") {
            Some(line) if !null => line,
            _ => entry,
        })
        .filter(|entry| !entry.is_empty())
        .map(|entry| path_from_bytes(entry).map_err(|err| format!("{label}: {err}")))
        .collect()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    std::str::from_utf8(bytes).map(PathBuf::from).map_err(|_| {
        format!(
            "a path is not valid utf-8: {}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// drops `.` components, so `./a` and `a` are the same path. a path made only of `.` stays `.`,
/// while the empty path stays empty: it must be refused, not turned into the current directory.
pub fn normalize(path: &Path) -> PathBuf {
    let normalized: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() && !path.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_dot_components() {
        assert_eq!(
            normalize(Path::new("./a.txt")),
            normalize(Path::new("a.txt"))
        );
        assert_eq!(
            normalize(Path::new("dir/./a.txt")),
            PathBuf::from("dir/a.txt")
        );
        assert_eq!(normalize(Path::new("./")), PathBuf::from("."));
        assert_eq!(normalize(Path::new(".")), PathBuf::from("."));
    }

    #[test]
    fn normalize_keeps_empty_path_empty() {
        assert_eq!(normalize(Path::new("")), PathBuf::new());
    }
}
