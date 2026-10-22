//! advisory checks on the storage behind a path. warnings never block an erase.

use std::path::{Path, PathBuf};

/// filesystems that write to new blocks instead of overwriting in place.
/// refs block cloning makes several files share the same clusters.
const COPY_ON_WRITE: &[&str] = &["apfs", "btrfs", "zfs", "refs", "bcachefs"];

/// local filesystems: overwriting stays on this machine.
const LOCAL: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "f2fs", "tmpfs", "ramfs", "overlay", "jfs", "reiserfs",
    "squashfs", "iso9660", "cd9660", "cdfs", "udf", "vfat", "msdos", "fat", "fat32", "exfat",
    "ntfs", "ntfs3", "hfs", "hfsplus", "erofs", "nilfs2", "cramfs", "minix", "ocfs2", "ecryptfs",
    "ufs",
];

/// network filesystems. overwriting happens on the server, which may keep copies.
const NETWORK: &[&str] = &[
    "nfs",
    "smb",
    "cifs",
    "smb2",
    "smbfs",
    "afpfs",
    "webdav",
    "ceph",
    "glusterfs",
    "9p",
    "afs",
    "coda",
    "lustre",
];

/// how a filesystem type is treated. fuse names (`fuse.sshfs`, `fuseblk`, `macfuse`, ...) and
/// unlisted types both end up as `FuseOrUnknown`: they may send writes to a remote server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Local,
    CopyOnWrite,
    Network,
    FuseOrUnknown,
}

fn classify(fs_name: &str) -> Kind {
    if COPY_ON_WRITE.contains(&fs_name) {
        Kind::CopyOnWrite
    } else if LOCAL.contains(&fs_name) {
        Kind::Local
    } else if NETWORK.contains(&fs_name) {
        Kind::Network
    } else {
        Kind::FuseOrUnknown
    }
}

/// warnings for every path that lives on a copy-on-write, network, fuse or unrecognized filesystem.
pub fn warnings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().flat_map(|path| path_warnings(path)).collect()
}

/// true when filling the free space of `path` would write to a server, or to a filesystem this
/// module cannot classify: network filesystems, fuse mounts, unlisted types and remote drives.
/// erasing there is allowed with a warning; filling its free space is not, because that space
/// belongs to the server and other users.
pub fn is_network(path: &Path) -> bool {
    filesystem_name(path)
        .is_none_or(|name| matches!(classify(&name), Kind::Network | Kind::FuseOrUnknown))
        || remote_drive(path)
}

