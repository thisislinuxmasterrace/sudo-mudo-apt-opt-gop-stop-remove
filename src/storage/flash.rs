//! flash detection through sysfs (linux only; elsewhere the answer is always unknown).

use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use super::fs_name::mount_source;

/// what is known about the medium under a path. `Rotational` is only what the kernel reports in
/// `queue/rotational`: a disk that lies, such as a usb bridge or a virtual disk, ends up `Unknown`.
// only linux detects the medium; elsewhere the other variants exist for the shared reports.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Medium {
    Flash,
    Rotational,
    Unknown,
}

impl Medium {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Flash => "flash",
            Self::Rotational => "rotational",
            Self::Unknown => "unknown",
        }
    }
}

/// true when the device under `path` is known to be flash. only linux reports this, through
/// sysfs; elsewhere, and when the answer is unclear, it is false.
#[cfg(target_os = "linux")]
pub(super) fn is_flash(path: &Path) -> bool {
    medium(path) == Medium::Flash
}

#[cfg(target_os = "linux")]
pub(crate) fn medium(path: &Path) -> Medium {
    block_dir(path).map_or(Medium::Unknown, |dir| medium_of(&dir, 0))
}

/// the sysfs directory of the block device that holds `path`, `None` when it cannot be told.
#[cfg(target_os = "linux")]
pub(super) fn block_dir(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Ok(meta) = std::fs::metadata(path) else {
        return None;
    };
    let mut dev = meta.dev();
    // btrfs and similar filesystems report an anonymous device (major 0): the disk is the mount source.
    if nix::sys::stat::major(dev) == 0 {
        let source = mount_source(path).and_then(|source| std::fs::metadata(source).ok());
        match source {
            Some(source) if source.file_type().is_block_device() => dev = source.rdev(),
            _ => return None,
        }
    }
    Some(PathBuf::from(format!(
        "/sys/dev/block/{}:{}",
        nix::sys::stat::major(dev),
        nix::sys::stat::minor(dev)
    )))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn is_flash(_path: &Path) -> bool {
    false
}

/// why the `queue/rotational` value of the disk at `disk_dir` cannot be trusted, `None` when nothing
/// speaks against it. usb bridges, removable media and virtual disks report whatever their firmware or
/// hypervisor makes up, so a report of 1 there proves nothing about the medium.
#[cfg(target_os = "linux")]
pub(crate) fn untrusted_reason(disk_dir: &Path) -> Option<&'static str> {
    let read = |name: &str| {
        std::fs::read_to_string(disk_dir.join(name))
            .ok()
            .map(|text| text.trim().to_owned())
    };
    if read("removable").as_deref() == Some("1") {
        return Some("removable media");
    }
    // fail closed: a path that cannot be inspected keeps the medium unknown.
    let Ok(resolved) = std::fs::canonicalize(disk_dir) else {
        return Some("the device path could not be inspected");
    };
    // hyper-v spells its bus VMBUS in sysfs.
    let resolved = resolved.to_string_lossy().to_ascii_lowercase();
    if resolved.contains("/usb") {
        return Some("usb transport");
    }
    // vendor strings are padded, and some carry a trailing comma ("VMware, ").
    let vendor = read("device/vendor");
    let vendor = vendor.as_deref().map(|v| v.trim_end_matches([',', ' ']));
    let virtual_model = read("device/model").is_some_and(|model| model.contains("Virtual Disk"));
    if resolved.contains("/virtio")
        || resolved.contains("/vmbus")
        || matches!(vendor, Some("QEMU" | "VMware" | "VBOX" | "Msft"))
        || virtual_model
    {
        return Some("virtual or hypervisor-backed disk");
    }
    None
}

