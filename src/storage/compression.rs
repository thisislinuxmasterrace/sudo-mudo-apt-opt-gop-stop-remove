//! transparent compression facts for the assessment. a filesystem that compresses can store new data
//! in smaller extents than the old data, so an overwrite does not have to reach the old blocks.
//! only what the mount line shows is used, and anything unclear stays `Unknown`.

#[cfg(target_os = "linux")]
use std::path::Path;

// only linux detects compression; elsewhere the other variants exist for the shared reports.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compression {
    /// not looked at, or the filesystem can compress and nothing shows whether it does.
    Unknown,
    /// the filesystem has no transparent compression at all.
    NotSupported,
    /// the mount options turn compression on, for some or all files.
    Enabled,
}

impl Compression {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::NotSupported => "not_supported",
            Self::Enabled => "enabled",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::Unknown => "compression and snapshots are unknown",
            Self::NotSupported => {
                "the filesystem has no transparent compression; snapshots are unknown"
            }
            Self::Enabled => {
                "transparent compression is indicated for this object or mount, so new data may land in smaller extents than the old data; snapshots are unknown"
            }
        }
    }
}

/// filesystems without any transparent compression feature.
#[cfg(target_os = "linux")]
const NO_COMPRESSION: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "vfat", "msdos", "exfat", "jfs", "reiserfs", "minix",
];

/// what is known about compression under `path`, whose filesystem name is `filesystem`.
#[cfg(target_os = "linux")]
pub(crate) fn detect(path: &Path, filesystem: Option<&str>) -> Compression {
    let Some(name) = filesystem else {
        return Compression::Unknown;
    };
    if NO_COMPRESSION.contains(&name) {
        return Compression::NotSupported;
    }
    match super::fs_name::mount_super_options(path) {
        Some(options) => from_options(name, &options),
        None => Compression::Unknown,
    }
}

/// compression the mount options of `fs_name` turn on. a missing option proves nothing: btrfs can also
/// compress through a per-file or per-directory property. f2fs is left out on purpose: it prints its
/// algorithm option whether or not any file is selected for compression, so the line shows nothing.
#[cfg(target_os = "linux")]
fn from_options(fs_name: &str, options: &str) -> Compression {
    let on = |option: &str| match fs_name {
        "btrfs" => {
            option == "compress"
                || option
                    .strip_prefix("compress=")
                    .or_else(|| option.strip_prefix("compress-force="))
                    .is_some_and(|algorithm| !matches!(algorithm, "no" | "none"))
        }
        _ => false,
    };
    if options.split(',').any(on) {
        Compression::Enabled
    } else {
        Compression::Unknown
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn btrfs_compress_options_turn_it_on() {
        for options in [
            "rw,relatime,compress=zstd:3,ssd",
            "rw,compress-force=lzo",
            "rw,compress",
        ] {
            assert_eq!(
                from_options("btrfs", options),
                Compression::Enabled,
                "{options}"
            );
        }
    }

    #[test]
    fn a_missing_option_proves_nothing() {
        assert_eq!(
            from_options("btrfs", "rw,relatime,ssd,space_cache=v2"),
            Compression::Unknown
        );
        assert_eq!(
            from_options("btrfs", "rw,nocompress_hint"),
            Compression::Unknown
        );
        assert_eq!(
            from_options("zfs", "rw,compress=zstd"),
            Compression::Unknown
        );
    }

    #[test]
    fn f2fs_and_disabled_btrfs_options_prove_nothing() {
        assert_eq!(
            from_options(
                "f2fs",
                "rw,compress_algorithm=zstd:6,compress_extension=txt"
            ),
            Compression::Unknown
        );
        assert_eq!(
            from_options("btrfs", "rw,compress=no"),
            Compression::Unknown
        );
        assert_eq!(
            from_options("btrfs", "rw,compress=none"),
            Compression::Unknown
        );
    }

    #[test]
    fn filesystems_without_compression_are_told_by_name() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path(), Some("ext4")), Compression::NotSupported);
        assert_eq!(detect(root.path(), None), Compression::Unknown);
    }
}