fn path_warnings(path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    match filesystem_name(path) {
        Some(name) => found.extend(fs_warning(path, &name)),
        None => found.push(unreadable_warning(path)),
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

/// the type of `path` could not be read: statfs failed, or a fuse mount had no mountinfo entry.
fn unreadable_warning(path: &Path) -> String {
    format!(
        "{}: could not read the filesystem type, it may be a remote one",
        path.display()
    )
}

fn fs_warning(path: &Path, fs_name: &str) -> Option<String> {
    let message = match classify(fs_name) {
        Kind::Local => return None,
        Kind::CopyOnWrite => {
            format!("{fs_name} is copy-on-write, old data may survive overwriting")
        }
        Kind::Network => format!(
            "{fs_name} is a network filesystem, overwriting happens on the server, which may keep copies"
        ),
        Kind::FuseOrUnknown => format!(
            "{fs_name} is a fuse or unrecognized filesystem, overwriting may happen on a remote server"
        ),
    };
    Some(format!("{}: {message}", path.display()))
}

/// filesystem names by magic number, from `include/uapi/linux/magic.h` in the kernel tree.
/// filesystems that share a magic share a name. one magic covers every fuse subtype, so fuse
/// is refined from mountinfo.
///
/// these values are not in magic.h; the source checked for each:
/// - ntfs 0x7366746E: `sb->s_magic` in `fs/ntfs3/super.c`, which statfs reports.
/// - ntfs 0x5346544E: the removed ntfs driver's value, as far as known; not found in the tree (unverified).
/// - hfs 0x4244, hfsplus 0x482B: `include/linux/hfs_common.h`.
/// - jfs 0x3153464A: the bytes of `JFS_MAGIC` ("JFS1", `fs/jfs/jfs_superblock.h`); the statfs
///   constant `JFS_SUPER_MAGIC` was not found in the sources checked (unverified).
/// - zfs 0x2FC12FC1, glusterfs 0x01161970, lustre 0x0BD00BD0: not found in any source checked (unverified).
#[cfg(target_os = "linux")]
const LINUX_MAGIC: &[(u64, &str)] = &[
    (0x9123_683E, "btrfs"),
    (0x2FC1_2FC1, "zfs"),
    (0xEF53, "ext4"),
    (0x5846_5342, "xfs"),
    (0xF2F5_2010, "f2fs"),
    (0x0102_1994, "tmpfs"),
    (0x8584_58F6, "ramfs"),
    (0x794C_7630, "overlay"),
    (0x4D44, "vfat"),
    (0x2011_BAB0, "exfat"),
    (0x7366_746E, "ntfs"),
    (0x5346_544E, "ntfs"),
    (0x9660, "iso9660"),
    (0x1501_3346, "udf"),
    (0x7371_7368, "squashfs"),
    (0x3153_464A, "jfs"),
    (0x5265_4973, "reiserfs"),
    (0xCA45_1A4E, "bcachefs"),
    (0x482B, "hfsplus"),
    (0x4244, "hfs"),
    (0x6969, "nfs"),
    (0x517B, "smb"),
    (0xFF53_4D42, "cifs"),
    (0xFE53_4D42, "smb2"),
    (0x00C3_6400, "ceph"),
    (0x0116_1970, "glusterfs"),
    (0x0102_1997, "9p"),
    (0x5346_414F, "afs"),
    (0x6B41_4653, "afs"),
    (0x7375_7245, "coda"),
    (0x0BD0_0BD0, "lustre"),
    (0x6573_5546, "fuse"),
    (0xE0F5_E1E2, "erofs"),
    (0x3434, "nilfs2"),
    (0x28CD_3D45, "cramfs"),
    (0x137F, "minix"),
    (0x138F, "minix"),
    (0x2468, "minix"),
    (0x2478, "minix"),
    (0x4D5A, "minix"),
    (0x7461_636F, "ocfs2"),
    (0xF15F, "ecryptfs"),
];

#[cfg(target_os = "linux")]
fn magic_name(magic: u64) -> Option<&'static str> {
    LINUX_MAGIC
        .iter()
        .find(|(known, _)| *known == magic)
        .map(|(_, name)| *name)
}

/// the low 32 bits of a statfs magic. statfs returns the field as a signed word on some
/// architectures, so a 32-bit magic can arrive sign-extended.
#[cfg(target_os = "linux")]
fn magic_bits(raw: i64) -> u64 {
    u64::from(raw as u32)
}

#[cfg(target_os = "linux")]
fn filesystem_name(path: &Path) -> Option<String> {
    use nix::sys::statfs::statfs;

    #[allow(clippy::useless_conversion)]
    let raw = i64::from(statfs(path).ok()?.filesystem_type().0);
    match magic_name(magic_bits(raw))? {
        "fuse" => mount_fs_type(path),
        name => Some(name.to_owned()),
    }
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
    mount_entry(path).map(|(_, source)| PathBuf::from(source))
}

/// the filesystem type of the mount that holds `path`, such as `fuse.sshfs`.
#[cfg(target_os = "linux")]
fn mount_fs_type(path: &Path) -> Option<String> {
    mount_entry(path).map(|(fs_type, _)| fs_type)
}

/// the (fs type, source) of the mount that holds `path`, from `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
fn mount_entry(path: &Path) -> Option<(String, String)> {
    let table = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let canonical = std::fs::canonicalize(path).ok()?;
    mount_entry_in(&table, &canonical)
}

/// the (fs type, source) of the longest mount point that contains `path`; `table` is mountinfo text.
#[cfg(target_os = "linux")]
fn mount_entry_in(table: &str, path: &Path) -> Option<(String, String)> {
    let mut best: Option<(usize, String, String)> = None;
    for line in table.lines() {
        // fields before " - " end with the mount point; fields after it start with the fs type and source
        let Some((head, tail)) = line.split_once(" - ") else {
            continue;
        };
        let Some(escaped) = head.split(' ').nth(4) else {
            continue;
        };
        let mount_point = PathBuf::from(crate::erase::unescape_mountinfo(escaped));
        let mut fields = tail.split(' ');
        let (Some(fs_type), Some(source)) = (fields.next(), fields.next()) else {
            continue;
        };
        // a later line on the same mount point is stacked on top of an earlier one, so it wins the tie
        if path.starts_with(&mount_point)
            && best
                .as_ref()
                .is_none_or(|(len, _, _)| mount_point.as_os_str().len() >= *len)
        {
            best = Some((
                mount_point.as_os_str().len(),
                fs_type.to_owned(),
                source.to_owned(),
            ));
        }
    }
    best.map(|(_, fs_type, source)| (fs_type, source))
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
    fn classifies_copy_on_write_filesystems() {
        for name in ["apfs", "btrfs", "zfs", "refs", "bcachefs"] {
            assert_eq!(classify(name), Kind::CopyOnWrite, "{name}");
        }
    }

    #[test]
    fn classifies_local_filesystems() {
        for name in [
            "ext2", "ext4", "xfs", "f2fs", "tmpfs", "ramfs", "overlay", "vfat", "msdos", "fat32",
            "exfat", "ntfs", "ntfs3", "hfs", "hfsplus", "jfs", "reiserfs", "iso9660", "cd9660",
            "cdfs", "udf", "squashfs", "erofs", "nilfs2", "cramfs", "minix", "ocfs2", "ecryptfs",
            "ufs",
        ] {
            assert_eq!(classify(name), Kind::Local, "{name}");
        }
    }

    #[test]
    fn classifies_network_filesystems() {
        for name in [
            "nfs",
            "smb",
            "cifs",
            "smb2",
            "smbfs",
            "afpfs",
            "webdav",
            "ceph",
            "glusterfs",
            "9p",
            "afs",
            "coda",
            "lustre",
        ] {
            assert_eq!(classify(name), Kind::Network, "{name}");
        }
    }

    #[test]
    fn classifies_fuse_as_fuse_or_unknown() {
        for name in [
            "fuse",
            "fuse.sshfs",
            "fuse.rclone",
            "fuseblk",
            "macfuse",
            "osxfuse",
        ] {
            assert_eq!(classify(name), Kind::FuseOrUnknown, "{name}");
        }
    }

    #[test]
    fn classifies_unlisted_filesystems_as_unknown() {
        for name in ["", "weirdfs", "unknown"] {
            assert_eq!(classify(name), Kind::FuseOrUnknown, "{name}");
        }
    }

    #[test]
    fn warns_about_network_and_fuse_but_not_local() {
        let path = Path::new("/mnt/share/file");
        assert!(fs_warning(path, "ceph").is_some_and(|w| w.contains("is a network filesystem")));
        assert!(
            fs_warning(path, "fuse.sshfs")
                .is_some_and(|w| w.contains("is a fuse or unrecognized filesystem"))
        );
        assert!(fs_warning(path, "btrfs").is_some_and(|w| w.contains("copy-on-write")));
        assert!(fs_warning(path, "ext4").is_none());
        assert!(
            unreadable_warning(path)
                .contains("could not read the filesystem type, it may be a remote one")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_filesystem_counts_as_network() {
        assert!(is_network(Path::new(
            "/nonexistent/sudo-mudo-apt-opt-gop-stop-remove"
        )));
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
            mount_entry_in(&table, Path::new("/usr/bin")).map(|(_, source)| source),
            Some("/dev/vdb1".to_owned())
        );
        assert_eq!(
            mount_entry_in(&table, Path::new("/home/a/file")).map(|(_, source)| source),
            Some("/dev/vdc1".to_owned())
        );
        assert_eq!(
            mount_entry_in(&table, Path::new("/home/with space/file")).map(|(_, source)| source),
            Some("tmpfs".to_owned())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_entry_reports_the_fuse_subtype() {
        let table =
            "41 25 0:40 / /home/me/remote rw,nosuid - fuse.sshfs user@host:/ rw,user_id=1000";
        assert_eq!(
            mount_entry_in(table, Path::new("/home/me/remote/a")),
            Some(("fuse.sshfs".to_owned(), "user@host:/".to_owned()))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_entry_prefers_the_last_mount_on_the_same_point() {
        let table = [
            "40 25 0:30 / /home/me/remote rw - ext4 /dev/vdd1 rw",
            "41 25 0:40 / /home/me/remote rw,nosuid - fuse.sshfs user@host:/ rw,user_id=1000",
        ]
        .join("\n");
        assert_eq!(
            mount_entry_in(&table, Path::new("/home/me/remote/a")),
            Some(("fuse.sshfs".to_owned(), "user@host:/".to_owned()))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn magic_numbers_name_network_and_fuse_filesystems() {
        assert_eq!(magic_name(0x6969), Some("nfs"));
        assert_eq!(magic_name(0xFF53_4D42), Some("cifs"));
        assert_eq!(magic_name(0xFE53_4D42), Some("smb2"));
        assert_eq!(magic_name(0x00C3_6400), Some("ceph"));
        assert_eq!(magic_name(0x0116_1970), Some("glusterfs"));
        assert_eq!(magic_name(0x0102_1997), Some("9p"));
        assert_eq!(magic_name(0x6573_5546), Some("fuse"));
        assert_eq!(magic_name(0x7366_746E), Some("ntfs"));
        assert_eq!(magic_name(0x6B41_4653), Some("afs"));
        assert_eq!(magic_name(0x1234_5678), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn statfs_sign_extension_is_undone_by_the_low_32_bits() {
        assert_eq!(magic_bits(-0x00AC_B2BE), 0xFF53_4D42);
        assert_eq!(magic_name(magic_bits(-0x00AC_B2BE)), Some("cifs"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_magic_values_with_different_names_do_not_collide() {
        for (i, (magic, name)) in LINUX_MAGIC.iter().enumerate() {
            for (other, other_name) in &LINUX_MAGIC[i + 1..] {
                assert!(
                    magic != other || name == other_name,
                    "{magic:#x} is both {name} and {other_name}"
                );
            }
        }
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
}
