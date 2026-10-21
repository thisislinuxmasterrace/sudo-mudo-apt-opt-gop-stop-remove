//! advisory checks on the storage behind a path. warnings never block an erase.

use std::path::{Path, PathBuf};

/// filesystems that write to new blocks instead of overwriting in place.
const COPY_ON_WRITE: &[&str] = &["apfs", "btrfs", "zfs"];

/// one warning per path that lives on a copy-on-write filesystem.
pub fn copy_on_write_warnings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| copy_on_write_warning(path, &filesystem_name(path)?))
        .collect()
}

fn copy_on_write_warning(path: &Path, fs_name: &str) -> Option<String> {
    COPY_ON_WRITE.contains(&fs_name).then(|| {
        format!(
            "{}: {fs_name} is copy-on-write, old data may survive overwriting",
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

    match statfs(path).ok()?.filesystem_type().0 as u64 {
        BTRFS_SUPER_MAGIC => Some("btrfs".to_owned()),
        ZFS_SUPER_MAGIC => Some("zfs".to_owned()),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn filesystem_name(path: &Path) -> Option<String> {
    let stat = nix::sys::statfs::statfs(path).ok()?;
    Some(stat.filesystem_type_name().to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn filesystem_name(_path: &Path) -> Option<String> {
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
    }

    #[test]
    fn ignores_in_place_filesystems() {
        let path = Path::new("/data/file");
        assert!(copy_on_write_warning(path, "ext4").is_none());
        assert!(copy_on_write_warning(path, "xfs").is_none());
        assert!(copy_on_write_warning(path, "ntfs").is_none());
    }
}
