//! optional snapshot evidence for top-level explanations. no match never proves absence of copies.

use std::path::Path;
use std::process::Command;

fn read(command: &mut Command) -> Option<String> {
    let (status, bytes) = crate::advisory::output(command)?;
    status
        .success()
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

pub(crate) fn explain(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if (!meta.is_file() && !meta.is_dir()) || meta.file_type().is_symlink() {
        return None;
    }
    let filesystem = crate::storage::filesystem_name(path)?;
    let result = match filesystem.as_str() {
        "btrfs" => btrfs(path),
        "zfs" => zfs(path),
        "apfs" => Some("snapshot check unknown: apfs/time machine needs a native volume-specific check".to_owned()),
        _ => return None,
    }.unwrap_or_else(|| "snapshot check unknown: the tool is unavailable, timed out or did not identify this target's storage".to_owned());
    Some(format!(
        "  - {}: {result}. snapshot contents and other backups are not checked; no matches do not prove absence of copies.",
        path.display()
    ))
}

fn btrfs(path: &Path) -> Option<String> {
    let path = std::path::absolute(path).ok()?;
    let id: u64 = read(
        Command::new("btrfs")
            .args(["inspect-internal", "rootid"])
            .arg(&path),
    )?
    .trim()
    .parse()
    .ok()?;
    let output = read(
        Command::new("btrfs")
            .args(["subvolume", "list", "-a", "-u", "-q"])
            .arg(path),
    )?;
    btrfs_result(id, &output)
}

fn btrfs_result(id: u64, output: &str) -> Option<String> {
    let fields: Vec<Vec<&str>> = output
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    let value = |fields: &[&str], key: &str| {
        fields
            .windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1].to_owned())
    };
    let source = fields
        .iter()
        .find(|row| value(row, "ID").as_deref().and_then(|s| s.parse().ok()) == Some(id))?;
    let uuid = value(source, "uuid")?;
    if uuid.len() != 36 || uuid == "-" {
        return None;
    }
    let count = fields
        .iter()
        .filter(|row| value(row, "parent_uuid").as_deref() == Some(&uuid))
        .count();
    Some(if count > 0 {
        format!("{count} direct snapshots found for this btrfs subvolume")
    } else {
        "no direct snapshots found for this btrfs subvolume; older snapshot ancestry remains unknown".to_owned()
    })
}

fn zfs(path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path).ok()?;
    let mounts = read(Command::new("zfs").args([
        "list",
        "-H",
        "-o",
        "name,mountpoint",
        "-t",
        "filesystem",
    ]))?;
    let dataset = mounts
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, mount)| Path::new(mount).is_absolute() && path.starts_with(mount))
        .max_by_key(|(_, mount)| mount.len())?
        .0;
    let snapshots = read(
        Command::new("zfs").args(["list", "-H", "-t", "snapshot", "-o", "name", "-r", dataset]),
    )?;
    let count = snapshots
        .lines()
        .filter(|line| {
            line.split_once('@')
                .is_some_and(|(name, _)| name == dataset)
        })
        .count();
    Some(if count > 0 {
        format!("{count} snapshots found for zfs dataset {dataset}")
    } else {
        format!("no snapshots found for zfs dataset {dataset}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_bound_to_source_uuid_not_nearby_subvolumes() {
        let source = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let output = format!(
            "ID 256 gen 1 parent 5 top level 5 uuid {source} parent_uuid - path live\nID 257 gen 2 parent 5 top level 5 uuid bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb parent_uuid {source} path snap one\nID 258 gen 2 parent 5 top level 5 uuid cccccccc-cccc-cccc-cccc-cccccccccccc parent_uuid dddddddd-dddd-dddd-dddd-dddddddddddd path unrelated\n"
        );
        assert_eq!(
            btrfs_result(256, &output).unwrap(),
            "1 direct snapshots found for this btrfs subvolume"
        );
        assert!(btrfs_result(5, &output).is_none());
        assert!(btrfs_result(256, "unrecognized output").is_none());
    }
}
