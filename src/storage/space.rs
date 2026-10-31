//! how many bytes a user can still write on the volume that holds a path.

#[cfg(windows)]
use super::fs_name::volume_root;
use std::path::Path;

/// bytes the current user can still write on the filesystem that holds `path`. on unix the
/// blocks reserved for root are not counted, because ordinary users cannot fill them.
#[cfg(unix)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    // block counts are u32 on some unixes and u64 on others, so one of these is a no-op.
    #[allow(clippy::useless_conversion)]
    let blocks = u64::from(stat.blocks_available());
    #[allow(clippy::useless_conversion)]
    let fragment = u64::from(stat.fragment_size());
    blocks.checked_mul(fragment)
}

#[cfg(windows)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    use std::ptr;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let root = volume_root(path)?;
    let mut available = 0u64;
    // safety: root is NUL-terminated; only the caller-visible free bytes are requested, the
    // total and the volume-wide free counts are passed as null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            root.as_ptr(),
            &mut available,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

#[cfg(not(any(unix, windows)))]
pub fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(unix, windows))]
    #[test]
    fn reports_space_for_a_temp_directory() {
        let dir = tempfile::tempdir().unwrap();

        assert!(available_bytes(dir.path()).is_some_and(|bytes| bytes > 0));
    }
}
