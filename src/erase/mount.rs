//! device ids and mount points. a bind mount has the device id of its parent, so the mount table is
//! read as well.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(unix)]
use nix::sys::stat::{FileStat, SFlag};

#[cfg(unix)]
use super::at::file_kind;

/// filesystem id of an entry.
#[cfg(unix)]
pub(crate) fn device(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    meta.dev()
}

// windows mount points and junctions are reparse points, which std reports as symlinks:
// they are removed as links and never descended into.
#[cfg(not(unix))]
pub(crate) fn device(_meta: &fs::Metadata) -> u64 {
    0
}

/// number of hard links to the entry's data. std does not expose it on windows yet, so 1 there.
#[cfg(unix)]
pub(crate) fn link_count(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    meta.nlink()
}

#[cfg(not(unix))]
pub(crate) fn link_count(_meta: &fs::Metadata) -> u64 {
    1
}

/// true when `entry` lives on another filesystem than its parent, or is a mount point itself.
/// symlinks are never followed here, so they are never reported.
pub(crate) fn is_mount_boundary(entry: &Path, parent_dev: u64) -> io::Result<bool> {
    let meta = fs::symlink_metadata(entry)?;
    if meta.file_type().is_symlink() {
        return Ok(false);
    }
    if device(&meta) != parent_dev {
        return Ok(true);
    }
    // bind mounts share the device id of their parent, so the mount table is needed as well.
    Ok(mount_points()?.contains(&fs::canonicalize(entry)?))
}

/// mount points of the running system, read once. on linux they come from
/// `/proc/self/mountinfo`, on macos from `getmntinfo`; elsewhere only the device check applies.
#[cfg(target_os = "linux")]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static MOUNTS: OnceLock<Result<HashSet<PathBuf>, String>> = OnceLock::new();
    MOUNTS
        .get_or_init(|| {
            fs::read_to_string("/proc/self/mountinfo")
                .map(|table| {
                    table
                        .lines()
                        // field 5 is the mount point
                        .filter_map(|line| line.split(' ').nth(4))
                        .map(|escaped| PathBuf::from(unescape_mountinfo(escaped)))
                        .collect()
                })
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| io::Error::other(format!("could not read the mount table: {err}")))
}

#[cfg(target_os = "macos")]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static MOUNTS: OnceLock<Result<HashSet<PathBuf>, String>> = OnceLock::new();
    MOUNTS
        .get_or_init(|| {
            macos_mount_points()
                .map(|points| points.into_iter().collect())
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| io::Error::other(format!("could not read the mount table: {err}")))
}

/// mount points from `getmntinfo(3)`. the array lives in libc's memory and is only read here.
#[cfg(target_os = "macos")]
fn macos_mount_points() -> io::Result<Vec<PathBuf>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    let mut buf: *mut libc::statfs = ptr::null_mut();
    // safety: buf is a valid out pointer. mnt_nowait keeps the call from contacting network
    // filesystems.
    let count = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if count <= 0 || buf.is_null() {
        return Err(io::Error::other("getmntinfo failed"));
    }
    // safety: getmntinfo returned `count` entries at `buf`, valid until the next call; nothing
    // else in this process calls it while this slice is alive.
    let entries = unsafe { std::slice::from_raw_parts(buf, count as usize) };
    Ok(entries
        .iter()
        .map(|entry| {
            // safety: f_mntonname is a NUL-terminated array inside the entry.
            let name = unsafe { CStr::from_ptr(entry.f_mntonname.as_ptr()) };
            PathBuf::from(OsStr::from_bytes(name.to_bytes()))
        })
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static NONE: OnceLock<HashSet<PathBuf>> = OnceLock::new();
    Ok(NONE.get_or_init(HashSet::new))
}

/// mountinfo escapes space, tab, newline and backslash as octal sequences.
#[cfg(target_os = "linux")]
pub(crate) fn unescape_mountinfo(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// same test as `is_mount_boundary`, for an entry already stat'ed through its directory.
#[cfg(unix)]
pub(super) fn is_boundary(st: &FileStat, dir_st: &FileStat, real: &Path) -> io::Result<bool> {
    if file_kind(st) == SFlag::S_IFLNK {
        return Ok(false);
    }
    Ok(st.st_dev != dir_st.st_dev || mount_points()?.contains(real))
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_table_lists_the_root_filesystem() {
        assert!(mount_points().unwrap().contains(Path::new("/")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(unescape_mountinfo(r"/mnt/a\040b\134c"), "/mnt/a b\\c");
    }
}
