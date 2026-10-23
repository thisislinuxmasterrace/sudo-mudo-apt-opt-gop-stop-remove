//! the mount table and the swap list. the checks read them to find where a device or one of its partitions
//! is in use.

use std::fs;
use std::path::{Path, PathBuf};

use crate::erase;

/// the fields of one line of `/proc/self/mountinfo` that the checks need.
pub(super) struct Mount {
    pub(super) dev: String,
    pub(super) point: PathBuf,
    pub(super) fstype: String,
    pub(super) source: String,
}

pub(super) fn read_mounts() -> Result<Vec<Mount>, String> {
    let table = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|err| format!("could not read the mount table: {err}"))?;
    parse_mountinfo(&table)
}

/// fields 2 (major:minor) and 4 (mount point) are read by position. the optional fields start at field 6 and
/// end at a `-` separator, followed by the filesystem type and the source. empty lines are skipped; a line of
/// any other shape is an error, because skipping it would hide a mount from the checks.
fn parse_mountinfo(table: &str) -> Result<Vec<Mount>, String> {
    let mut mounts = Vec::new();
    for (index, line) in table.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fail = || format!("could not parse the mount table at line {}", index + 1);
        let fields: Vec<&str> = line.split(' ').collect();
        let dash = fields
            .iter()
            .skip(6)
            .position(|field| *field == "-")
            .map(|offset| offset + 6)
            .ok_or_else(fail)?;
        mounts.push(Mount {
            dev: fields.get(2).ok_or_else(fail)?.to_string(),
            point: PathBuf::from(erase::unescape_mountinfo(fields.get(4).ok_or_else(fail)?)),
            fstype: fields.get(dash + 1).ok_or_else(fail)?.to_string(),
            source: erase::unescape_mountinfo(fields.get(dash + 2).ok_or_else(fail)?),
        });
    }
    Ok(mounts)
}

/// the sources of the swap areas, as listed in `/proc/swaps` (the header line is skipped).
pub(super) fn read_swaps() -> Result<Vec<String>, String> {
    let table = fs::read_to_string("/proc/swaps")
        .map_err(|err| format!("could not read /proc/swaps: {err}"))?;
    Ok(table
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(erase::unescape_mountinfo)
        .collect())
}

/// true when a mount source or a swap entry names `node`, as written or after resolving links.
pub(super) fn source_is(source: &str, node: &Path) -> bool {
    if !source.starts_with('/') {
        return false;
    }
    let source = Path::new(source);
    source == node || fs::canonicalize(source).is_ok_and(|real| real.as_path() == node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_fields_are_read_by_position() {
        let table = "36 35 8:17 / /mnt/my\\040disk rw,relatime shared:1 - ext4 /dev/sdb1 rw\n";

        let mounts = parse_mountinfo(table).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].dev, "8:17");
        assert_eq!(mounts[0].point, PathBuf::from("/mnt/my disk"));
        assert_eq!(mounts[0].fstype, "ext4");
        assert_eq!(mounts[0].source, "/dev/sdb1");
    }

    #[test]
    fn empty_mountinfo_lines_are_skipped_and_broken_ones_are_errors() {
        let good = "36 35 8:17 / /mnt rw - ext4 /dev/sdb1 rw\n\n";
        assert_eq!(parse_mountinfo(good).unwrap().len(), 1);

        let broken = "36 35 8:17 / /mnt rw\n";
        let err = parse_mountinfo(broken).err().unwrap();
        assert!(err.contains("line 1"));
    }

    #[test]
    fn a_source_names_a_node_only_when_it_is_that_node() {
        assert!(source_is("/dev/sdb1", Path::new("/dev/sdb1")));
        assert!(!source_is("tmpfs", Path::new("/dev/sdb1")));
        assert!(!source_is("/dev/sdb", Path::new("/dev/sdb1")));
    }
}