/// the medium of a device. partitions take the answer of their disk, device-mapper and md devices of
/// their slaves; slaves that disagree make it unknown.
#[cfg(target_os = "linux")]
fn medium_of(block_dir: &Path, depth: u32) -> Medium {
    if depth > 4 {
        return Medium::Unknown;
    }
    let Ok(dir) = std::fs::canonicalize(block_dir) else {
        return Medium::Unknown;
    };
    if let Ok(entries) = std::fs::read_dir(dir.join("slaves")) {
        let answers: Vec<Medium> = entries
            .filter_map(Result::ok)
            .map(|entry| medium_of(&entry.path(), depth + 1))
            .collect();
        if let Some(first) = answers.first().copied() {
            return if answers.iter().all(|answer| *answer == first) {
                first
            } else {
                Medium::Unknown
            };
        }
    }
    if let Ok(value) = std::fs::read_to_string(dir.join("queue").join("rotational")) {
        return match value.trim() {
            "0" => Medium::Flash,
            "1" if untrusted_reason(&dir).is_none() => Medium::Rotational,
            _ => Medium::Unknown,
        };
    }
    if dir.join("partition").exists() {
        return dir
            .parent()
            .map_or(Medium::Unknown, |disk| medium_of(disk, depth + 1));
    }
    Medium::Unknown
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::*;
    #[cfg(target_os = "linux")]
    use std::fs;

    #[cfg(target_os = "linux")]
    fn disk_with(dir: &Path, rotational: &str) -> PathBuf {
        let disk = dir.join("sda");
        fs::create_dir_all(disk.join("queue")).unwrap();
        fs::write(disk.join("queue").join("rotational"), rotational).unwrap();
        disk
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_partition_takes_the_answer_of_its_disk() {
        let dir = tempfile::tempdir().unwrap();
        let disk = disk_with(dir.path(), "0\n");
        fs::create_dir_all(disk.join("sda1")).unwrap();
        fs::write(disk.join("sda1").join("partition"), "1\n").unwrap();

        assert_eq!(medium_of(&disk.join("sda1"), 0), Medium::Flash);
        fs::write(disk.join("queue").join("rotational"), "1\n").unwrap();
        assert_eq!(medium_of(&disk.join("sda1"), 0), Medium::Rotational);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_device_mapper_uses_its_slaves() {
        let dir = tempfile::tempdir().unwrap();
        let disk = disk_with(dir.path(), "0\n");
        fs::create_dir_all(disk.join("sda1")).unwrap();
        fs::write(disk.join("sda1").join("partition"), "1\n").unwrap();
        let dm = dir.path().join("dm-0");
        fs::create_dir_all(dm.join("slaves")).unwrap();
        std::os::unix::fs::symlink(disk.join("sda1"), dm.join("slaves").join("sda1")).unwrap();

        assert_eq!(medium_of(&dm, 0), Medium::Flash);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_unknown_device_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(medium_of(dir.path(), 0), Medium::Unknown);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_rotational_report_is_not_trusted_on_removable_usb_or_virtual_disks() {
        let dir = tempfile::tempdir().unwrap();
        let disk = disk_with(dir.path(), "1\n");
        assert_eq!(untrusted_reason(&disk), None);
        assert_eq!(medium_of(&disk, 0), Medium::Rotational);

        fs::write(disk.join("removable"), "1\n").unwrap();
        assert_eq!(untrusted_reason(&disk), Some("removable media"));
        assert_eq!(medium_of(&disk, 0), Medium::Unknown);
        fs::write(disk.join("removable"), "0\n").unwrap();

        fs::create_dir_all(disk.join("device")).unwrap();
        for (vendor, model) in [
            ("QEMU    ", "QEMU HARDDISK"),
            ("VMware, ", "VMware Virtual S"),
            ("VBOX    ", "HARDDISK"),
            ("Msft    ", "x"),
            ("ATA     ", "Virtual Disk"),
        ] {
            fs::write(disk.join("device/vendor"), vendor).unwrap();
            fs::write(disk.join("device/model"), model).unwrap();
            assert_eq!(
                untrusted_reason(&disk),
                Some("virtual or hypervisor-backed disk"),
                "{vendor}"
            );
        }
        fs::write(disk.join("device/vendor"), "ATA     ").unwrap();
        fs::write(disk.join("device/model"), "ST1000DM010").unwrap();
        assert_eq!(untrusted_reason(&disk), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn usb_and_virtio_paths_are_untrusted_and_flash_stays_flash() {
        let dir = tempfile::tempdir().unwrap();
        for (path, reason) in [
            ("pci/usb1/1-2/block/sdb", "usb transport"),
            ("pci/virtio3/block/vda", "virtual or hypervisor-backed disk"),
            ("vmbus/block/sda", "virtual or hypervisor-backed disk"),
        ] {
            let disk = disk_with(&dir.path().join(path), "1\n");
            assert_eq!(untrusted_reason(&disk), Some(reason), "{path}");
            assert_eq!(medium_of(&disk, 0), Medium::Unknown, "{path}");
            fs::write(disk.join("queue").join("rotational"), "0\n").unwrap();
            assert_eq!(medium_of(&disk, 0), Medium::Flash, "{path}");
        }
    }
}
