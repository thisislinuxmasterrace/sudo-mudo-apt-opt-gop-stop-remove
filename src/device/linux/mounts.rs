//! the mount table and the swap list. the checks read them to find where a device or one of its partitions
//! is in use.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::erase;
use crate::storage::mountinfo;

/// the fields of one line of `/proc/self/mountinfo` that the checks need.
pub(super) struct Mount {
    pub(super) dev: String,
    pub(super) point: PathBuf,
    pub(super) fstype: String,
    pub(super) source: PathBuf,
}

pub(super) fn read_mounts() -> Result<Vec<Mount>, String> {
    let table =
        mountinfo::read_table().map_err(|err| format!("could not read the mount table: {err}"))?;
    parse_mountinfo(&table)
}

/// every line of the table must parse: skipping one would hide a mount from the checks.
fn parse_mountinfo(table: &[u8]) -> Result<Vec<Mount>, String> {
    mountinfo::parse(table)
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| Mount {
                    dev: entry.dev,
                    point: entry.point,
                    fstype: entry.fs_type,
                    source: entry.source,
                })
                .collect()
        })
        .map_err(|line| format!("could not parse the mount table at line {line}"))
}

/// the sources of the swap areas, as listed in `/proc/swaps` (the header line is skipped).
pub(super) fn read_swaps() -> Result<Vec<PathBuf>, String> {
    let table =
        fs::read("/proc/swaps").map_err(|err| format!("could not read /proc/swaps: {err}"))?;
    Ok(parse_swaps(&table))
}

fn parse_swaps(table: &[u8]) -> Vec<PathBuf> {
    table
        .split(|&byte| byte == b'\n')
        .skip(1)
        .filter_map(|line| {
            line.split(|byte| byte.is_ascii_whitespace())
                .find(|field| !field.is_empty())
        })
        .map(|name| PathBuf::from(OsString::from_vec(erase::unescape_mountinfo(name))))
        .collect()
}

/// true when a mount source or a swap entry names `node`, as written or after resolving links.
pub(super) fn source_is(source: &Path, node: &Path) -> bool {
    if !source.is_absolute() {
        return false;
    }
    source == node || fs::canonicalize(source).is_ok_and(|real| real.as_path() == node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_fields_are_read_by_position() {
        let table = b"36 35 8:17 / /mnt/my\\040disk rw,relatime shared:1 - ext4 /dev/sdb1 rw\n";

        let mounts = parse_mountinfo(table).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].dev, "8:17");
        assert_eq!(mounts[0].point, PathBuf::from("/mnt/my disk"));
        assert_eq!(mounts[0].fstype, "ext4");
        assert_eq!(mounts[0].source, PathBuf::from("/dev/sdb1"));
    }

    #[test]
    fn empty_mountinfo_lines_are_skipped_and_broken_ones_are_errors() {
        let good = b"36 35 8:17 / /mnt rw - ext4 /dev/sdb1 rw\n\n";
        assert_eq!(parse_mountinfo(good).unwrap().len(), 1);

        let broken = b"36 35 8:17 / /mnt rw\n";
        let err = parse_mountinfo(broken).err().unwrap();
        assert!(err.contains("line 1"));
    }

    #[test]
    fn swap_entries_are_decoded_and_the_header_is_skipped() {
        let table = b"Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n\
            /dev/sdb2                               partition\t1048572\t\t0\t\t-2\n\
            /swap\\040file\t\t\t\tfile\t\t2097148\t\t0\t\t-3\n";

        assert_eq!(
            parse_swaps(table),
            [PathBuf::from("/dev/sdb2"), PathBuf::from("/swap file")]
        );
    }

    #[test]
    fn a_source_names_a_node_only_when_it_is_that_node() {
        let node = Path::new("/dev/sdb1");
        assert!(source_is(Path::new("/dev/sdb1"), node));
        assert!(!source_is(Path::new("tmpfs"), node));
        assert!(!source_is(Path::new("/dev/sdb"), node));
    }
}
