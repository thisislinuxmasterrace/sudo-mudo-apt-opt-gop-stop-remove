//! advisory checks on the storage behind a path. warnings never block an erase.

use std::path::{Path, PathBuf};

/// filesystems that write to new blocks instead of overwriting in place.
/// refs block cloning makes several files share the same clusters.
const COPY_ON_WRITE: &[&str] = &["apfs", "btrfs", "zfs", "refs"];

/// network filesystems. overwriting happens on the server, which may keep copies.
const NETWORK: &[&str] = &["nfs", "smb", "cifs", "smb2", "smbfs", "afpfs", "webdav"];

/// warnings for every path that lives on a copy-on-write or network filesystem.
pub fn warnings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().flat_map(|path| path_warnings(path)).collect()
}

/// true when `path` is on a network filesystem or a remote drive. erasing there is allowed
/// with a warning; filling its free space is not, because that space belongs to the server.
pub fn is_network(path: &Path) -> bool {
    filesystem_name(path).is_some_and(|name| NETWORK.contains(&name.as_str())) || remote_drive(path)
}

/// true on filesystems where files can share clusters (refs block cloning): overwriting one
/// file then changes the others as well.
#[cfg(windows)]
pub fn shares_clusters(path: &Path) -> bool {
    filesystem_name(path).is_some_and(|name| name == "refs")
}

fn path_warnings(path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(name) = filesystem_name(path) {
        found.extend(copy_on_write_warning(path, &name));
        found.extend(network_warning(path, &name));
    }
    if remote_drive(path) {
        found.push(format!(
            "{}: remote drive, overwriting happens on the server, which may keep copies",
            path.display()
        ));
    }
    found
}

fn copy_on_write_warning(path: &Path, fs_name: &str) -> Option<String> {
    COPY_ON_WRITE.contains(&fs_name).then(|| {
        format!(
            "{}: {fs_name} is copy-on-write, old data may survive overwriting",
            path.display()
        )
    })
}

fn network_warning(path: &Path, fs_name: &str) -> Option<String> {
    NETWORK.contains(&fs_name).then(|| {
        format!(
            "{}: {fs_name} is a network filesystem, overwriting happens on the server, which may keep copies",
            path.display()
        )
    })
}

#[cfg(target_os = "linux")]
fn filesystem_name(path: &Path) -> Option<String> {
    use nix::sys::statfs::statfs;

    // magic numbers from linux/magic.h
    const BTRFS_SUPER_MAGIC: u64 = 0x9123_683E;
    const ZFS_SUPER_MAGIC: u64 = 0x2FC1_2FC1;
    const NFS_SUPER_MAGIC: u64 = 0x6969;
    const SMB_SUPER_MAGIC: u64 = 0x517B;
    const CIFS_MAGIC_NUMBER: u64 = 0xFF53_4D42;
    const SMB2_MAGIC_NUMBER: u64 = 0xFE53_4D42;

    let name = match statfs(path).ok()?.filesystem_type().0 as u64 {
        BTRFS_SUPER_MAGIC => "btrfs",
        ZFS_SUPER_MAGIC => "zfs",
        NFS_SUPER_MAGIC => "nfs",
        SMB_SUPER_MAGIC => "smb",
        CIFS_MAGIC_NUMBER => "cifs",
        SMB2_MAGIC_NUMBER => "smb2",
        _ => return None,
    };
    Some(name.to_owned())
}

#[cfg(target_os = "macos")]
fn filesystem_name(path: &Path) -> Option<String> {
    let stat = nix::sys::statfs::statfs(path).ok()?;
    Some(stat.filesystem_type_name().to_owned())
}

#[cfg(windows)]
fn filesystem_name(path: &Path) -> Option<String> {
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;

    let root = volume_root(path)?;
    let mut name = [0u16; 64];
    // safety: root is NUL-terminated (from volume_root); the volume name and the serial and
    // flags outputs are not requested, so they are passed as null.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }

    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..len]).to_ascii_lowercase())
}

#[cfg(windows)]
fn remote_drive(path: &Path) -> bool {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    // drive_remote from winbase.h; not exported by the feature set used here
    const DRIVE_REMOTE: u32 = 4;

    let Some(root) = volume_root(path) else {
        return false;
    };
    // safety: root is NUL-terminated (from volume_root).
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(not(windows))]
fn remote_drive(_path: &Path) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn filesystem_name(_path: &Path) -> Option<String> {
    None
}

/// NUL-terminated root of the volume that holds `path`.
#[cfg(windows)]
fn volume_root(path: &Path) -> Option<Vec<u16>> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
    let mut root = [0u16; 512];
    // safety: wide is NUL-terminated and root is writable for the length passed.
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return None;
    }
    let len = root.iter().position(|&c| c == 0).unwrap_or(root.len());
    Some(root[..len].iter().copied().chain(once(0)).collect())
}

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

    #[test]
    fn flags_copy_on_write_filesystems() {
        let path = Path::new("/data/file");
        assert!(copy_on_write_warning(path, "apfs").is_some());
        assert!(copy_on_write_warning(path, "btrfs").is_some());
        assert!(copy_on_write_warning(path, "zfs").is_some());
        assert!(copy_on_write_warning(path, "refs").is_some());
    }

    #[test]
    fn ignores_in_place_filesystems() {
        let path = Path::new("/data/file");
        assert!(copy_on_write_warning(path, "ext4").is_none());
        assert!(copy_on_write_warning(path, "xfs").is_none());
        assert!(copy_on_write_warning(path, "ntfs").is_none());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn reports_space_for_a_temp_directory() {
        let dir = tempfile::tempdir().unwrap();

        assert!(available_bytes(dir.path()).is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn flags_network_filesystems() {
        let path = Path::new("/mnt/share/file");
        assert!(network_warning(path, "nfs").is_some());
        assert!(network_warning(path, "cifs").is_some());
        assert!(network_warning(path, "smbfs").is_some());
        assert!(network_warning(path, "ext4").is_none());
    }
}
