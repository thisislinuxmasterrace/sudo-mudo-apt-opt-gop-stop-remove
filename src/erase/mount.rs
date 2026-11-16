//! device ids and mount points. a bind mount has the device id of its parent, so the mount table is
//! read as well.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
pub(crate) fn is_mount_boundary_in(
    entry: &Path,
    parent_dev: u64,
    mounts: &MountTable,
) -> io::Result<bool> {
    let meta = fs::symlink_metadata(entry)?;
    if meta.file_type().is_symlink() || crate::entry::is_unsupported_reparse(&meta) {
        return Ok(false);
    }
    if device(&meta) != parent_dev {
        return Ok(true);
    }
    // the table is always empty on windows: do not resolve the path for nothing.
    if cfg!(windows) {
        return Ok(false);
    }
    // bind mounts share the device id of their parent, so the mount table is needed as well.
    mounts.contains(&fs::canonicalize(entry)?)
}

/// `is_mount_boundary_in` with a table read for this entry. windows has no table, so it is empty.
#[cfg(windows)]
pub(crate) fn is_mount_boundary(entry: &Path, parent_dev: u64) -> io::Result<bool> {
    is_mount_boundary_in(entry, parent_dev, &MountTable::read())
}

/// the mount points of the running system. it is read for each top-level target, so a mount made
/// while an earlier target was erased is seen. a read error is kept and returned by every lookup:
/// links need no lookup, every other entry is refused (fail closed).
pub(crate) struct MountTable(Result<HashSet<PathBuf>, String>);

impl MountTable {
    pub(crate) fn read() -> Self {
        Self(
            mount_points()
                .map(|points| points.into_iter().collect())
                .map_err(|err| err.to_string()),
        )
    }

    fn contains(&self, path: &Path) -> io::Result<bool> {
        self.0
            .as_ref()
            .map(|points| points.contains(path))
            .map_err(|err| io::Error::other(format!("could not read the mount table: {err}")))
    }
}

/// mount points from `/proc/self/mountinfo`, read as bytes: a mount point that is not utf-8 is kept
/// as it is and does not make the whole table unreadable.
#[cfg(target_os = "linux")]
fn mount_points() -> io::Result<Vec<PathBuf>> {
    use crate::storage::mountinfo;

    mountinfo::read_table().map(|table| mountinfo::mount_points(&table))
}

/// decodes the `\ooo` octal escapes of mountinfo and `/proc/swaps`, as bytes. the kernel escapes every
/// backslash too, so a backslash followed by three octal digits is always an escape. only the shared
/// parser in `storage::mountinfo` calls this.
#[cfg(target_os = "linux")]
pub(crate) fn unescape_mountinfo(escaped: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(escaped.len());
    let mut index = 0;
    while index < escaped.len() {
        if let [b'\\', a @ b'0'..=b'3', b @ b'0'..=b'7', c @ b'0'..=b'7', ..] = escaped[index..] {
            out.push((a - b'0') * 64 + (b - b'0') * 8 + (c - b'0'));
            index += 4;
        } else {
            out.push(escaped[index]);
            index += 1;
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn mount_points() -> io::Result<Vec<PathBuf>> {
    macos_mount_points()
}

/// mount points from `getmntinfo(3)`. the array lives in libc's memory and is only read here.
#[cfg(target_os = "macos")]
fn macos_mount_points() -> io::Result<Vec<PathBuf>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;
    use std::sync::Mutex;

    // getmntinfo reuses libc-owned storage; keep it valid until the paths have been copied.
    static MOUNTS: Mutex<()> = Mutex::new(());
    let _guard = MOUNTS
        .lock()
        .map_err(|_| io::Error::other("mount table lock is poisoned"))?;

    let mut buf: *mut libc::statfs = ptr::null_mut();
    // safety: buf is a valid out pointer. mnt_nowait keeps the call from contacting network
    // filesystems.
    let count = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if count <= 0 || buf.is_null() {
        return Err(io::Error::other("getmntinfo failed"));
    }
    // safety: getmntinfo returned `count` entries at `buf`, valid until the next call. the lock
    // serializes every call in this process until this slice is copied into owned paths.
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

/// elsewhere only the device check applies.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn mount_points() -> io::Result<Vec<PathBuf>> {
    Ok(Vec::new())
}

/// same test as `is_mount_boundary`, for an entry already stat'ed through its directory.
#[cfg(unix)]
pub(super) fn is_boundary(
    st: &FileStat,
    dir_st: &FileStat,
    real: &Path,
    mounts: &MountTable,
) -> io::Result<bool> {
    if file_kind(st) == SFlag::S_IFLNK {
        return Ok(false);
    }
    Ok(st.st_dev != dir_st.st_dev || mounts.contains(real)?)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn mount_table_lists_the_root_filesystem() {
        assert!(MountTable::read().contains(Path::new("/")).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(
            unescape_mountinfo(br"/mnt/a\040b\134c\011"),
            b"/mnt/a b\\c\t"
        );
        assert_eq!(unescape_mountinfo(br"\12"), br"\12");
    }

    #[test]
    fn unreadable_table_refuses_every_lookup() {
        let table = MountTable(Err("denied".to_owned()));

        let err = table.contains(Path::new("/")).unwrap_err().to_string();

        assert_eq!(err, "could not read the mount table: denied");
    }
}
