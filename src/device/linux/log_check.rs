//! the `--log` file of a device run: it must not be on the device being erased, and the filesystem that
//! holds it must be a plain block device.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use crate::log;

use super::facts::Facts;
use super::mounts::read_mounts;
use super::sysfs::{read_names, sys_disk};
use super::target::{Target, node_of};

/// checks the `--log` path before the confirmation, so a refusal costs no question. the log must be a new
/// file, and it must not be on the device or on one of its partitions. the filesystem that holds the log is
/// the mount with the longest mount point above the log's directory (the last listed, if several share it).
/// its device number is not used: btrfs and zfs report an anonymous one. when that filesystem cannot be
/// tied to one plain block device, the run is refused.
pub(super) fn check_log(
    log: &Path,
    paths: &[PathBuf],
    targets: &[Target],
    states: &[Facts],
) -> Result<(), String> {
    log::check_path(log, paths)?;
    check_log_location(log, targets, states)
}

/// the location part of `check_log`, which is also run once the log exists: `check_path` refuses an existing
/// file, so it is not repeated then.
pub(super) fn check_log_location(
    log: &Path,
    targets: &[Target],
    states: &[Facts],
) -> Result<(), String> {
    let shown = log.display();
    let dir = match log.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let dir = fs::canonicalize(dir).map_err(|err| format!("{shown}: {err}"))?;
    let table = read_mounts()?;
    let mount = table
        .iter()
        .filter(|mount| dir.starts_with(&mount.point))
        .max_by_key(|mount| mount.point.components().count())
        .ok_or_else(|| {
            format!("{shown}: could not tell which filesystem holds the log, choose another file")
        })?;
    if !mount.source.starts_with('/') {
        return Err(format!(
            "{shown}: the log is on {}, which is not a block device: choose a log on a disk or partition",
            mount.source
        ));
    }
    let real = fs::canonicalize(&mount.source).map_err(|err| format!("{shown}: {err}"))?;
    let meta = fs::metadata(&real).map_err(|err| format!("{shown}: {err}"))?;
    if !meta.file_type().is_block_device() {
        return Err(format!(
            "{shown}: the log is on {}, which is not a block device: choose a log on a disk or partition",
            real.display()
        ));
    }
    for (target, facts) in targets.iter().zip(states) {
        if nodes_of(target, facts).contains(&real) {
            return Err(format!(
                "{shown}: the log would be on {}, choose another file",
                target.shown.display()
            ));
        }
    }
    let name = real
        .file_name()
        .ok_or_else(|| format!("{shown}: unexpected log device {}", real.display()))?;
    let sys = fs::canonicalize(Path::new("/sys/class/block").join(name))
        .map_err(|err| format!("{shown}: {err}"))?;
    if !read_names(&sys_disk(&sys)?.join("slaves"))?.is_empty() {
        return Err(format!(
            "{shown}: the log is on {}, which is built on other devices (device-mapper, md): choose another file",
            real.display()
        ));
    }
    Ok(())
}

/// the nodes that make up a target: the device itself and, for a whole disk, its partitions. for a
/// partition, the disk it is part of.
fn nodes_of(target: &Target, facts: &Facts) -> Vec<PathBuf> {
    let mut nodes = vec![target.path.clone()];
    if target.partition {
        nodes.push(node_of(&target.disk_name()));
    } else {
        nodes.extend(facts.partitions.iter().map(|(name, _)| node_of(name)));
    }
    nodes
}
