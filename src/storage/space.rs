//! the size and the free space of the volume that holds a path.

#[cfg(windows)]
use super::fs_name::volume_root;
use std::path::Path;

/// the size and the free space of the filesystem that holds `path`, in bytes: `(total, free)`.
/// free counts the blocks reserved for root too, which is what a run as root can fill.
#[cfg(unix)]
fn volume_bytes(path: &Path) -> Option<(u64, u64)> {
    sizes(&nix::sys::statvfs::statvfs(path).ok()?)
}

/// size of the filesystem behind an open directory. the descriptor cannot be redirected by a
/// swapped path.
#[cfg(unix)]
pub fn total_bytes_of_open(dir: &std::fs::File) -> Option<u64> {
    sizes(&nix::sys::statvfs::fstatvfs(dir).ok()?).map(|(total, _)| total)
}

#[cfg(unix)]
fn sizes(stat: &nix::sys::statvfs::Statvfs) -> Option<(u64, u64)> {
    // block counts are u32 on some unixes and u64 on others, so one of these is a no-op.
    #[allow(clippy::useless_conversion)]
    let (blocks, free, fragment) = (
        u64::from(stat.blocks()),
        u64::from(stat.blocks_free()),
        u64::from(stat.fragment_size()),
    );
    Some((blocks.checked_mul(fragment)?, free.checked_mul(fragment)?))
}

#[cfg(windows)]
fn volume_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::ptr;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let root = volume_root(path)?;
    let (mut total, mut free) = (0u64, 0u64);
    // safety: root is NUL-terminated; the caller-visible free bytes are not requested, so that
    // pointer is null, and total and free point to live locals.
    let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), ptr::null_mut(), &mut total, &mut free) };
    (ok != 0).then_some((total, free))
}

#[cfg(not(any(unix, windows)))]
fn volume_bytes(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// size of the filesystem that holds `path`. a fill pass cannot honestly write more than this.
#[cfg(not(unix))]
pub fn total_bytes(path: &Path) -> Option<u64> {
    volume_bytes(path).map(|(total, _)| total)
}

/// free bytes of the filesystem that holds `path`, including the blocks reserved for root.
pub fn free_bytes(path: &Path) -> Option<u64> {
    volume_bytes(path).map(|(_, free)| free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(unix, windows))]
    #[test]
    fn reports_size_and_space_for_a_temp_directory() {
        let dir = tempfile::tempdir().unwrap();

        let total = volume_bytes(dir.path()).unwrap().0;
        assert!(total > 0);
        assert!(free_bytes(dir.path()).is_some_and(|free| free <= total));
    }
}
