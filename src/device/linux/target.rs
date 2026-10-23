//! a device named on the command line: the path as typed and as resolved, its sysfs entry, and the checks
//! that need only these names (the same device listed twice, a disk together with its own partition).

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

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
                    shown.display()
                )
            })?
            .to_owned();
        let path = fs::canonicalize(shown).map_err(|err| format!("{}: {err}", shown.display()))?;
        let meta = fs::metadata(&path).map_err(|err| format!("{}: {err}", shown.display()))?;
        if !meta.file_type().is_block_device() {
            return Err(format!("{}: not a block device", shown.display()));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: unexpected device name", shown.display()))?
            .to_owned();
        let sys = fs::canonicalize(Path::new("/sys/class/block").join(&name))
            .map_err(|err| format!("{}: no sysfs entry for {name}: {err}", shown.display()))?;
        if sys.components().any(|part| part.as_os_str() == "virtual") {
            return Err(format!(
                "{}: virtual device (device-mapper, md, loop) is not supported yet, erase the disk or partition under it",
                shown.display()
            ));
        }
        let partition = sys.join("partition").try_exists().map_err(|err| {
            format!(
                "{}: could not read {}: {err}",
                shown.display(),
                sys.display()
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
                    target.shown.display()
                ));
            }
            if target.partition != other.partition && target.disk_name() == other.disk_name() {
                return Some(format!(
                    "{}: the disk or partition overlaps {}",
                    target.shown.display(),
                    other.shown.display()
                ));
            }
        }
    }
    None
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
}
