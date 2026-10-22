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
    if is_flash(path) {
        found.push(format!(
            "{}: on a flash (non-rotational) device, the device may keep old data in blocks it remapped",
            path.display()
        ));
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

/// true when the device under `path` is known to be flash. only linux reports this, through
/// sysfs; elsewhere, and when the answer is unclear, it is false.
#[cfg(target_os = "linux")]
fn is_flash(path: &Path) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let mut dev = meta.dev();
    // btrfs and similar filesystems report an anonymous device (major 0): the disk is the mount source.
    if nix::sys::stat::major(dev) == 0 {
        let source = mount_source(path).and_then(|source| std::fs::metadata(source).ok());
        match source {
            Some(source) if source.file_type().is_block_device() => dev = source.rdev(),
            _ => return false,
        }
    }
    let block = PathBuf::from(format!(
        "/sys/dev/block/{}:{}",
        nix::sys::stat::major(dev),
        nix::sys::stat::minor(dev)
    ));
    non_rotational(&block, 0) == Some(true)
}

#[cfg(not(target_os = "linux"))]
fn is_flash(_path: &Path) -> bool {
    false
}

/// the device the filesystem holding `path` is mounted from, read from `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
fn mount_source(path: &Path) -> Option<PathBuf> {
    let table = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let canonical = std::fs::canonicalize(path).ok()?;
    mount_source_in(&table, &canonical).map(PathBuf::from)
}

/// the source of the longest mount point that contains `path`; `table` is mountinfo text.
#[cfg(target_os = "linux")]
fn mount_source_in(table: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in table.lines() {
        // fields before " - " end with the mount point; fields after it start with the fs type and source
        let Some((head, tail)) = line.split_once(" - ") else {
            continue;
        };
        let Some(escaped) = head.split(' ').nth(4) else {
            continue;
        };
        let mount_point = PathBuf::from(crate::erase::unescape_mountinfo(escaped));
        let Some(source) = tail.split(' ').nth(1) else {
            continue;
        };
        if path.starts_with(&mount_point)
            && best
                .as_ref()
                .is_none_or(|(len, _)| mount_point.as_os_str().len() > *len)
        {
            best = Some((mount_point.as_os_str().len(), source.to_owned()));
        }
    }
    best.map(|(_, source)| source)
}

/// `Some(true)` for a flash device, `Some(false)` for a rotating one, `None` when unknown.
/// partitions take the answer of their disk, device-mapper and md devices of their slaves.
#[cfg(target_os = "linux")]
fn non_rotational(block_dir: &Path, depth: u32) -> Option<bool> {
    if depth > 4 {
        return None;
    }
    let dir = std::fs::canonicalize(block_dir).ok()?;
    if let Ok(entries) = std::fs::read_dir(dir.join("slaves")) {
        let answers: Vec<Option<bool>> = entries
            .filter_map(Result::ok)
            .map(|entry| non_rotational(&entry.path(), depth + 1))
            .collect();
        if !answers.is_empty() {
            return if answers.iter().all(|a| *a == Some(true)) {
                Some(true)
            } else if answers.iter().all(|a| *a == Some(false)) {
                Some(false)
            } else {
                None
            };
        }
    }
    if let Ok(value) = std::fs::read_to_string(dir.join("queue").join("rotational")) {
        return match value.trim() {
            "0" => Some(true),
            "1" => Some(false),
            _ => None,
        };
    }
    if dir.join("partition").exists() {
        return non_rotational(dir.parent()?, depth + 1);
    }
    None
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
    #[cfg(target_os = "linux")]
    use std::fs;

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

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_source_is_the_longest_matching_mount_point() {
        let table = [
            "36 25 0:32 / / rw,relatime shared:1 - btrfs /dev/vdb1 rw",
            r"37 36 0:33 / /home/with\040space rw - tmpfs tmpfs rw",
            "38 36 0:34 / /home rw - ext4 /dev/vdc1 rw",
        ]
        .join("\n");
        assert_eq!(
            mount_source_in(&table, Path::new("/usr/bin")).as_deref(),
            Some("/dev/vdb1")
        );
        assert_eq!(
            mount_source_in(&table, Path::new("/home/a/file")).as_deref(),
            Some("/dev/vdc1")
        );
        assert_eq!(
            mount_source_in(&table, Path::new("/home/with space/file")).as_deref(),
            Some("tmpfs")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_partition_takes_the_answer_of_its_disk() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("sda");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("queue").join("rotational"), "0\n").unwrap();
        fs::create_dir_all(disk.join("sda1")).unwrap();
        fs::write(disk.join("sda1").join("partition"), "1\n").unwrap();

        assert_eq!(non_rotational(&disk.join("sda1"), 0), Some(true));
        fs::write(disk.join("queue").join("rotational"), "1\n").unwrap();
        assert_eq!(non_rotational(&disk.join("sda1"), 0), Some(false));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_device_mapper_uses_its_slaves() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("sda");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("queue").join("rotational"), "0\n").unwrap();
        fs::create_dir_all(disk.join("sda1")).unwrap();
        fs::write(disk.join("sda1").join("partition"), "1\n").unwrap();
        let dm = dir.path().join("dm-0");
        fs::create_dir_all(dm.join("slaves")).unwrap();
        std::os::unix::fs::symlink(disk.join("sda1"), dm.join("slaves").join("sda1")).unwrap();

        assert_eq!(non_rotational(&dm, 0), Some(true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_unknown_device_is_not_flash() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(non_rotational(dir.path(), 0), None);
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
