//! flash detection through sysfs (linux only; elsewhere the answer is always no).

use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use super::fs_name::mount_source;

/// true when the device under `path` is known to be flash. only linux reports this, through
/// sysfs; elsewhere, and when the answer is unclear, it is false.
#[cfg(target_os = "linux")]
pub(super) fn is_flash(path: &Path) -> bool {
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
pub(super) fn is_flash(_path: &Path) -> bool {
    false
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

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::*;
    #[cfg(target_os = "linux")]
    use std::fs;

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
