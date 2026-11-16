//! optional snapshot evidence for top-level explanations. no match never proves absence of copies.

use std::path::Path;
use std::process::Command;

use crate::advisory::Failure;

/// what a check found, or why it found nothing.
type Found = Result<String, Failure>;

fn read(command: &mut Command) -> Found {
    let (status, bytes) = crate::advisory::checked_output(command)?;
    if !status.success() {
        return Err(Failure::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| Failure::Unavailable)
}

pub(crate) fn explain(path: &Path) -> Option<String> {
    // a symbolic link is neither a file nor a directory here, so it is left out too.
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() && !meta.is_dir() {
        return None;
    }
    let filesystem = crate::storage::filesystem_name(path)?;
    let result = match filesystem.as_str() {
        "btrfs" => btrfs(path),
        "zfs" => zfs(path),
        "apfs" => Ok("apfs and time machine snapshots are not checked; `tmutil listlocalsnapshots /` lists local ones".to_owned()),
        _ => return None,
    }.unwrap_or_else(|failure| match failure {
        Failure::TooLarge => "could not check: output too large".to_owned(),
        Failure::Unavailable => "could not check: the tool is unavailable, timed out or did not identify the storage".to_owned(),
    });
    Some(format!(
        "  - snapshots: {}: {result}. other backups are not checked.",
        crate::ui::shown(path)
    ))
}

fn btrfs(path: &Path) -> Found {
    btrfs_using(path, std::ffi::OsStr::new("btrfs"))
}

fn btrfs_using(path: &Path, tool: &std::ffi::OsStr) -> Found {
    let path = std::path::absolute(path).map_err(|_| Failure::Unavailable)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|_| Failure::Unavailable)?;
    let directory = if meta.is_dir() {
        path.as_path()
    } else if meta.is_file() {
        path.parent().ok_or(Failure::Unavailable)?
    } else {
        return Err(Failure::Unavailable);
    };
    let id: u64 = read(
        Command::new(tool)
            .args(["inspect-internal", "rootid"])
            .arg(&path),
    )?
    .trim()
    .parse()
    .map_err(|_| Failure::Unavailable)?;
    let output = read(
        Command::new(tool)
            .args(["subvolume", "list", "-a", "-u", "-q"])
            .arg(directory),
    )?;
    btrfs_result(id, &output).ok_or(Failure::Unavailable)
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
        format!(
            "{} of its btrfs subvolume may keep older versions; remove them first",
            crate::ui::count(count as u64, "direct snapshot", "direct snapshots")
        )
    } else {
        "no direct snapshots of its btrfs subvolume found; older snapshot ancestry is unknown"
            .to_owned()
    })
}

fn zfs(path: &Path) -> Found {
    let path = std::fs::canonicalize(path).map_err(|_| Failure::Unavailable)?;
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
        .max_by_key(|(_, mount)| mount.len())
        .ok_or(Failure::Unavailable)?
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
    Ok(if count > 0 {
        format!(
            "{} of zfs dataset {dataset} may keep older versions; remove them first",
            crate::ui::count(count as u64, "snapshot", "snapshots")
        )
    } else {
        format!("no snapshots of zfs dataset {dataset} found")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn file_target_uses_containing_directory_for_listing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("btrfs");
        std::fs::write(&tool, concat!(
            "#!/bin/sh\n",
            "if [ \"$1\" = inspect-internal ]; then printf '256\\n'; exit 0; fi\n",
            "[ -d \"$6\" ] || exit 1\n",
            "printf 'ID 256 gen 7 top level 5 parent_uuid - uuid aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa path live\\n'\n",
            "printf 'ID 258 gen 7 top level 5 parent_uuid aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa uuid bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb path snapshot\\n'\n",
        )).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"contents").unwrap();
        // a fork in a parallel test can briefly hold the script open for writing (ETXTBSY), which
        // looks like an unavailable tool; a persistent failure still fails the assertion.
        let using = |target: &Path| {
            for _ in 0..50 {
                match btrfs_using(target, tool.as_os_str()) {
                    Err(Failure::Unavailable) => {
                        std::thread::sleep(std::time::Duration::from_millis(20))
                    }
                    found => return found,
                }
            }
            btrfs_using(target, tool.as_os_str())
        };
        for target in [&path, &dir.path().to_path_buf()] {
            assert_eq!(
                using(target).as_deref(),
                Ok(
                    "1 direct snapshot of its btrfs subvolume may keep older versions; remove them first"
                )
            );
        }
        assert_eq!(std::fs::read(path).unwrap(), b"contents");
    }

    #[cfg(unix)]
    #[test]
    fn oversized_tool_output_is_reported_as_too_large() {
        // no script file: running a freshly written executable races with forks in parallel tests (ETXTBSY).
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read(Command::new("sh").args(["-c", "head -c 70000 /dev/zero"])),
            Err(Failure::TooLarge)
        );
        assert_eq!(
            btrfs_using(dir.path(), std::ffi::OsStr::new("mudo-missing-tool")),
            Err(Failure::Unavailable)
        );
    }

    #[test]
    fn snapshots_are_bound_to_source_uuid_not_nearby_subvolumes() {
        let source = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let output = format!(
            "ID 256 gen 1 parent 5 top level 5 uuid {source} parent_uuid - path live\nID 257 gen 2 parent 5 top level 5 uuid bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb parent_uuid {source} path snap one\nID 258 gen 2 parent 5 top level 5 uuid cccccccc-cccc-cccc-cccc-cccccccccccc parent_uuid dddddddd-dddd-dddd-dddd-dddddddddddd path unrelated\n"
        );
        assert_eq!(
            btrfs_result(256, &output).unwrap(),
            "1 direct snapshot of its btrfs subvolume may keep older versions; remove them first"
        );
        assert!(btrfs_result(5, &output).is_none());
        assert!(btrfs_result(256, "unrecognized output").is_none());
    }
}
