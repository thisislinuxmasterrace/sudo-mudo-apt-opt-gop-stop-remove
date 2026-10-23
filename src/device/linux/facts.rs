//! what the kernel and the mount table say about a device, and the checks on it: `inspect` reads the facts,
//! `problems` lists the refusals, and `layout_error` says whether the writes can use the device at all.

use std::fs;
use std::path::PathBuf;

use super::mounts::{read_mounts, read_swaps, source_is};
use super::sysfs::{read_names, read_required, read_sectors, read_u64};
use super::target::{Target, node_of};
use super::{CHUNK, SECTOR};

/// what the kernel and the mount table say about a device and, for a whole disk, its partitions.
/// two readings that differ mean the device changed.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Facts {
    /// bytes. zero when there is no medium.
    pub(super) size: u64,
    pub(super) logical_block: u64,
    /// the content of `queue/rotational`, as read: only "1" is a hard disk. an unreadable file stops the run.
    pub(super) rotational: String,
    /// partitions of a whole disk, with their sizes in bytes.
    pub(super) partitions: Vec<(String, u64)>,
    /// (name of the device or partition, mount point).
    pub(super) mounts: Vec<(String, PathBuf)>,
    /// names of the devices or partitions in use as swap.
    pub(super) swap: Vec<String>,
    /// (name of the device or partition, name of the holder, such as `dm-0`).
    pub(super) holders: Vec<(String, String)>,
    /// true when btrfs or zfs is mounted anywhere on the system: a multi-device filesystem may use this
    /// device, and its other members are not all in the mount table.
    pub(super) multi_device_fs: bool,
}

impl Facts {
    /// reasons to refuse the device unless `--i-know-what-i-am-doing` is given. each one completes
    /// the sentence "<path>: <reason>, refusing to erase it".
    pub(super) fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        problems.extend(
            self.mounts
                .iter()
                .map(|(name, point)| format!("/dev/{name} is mounted at {}", point.display())),
        );
        problems.extend(
            self.swap
                .iter()
                .map(|name| format!("/dev/{name} is in use as swap")),
        );
        problems.extend(
            self.holders
                .iter()
                .map(|(name, holder)| format!("/dev/{name} is in use by {holder}")),
        );
        if self.multi_device_fs {
            problems.push(
                "btrfs or zfs is mounted on this system; a multi-device filesystem may use this device and its members are not visible in the mount table"
                    .to_owned(),
            );
        }
        if self.rotational != "1" {
            problems.push(format!(
                "media is not known to be a hard disk (queue/rotational is {}), overwriting flash is not reliable",
                self.rotational
            ));
        }
        problems
    }
}

