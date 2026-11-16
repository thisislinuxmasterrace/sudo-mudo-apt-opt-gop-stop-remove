//! the one parser of `/proc/self/mountinfo`, shared by the erase walk, the storage lookups and the device
//! checks. it works on bytes: a path that is not utf-8 is kept as it is, and the kernel's octal escapes
//! (`\040` and the like) are decoded only here.

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

// the decoder lives with the erase walk, which re-exports it for the whole crate.
use crate::erase::unescape_mountinfo as unescape;

/// the fields of one mountinfo line that the callers need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// field 3, `major:minor`.
    pub(crate) dev: String,
    /// field 5.
    pub(crate) point: PathBuf,
    pub(crate) fs_type: String,
    pub(crate) source: PathBuf,
    /// the comma separated super options, empty when the line has none.
    pub(crate) options: String,
}

/// the whole table as bytes.
pub(crate) fn read_table() -> io::Result<Vec<u8>> {
    std::fs::read("/proc/self/mountinfo")
}

fn lines(table: &[u8]) -> impl Iterator<Item = &[u8]> {
    table.split(|&byte| byte == b'\n')
}

fn fields(line: &[u8]) -> Vec<&[u8]> {
    line.split(|&byte| byte == b' ').collect()
}

fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

fn path(field: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(unescape(field)))
}

/// the mount point of a line, taken by position. it needs no separator, so a damaged line still
/// reports the point it names.
pub(crate) fn mount_point(line: &[u8]) -> Option<PathBuf> {
    line.split(|&byte| byte == b' ').nth(4).map(path)
}

/// the mount points of every line of the table.
pub(crate) fn mount_points(table: &[u8]) -> Vec<PathBuf> {
    lines(table).filter_map(mount_point).collect()
}

/// fields 3 (major:minor) and 5 (mount point) are read by position. the optional fields start at field 7
/// and end at a `-` separator, followed by the filesystem type, the source and the super options.
/// `None` for a line that does not have this shape.
pub(crate) fn parse_line(line: &[u8]) -> Option<Entry> {
    let fields = fields(line);
    let dash = fields
        .iter()
        .skip(6)
        .position(|field| *field == b"-")
        .map(|offset| offset + 6)?;
    Some(Entry {
        dev: text(fields.get(2)?),
        point: path(fields.get(4)?),
        fs_type: text(fields.get(dash + 1)?),
        source: path(fields.get(dash + 2)?),
        options: fields
            .get(dash + 3)
            .map(|field| text(field))
            .unwrap_or_default(),
    })
}

/// every entry of the table. empty lines are skipped; a line of any other shape is an error naming its
/// number, because skipping it would hide a mount from the checks.
pub(crate) fn parse(table: &[u8]) -> Result<Vec<Entry>, usize> {
    let mut entries = Vec::new();
    for (index, line) in lines(table).enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        entries.push(parse_line(line).ok_or(index + 1)?);
    }
    Ok(entries)
}

/// the entry of the longest mount point that contains `path`. a later line on the same mount point is
/// stacked on top of an earlier one, so it wins the tie. lines that do not parse are skipped: this is a
/// lookup for one path, not a check that must see every mount.
pub(crate) fn entry_for(table: &[u8], path: &std::path::Path) -> Option<Entry> {
    let mut best: Option<Entry> = None;
    for entry in lines(table).filter_map(parse_line) {
        if path.starts_with(&entry.point)
            && best
                .as_ref()
                .is_none_or(|b| entry.point.as_os_str().len() >= b.point.as_os_str().len())
        {
            best = Some(entry);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[test]
    fn fields_are_read_by_position() {
        let line = b"36 35 8:17 / /mnt/my\\040disk rw,relatime shared:1 - ext4 /dev/sdb1 rw,x";
        let entry = parse_line(line).unwrap();
        assert_eq!(entry.dev, "8:17");
        assert_eq!(entry.point, Path::new("/mnt/my disk"));
        assert_eq!(entry.fs_type, "ext4");
        assert_eq!(entry.source, Path::new("/dev/sdb1"));
        assert_eq!(entry.options, "rw,x");
        let bare = parse_line(b"37 36 0:33 / /mnt rw - ext4 /dev/vdc1").unwrap();
        assert_eq!(bare.options, "");
    }

    #[test]
    fn empty_lines_are_skipped_and_broken_ones_are_errors() {
        assert_eq!(
            parse(b"36 35 8:17 / /mnt rw - ext4 /dev/sdb1 rw\n\n")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse(b"36 35 8:17 / /mnt rw\n").unwrap_err(), 1);
    }

    #[test]
    fn a_path_that_is_not_utf8_is_kept_as_bytes() {
        let table = b"22 1 0:21 / / rw - ext4 /dev/sda1 rw\n\
            36 22 0:30 / /mnt/a\xffb\\040c rw - ext4 /dev/sdb1 rw\n";

        assert_eq!(
            mount_points(table),
            [
                PathBuf::from("/"),
                PathBuf::from(OsStr::from_bytes(b"/mnt/a\xffb c")),
            ]
        );
        assert_eq!(
            entry_for(table, Path::new(OsStr::from_bytes(b"/mnt/a\xffb c/x")))
                .unwrap()
                .source,
            Path::new("/dev/sdb1")
        );
    }

    #[test]
    fn a_damaged_line_still_names_its_mount_point_but_has_no_entry() {
        assert_eq!(
            mount_points(b"1 2 3 / /broken rw"),
            [PathBuf::from("/broken")]
        );
        assert!(parse_line(b"1 2 3 / /broken rw").is_none());
    }

    #[test]
    fn the_longest_mount_point_wins_and_a_later_line_wins_a_tie() {
        let table = b"36 25 0:32 / / rw - btrfs /dev/vdb1 rw\n\
            37 36 0:33 / /home/with\\040space rw - tmpfs tmpfs rw\n\
            38 36 0:34 / /home rw - ext4 /dev/vdc1 rw\n\
            40 25 0:30 / /r rw - ext4 /dev/vdd1 rw\n\
            41 25 0:40 / /r rw - fuse.sshfs user@host:/ rw\n";
        let source = |p: &str| entry_for(table, Path::new(p)).map(|e| e.source);
        assert_eq!(source("/usr/bin"), Some("/dev/vdb1".into()));
        assert_eq!(source("/home/a/file"), Some("/dev/vdc1".into()));
        assert_eq!(source("/home/with space/file"), Some("tmpfs".into()));
        assert_eq!(source("/r/a"), Some("user@host:/".into()));
    }
}
