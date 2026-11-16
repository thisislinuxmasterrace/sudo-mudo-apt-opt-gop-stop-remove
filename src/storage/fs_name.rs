//! the name of the filesystem that holds a path, and the mount table lookups behind it.

use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

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
    (0x2405_1905, "ubifs"),
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
pub(crate) fn filesystem_name(path: &Path) -> Option<String> {
    use nix::sys::statfs::statfs;

    #[allow(clippy::unnecessary_cast)]
    let raw = statfs(path).ok()?.filesystem_type().0 as i64;
    match magic_name(magic_bits(raw))? {
        "fuse" => mount_fs_type(path),
        name => Some(name.to_owned()),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn opened_filesystem_name(file: &std::fs::File) -> Option<String> {
    #[allow(clippy::unnecessary_cast)]
    let raw = nix::sys::statfs::fstatfs(file).ok()?.filesystem_type().0 as i64;
    magic_name(magic_bits(raw)).map(str::to_owned)
}

#[cfg(target_os = "macos")]
pub(crate) fn opened_filesystem_name(file: &std::fs::File) -> Option<String> {
    Some(
        nix::sys::statfs::fstatfs(file)
            .ok()?
            .filesystem_type_name()
            .to_owned(),
    )
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub(crate) fn opened_filesystem_name(_file: &std::fs::File) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn filesystem_name(path: &Path) -> Option<String> {
    let stat = nix::sys::statfs::statfs(path).ok()?;
    Some(stat.filesystem_type_name().to_owned())
}

#[cfg(windows)]
pub(crate) fn filesystem_name(path: &Path) -> Option<String> {
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
pub(super) fn remote_drive(path: &Path) -> bool {
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
pub(super) fn remote_drive(_path: &Path) -> bool {
    false
}

/// the device the filesystem holding `path` is mounted from, read from `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
pub(super) fn mount_source(path: &Path) -> Option<PathBuf> {
    mount_entry(path).map(|entry| entry.source)
}

/// the filesystem type of the mount that holds `path`, such as `fuse.sshfs`.
#[cfg(target_os = "linux")]
fn mount_fs_type(path: &Path) -> Option<String> {
    mount_entry(path).map(|entry| entry.fs_type)
}

/// the comma separated super options (`rw,compress=zstd:3,...`) of the mount that holds `path`.
#[cfg(target_os = "linux")]
pub(super) fn mount_super_options(path: &Path) -> Option<String> {
    mount_entry(path).map(|entry| entry.options)
}

/// the mountinfo entry of the mount that holds `path`.
#[cfg(target_os = "linux")]
fn mount_entry(path: &Path) -> Option<super::mountinfo::Entry> {
    let table = super::mountinfo::read_table().ok()?;
    let canonical = std::fs::canonicalize(path).ok()?;
    super::mountinfo::entry_for(&table, &canonical)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn filesystem_name(_path: &Path) -> Option<String> {
    None
}

/// NUL-terminated root of the volume that holds `path`.
#[cfg(windows)]
pub(super) fn volume_root(path: &Path) -> Option<Vec<u16>> {
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

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
        assert_eq!(magic_name(0x2405_1905), Some("ubifs"));
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
}