/// reads what the device looks like now. errors here are not refusals: the state cannot be trusted.
pub(super) fn inspect(target: &Target) -> Result<Facts, String> {
    let disk = target.disk_dir();
    let size = read_sectors(&target.sys)?;
    let logical_block = read_u64(&disk.join("queue/logical_block_size"))?;
    let rotational = read_required(&disk.join("queue/rotational"))?;

    // each member is checked for mounts, swap and holders: the device itself, then for a whole disk its
    // partitions, and for a partition its disk, whose holders and mounts use the partition's blocks too.
    let mut members = vec![(target.name.clone(), target.sys.clone())];
    let mut partitions = Vec::new();
    if target.partition {
        members.push((target.disk_name(), target.disk_dir()));
    } else {
        let entries = fs::read_dir(&disk)
            .map_err(|err| format!("could not read {}: {err}", disk.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| format!("could not read {}: {err}", disk.display()))?;
            let sys = entry.path();
            let is_partition = sys
                .join("partition")
                .try_exists()
                .map_err(|err| format!("could not read {}: {err}", sys.display()))?;
            if !is_partition {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            partitions.push((name.clone(), read_sectors(&sys)?));
            members.push((name, sys));
        }
    }
    partitions.sort();

    let table = read_mounts()?;
    let multi_device_fs = table
        .iter()
        .any(|mount| matches!(mount.fstype.as_str(), "btrfs" | "zfs"));
    let swaps = read_swaps()?;
    let mut mounts = Vec::new();
    let mut swap = Vec::new();
    let mut holders = Vec::new();
    for (name, sys) in &members {
        let node = node_of(name);
        let dev = read_required(&sys.join("dev"))?;
        mounts.extend(
            table
                .iter()
                .filter(|mount| dev == mount.dev || source_is(&mount.source, &node))
                .map(|mount| (name.clone(), mount.point.clone())),
        );
        if swaps.iter().any(|source| source_is(source, &node)) {
            swap.push(name.clone());
        }
        holders.extend(
            read_names(&sys.join("holders"))?
                .into_iter()
                .map(|holder| (name.clone(), holder)),
        );
    }
    mounts.sort();
    swap.sort();
    holders.sort();

    Ok(Facts {
        size,
        logical_block,
        rotational,
        partitions,
        mounts,
        swap,
        holders,
        multi_device_fs,
    })
}

/// a device with no medium, and a layout the writes cannot use: a power of two from 512 bytes up to one
/// chunk, and a size that is a multiple of it. `--i-know-what-i-am-doing` does not lift these.
pub(super) fn layout_error(facts: &Facts) -> Option<String> {
    if facts.size == 0 {
        return Some("there is no medium (size is zero)".to_owned());
    }
    let block = facts.logical_block;
    if !block.is_power_of_two() || !(SECTOR..=CHUNK as u64).contains(&block) {
        return Some(format!(
            "logical block size {block} is not supported (a power of two from 512 b to 1 mib)"
        ));
    }
    if !facts.size.is_multiple_of(block) {
        return Some(format!(
            "size {} is not a multiple of the logical block size {block}",
            facts.size
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            size: 1 << 30,
            logical_block: 512,
            rotational: "1".to_owned(),
            partitions: Vec::new(),
            mounts: Vec::new(),
            swap: Vec::new(),
            holders: Vec::new(),
            multi_device_fs: false,
        }
    }

    #[test]
    fn a_clean_hard_disk_has_no_problems() {
        assert!(facts().problems().is_empty());
    }

    #[test]
    fn mounts_swap_and_holders_are_refusals() {
        let mut busy = facts();
        busy.mounts
            .push(("sdb1".to_owned(), PathBuf::from("/mnt/x")));
        busy.swap.push("sdb2".to_owned());
        busy.holders.push(("sdb".to_owned(), "dm-0".to_owned()));

        assert_eq!(busy.problems().len(), 3);
    }

    #[test]
    fn btrfs_or_zfs_on_the_system_is_a_refusal() {
        let shared = Facts {
            multi_device_fs: true,
            ..facts()
        };

        assert_eq!(shared.problems().len(), 1);
    }

    #[test]
    fn media_that_is_not_a_hard_disk_is_a_refusal() {
        let flash = Facts {
            rotational: "0".to_owned(),
            ..facts()
        };
        let unknown = Facts {
            rotational: "unknown".to_owned(),
            ..facts()
        };

        assert_eq!(flash.problems().len(), 1);
        assert_eq!(unknown.problems().len(), 1);
    }

    #[test]
    fn zero_size_is_a_layout_error_not_a_problem() {
        let empty = Facts { size: 0, ..facts() };

        assert!(empty.problems().is_empty());
        assert!(layout_error(&empty).is_some());
    }

    #[test]
    fn layout_needs_whole_logical_blocks() {
        assert!(layout_error(&facts()).is_none());
        let odd = Facts {
            size: 1000,
            ..facts()
        };
        let bad_block = Facts {
            logical_block: 3000,
            ..facts()
        };

        assert!(layout_error(&odd).is_some());
        assert!(layout_error(&bad_block).is_some());
    }
}
