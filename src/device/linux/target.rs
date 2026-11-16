//! a device named on the command line: the path as typed and as resolved, its sysfs entry, and the checks
//! that need only these names (the same device listed twice, a disk together with its own partition).

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use super::sysfs::read_required;

/// a device named on the command line. `shown` is the path as typed, `path` is the resolved node.
pub(super) struct Target {
    pub(super) shown: PathBuf,
    pub(super) typed: String,
    pub(super) path: PathBuf,
    pub(super) name: String,
    /// the sysfs directory of the device, resolved (a partition's is under its disk).
    pub(super) sys: PathBuf,
    pub(super) partition: bool,
    pub(super) rdev: u64,
}

impl Target {
    pub(super) fn resolve(shown: &Path) -> Result<Self, String> {
        let typed = shown
            .to_str()
            .ok_or_else(|| {
                format!(
                    "{}: the path is not valid utf-8, it cannot be typed to confirm",
                    crate::ui::shown(shown)
                )
            })?
            .to_owned();
        let path =
            fs::canonicalize(shown).map_err(|err| format!("{}: {err}", crate::ui::shown(shown)))?;
        let meta =
            fs::metadata(&path).map_err(|err| format!("{}: {err}", crate::ui::shown(shown)))?;
        if !meta.file_type().is_block_device() {
            return Err(format!("{}: not a block device", crate::ui::shown(shown)));
        }
        // the sysfs entry comes from the device number of the node, not from its file name: a node
        // can carry any name.
        let sys = sys_entry(Path::new("/sys"), meta.rdev())
            .map_err(|err| format!("{}: {err}", crate::ui::shown(shown)))?;
        let name = sys
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: unexpected sysfs entry", crate::ui::shown(shown)))?
            .to_owned();
        if sys.components().any(|part| part.as_os_str() == "virtual") {
            return Err(format!(
                "{}: virtual device (device-mapper, md, loop) is not supported yet, erase the disk or partition under it",
                crate::ui::shown(shown)
            ));
        }
        let partition = sys.join("partition").try_exists().map_err(|err| {
            format!(
                "{}: could not read {}: {err}",
                crate::ui::shown(shown),
                crate::ui::shown(&sys)
            )
        })?;
        Ok(Self {
            shown: shown.to_path_buf(),
            typed,
            path,
            name,
            sys,
            partition,
            rdev: meta.rdev(),
        })
    }

    /// the sysfs directory of the whole disk: the device itself, or the parent of a partition.
    pub(super) fn disk_dir(&self) -> PathBuf {
        if self.partition {
            self.sys
                .parent()
                .map_or_else(|| self.sys.clone(), Path::to_path_buf)
        } else {
            self.sys.clone()
        }
    }

    pub(super) fn disk_name(&self) -> String {
        self.disk_dir()
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }
}

/// the first target that names a device already listed, or a disk together with one of its partitions.
/// two partitions of one disk do not overlap each other.
pub(super) fn overlap_error(targets: &[Target]) -> Option<String> {
    for (index, target) in targets.iter().enumerate() {
        for other in &targets[..index] {
            if target.rdev == other.rdev {
                return Some(format!(
                    "{}: the same device is listed more than once",
                    crate::ui::shown(&target.shown)
                ));
            }
            if target.partition != other.partition && target.disk_name() == other.disk_name() {
                return Some(format!(
                    "{}: the disk or partition overlaps {}",
                    crate::ui::shown(&target.shown),
                    crate::ui::shown(&other.shown)
                ));
            }
        }
    }
    None
}

/// the sysfs directory of the block device `rdev`, resolved through `{sys_root}/dev/block/major:minor`.
/// its `dev` file must name the same numbers, so an entry that does not match is refused, not trusted.
pub(super) fn sys_entry(sys_root: &Path, rdev: u64) -> Result<PathBuf, String> {
    let number = format!("{}:{}", libc::major(rdev), libc::minor(rdev));
    let link = sys_root.join("dev/block").join(&number);
    let sys = fs::canonicalize(&link)
        .map_err(|err| format!("no sysfs entry for device {number}: {err}"))?;
    let dev = read_required(&sys.join("dev"))?;
    if dev != number {
        return Err(format!(
            "the sysfs entry {} is device {dev}, not {number}",
            crate::ui::shown(&sys)
        ));
    }
    Ok(sys)
}

/// node of a device or partition by its kernel name.
pub(super) fn node_of(name: &str) -> PathBuf {
    Path::new("/dev").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, sys: &str, partition: bool, rdev: u64) -> Target {
        Target {
            shown: PathBuf::from(format!("/dev/{name}")),
            typed: format!("/dev/{name}"),
            path: PathBuf::from(format!("/dev/{name}")),
            name: name.to_owned(),
            sys: PathBuf::from(sys),
            partition,
            rdev,
        }
    }

    #[test]
    fn a_disk_and_its_partition_overlap_but_two_partitions_do_not() {
        let disk = target("sdb", "/sys/devices/pci/block/sdb", false, 0x800);
        let part = target("sdb1", "/sys/devices/pci/block/sdb/sdb1", true, 0x801);
        let part2 = target("sdb2", "/sys/devices/pci/block/sdb/sdb2", true, 0x802);
        let other = target("sdc", "/sys/devices/pci/block/sdc", false, 0x820);

        assert!(overlap_error(&[disk, part]).is_some());
        assert!(overlap_error(&[part2, other]).is_none());
        let first = target("sdb", "/sys/devices/pci/block/sdb", false, 0x800);
        let second = target("sdb", "/sys/devices/pci/block/sdb", false, 0x800);
        assert!(overlap_error(&[first, second]).is_some());
    }

    /// a sysfs tree with one disk, `sdb` (8:16), and its partition `sdb1` whose `dev` file says `dev1`.
    fn fake_sysfs(dev1: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let disk = root.path().join("devices/pci/block/sdb");
        fs::create_dir_all(disk.join("sdb1")).unwrap();
        fs::write(disk.join("dev"), "8:16\n").unwrap();
        fs::write(disk.join("sdb1/dev"), format!("{dev1}\n")).unwrap();
        fs::create_dir_all(root.path().join("dev/block")).unwrap();
        std::os::unix::fs::symlink(&disk, root.path().join("dev/block/8:16")).unwrap();
        std::os::unix::fs::symlink(disk.join("sdb1"), root.path().join("dev/block/8:17")).unwrap();
        root
    }

    #[test]
    fn sysfs_entry_is_found_by_major_and_minor() {
        let root = fake_sysfs("8:17");

        let disk = sys_entry(root.path(), libc::makedev(8, 16)).unwrap();
        let part = sys_entry(root.path(), libc::makedev(8, 17)).unwrap();

        assert!(disk.ends_with("devices/pci/block/sdb"));
        assert!(part.ends_with("devices/pci/block/sdb/sdb1"));
    }

    #[test]
    fn sysfs_entry_whose_dev_file_differs_is_refused() {
        let root = fake_sysfs("8:18");

        let err = sys_entry(root.path(), libc::makedev(8, 17)).unwrap_err();

        assert!(err.contains("is device 8:18, not 8:17"), "{err}");
    }

    #[test]
    fn missing_sysfs_entry_is_refused() {
        let root = fake_sysfs("8:17");

        let err = sys_entry(root.path(), libc::makedev(8, 32)).unwrap_err();

        assert!(err.contains("no sysfs entry for device 8:32"), "{err}");
    }
}
