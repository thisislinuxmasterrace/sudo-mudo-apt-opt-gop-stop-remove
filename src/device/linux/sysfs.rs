//! reads of sysfs and of the directories the checks depend on. a read error is never taken for an empty
//! value: it stops the run.

use std::fs;
use std::path::{Path, PathBuf};

use super::SECTOR;

/// the sysfs directory where a device's stacking shows: the device itself, or the disk of a partition.
pub(super) fn sys_disk(sys: &Path) -> Result<PathBuf, String> {
    let partition = sys
        .join("partition")
        .try_exists()
        .map_err(|err| format!("could not read {}: {err}", crate::ui::shown(sys)))?;
    Ok(match sys.parent() {
        Some(parent) if partition => parent.to_path_buf(),
        _ => sys.to_path_buf(),
    })
}

/// a sysfs attribute that the checks depend on. a read error is not an empty value: it stops the run.
pub(super) fn read_required(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|err| format!("could not read {}: {err}", crate::ui::shown(path)))
}

pub(super) fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

pub(super) fn read_u64(path: &Path) -> Result<u64, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", crate::ui::shown(path)))?;
    text.trim()
        .parse()
        .map_err(|err| format!("could not parse {}: {err}", crate::ui::shown(path)))
}

/// a sysfs size is in 512-byte sectors whatever the logical block size is.
pub(super) fn read_sectors(sys: &Path) -> Result<u64, String> {
    read_u64(&sys.join("size"))?
        .checked_mul(SECTOR)
        .ok_or_else(|| format!("size of {} is too large", crate::ui::shown(sys)))
}

/// the names in `dir`. a missing directory is an error too: an empty list would pass a check that was not made.
pub(super) fn read_names(dir: &Path) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|err| format!("could not read {}: {err}", crate::ui::shown(dir)))?;
    entries
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|err| format!("could not read {}: {err}", crate::ui::shown(dir)))
        })
        .collect()
}
