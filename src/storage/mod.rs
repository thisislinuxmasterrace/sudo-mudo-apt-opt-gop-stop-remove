//! advisory checks on the storage behind a path. warnings never block an erase.

mod compression;
mod crypt;
mod flash;
mod fs_name;
#[cfg(target_os = "linux")]
pub(crate) mod mountinfo;
mod space;

use std::path::{Path, PathBuf};

pub(crate) use compression::Compression;
#[cfg(target_os = "linux")]
pub(crate) use compression::detect as assessment_compression;
pub(crate) use crypt::Encryption;
#[cfg(target_os = "linux")]
pub(crate) use crypt::detect as assessment_encryption;
#[cfg(target_os = "linux")]
pub(crate) use crypt::encrypted_file;
pub(crate) use flash::Medium;
use flash::is_flash;
#[cfg(target_os = "linux")]
pub(crate) use flash::medium as assessment_medium;
#[cfg(target_os = "linux")]
pub(crate) use flash::untrusted_reason as untrusted_medium_reason;
#[cfg(any(target_os = "linux", windows))]
pub(crate) use fs_name::filesystem_name as assessment_filesystem;
pub(crate) use fs_name::filesystem_name;
use fs_name::remote_drive;
pub use space::free_bytes;
#[cfg(not(unix))]
pub use space::total_bytes;
#[cfg(unix)]
pub use space::total_bytes_of_open;

/// filesystems that write to new blocks instead of overwriting in place.
/// refs block cloning makes several files share the same clusters. f2fs, nilfs2 and ubifs are
/// log-structured: they write new data elsewhere. an overlay copies a file up from a lower layer
/// before the first write.
const COPY_ON_WRITE: &[&str] = &[
    "apfs", "btrfs", "zfs", "refs", "bcachefs", "f2fs", "nilfs2", "ubifs", "overlay",
];

/// filesystems that live in ram. there are no disk blocks, but old pages may sit in swap.
const MEMORY: &[&str] = &["tmpfs", "ramfs"];

/// local filesystems: overwriting stays on this machine.
const LOCAL: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "jfs", "reiserfs", "squashfs", "iso9660", "cd9660", "cdfs",
    "udf", "vfat", "msdos", "fat", "fat32", "exfat", "ntfs", "ntfs3", "hfs", "hfsplus", "erofs",
    "cramfs", "minix", "ocfs2", "ecryptfs", "ufs",
];

/// network filesystems. overwriting happens on the server, which may keep copies.
const NETWORK: &[&str] = &[
    "nfs",
    "smb",
    "cifs",
    "smb2",
    "smbfs",
    "afpfs",
    "webdav",
    "ceph",
    "glusterfs",
    "9p",
    "afs",
    "coda",
    "lustre",
];

/// how a filesystem type is treated. fuse names (`fuse.sshfs`, `fuseblk`, `macfuse`, ...) and
/// unlisted types both end up as `FuseOrUnknown`: they may send writes to a remote server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Local,
    CopyOnWrite,
    Memory,
    Network,
    FuseOrUnknown,
}

pub(crate) fn classify(fs_name: &str) -> Kind {
    if COPY_ON_WRITE.contains(&fs_name) {
        Kind::CopyOnWrite
    } else if LOCAL.contains(&fs_name) {
        Kind::Local
    } else if MEMORY.contains(&fs_name) {
        Kind::Memory
    } else if NETWORK.contains(&fs_name) {
        Kind::Network
    } else {
        Kind::FuseOrUnknown
    }
}

pub(crate) fn assessment_fs_reason(name: &str) -> Option<&'static str> {
    match classify(name) {
        Kind::Local => None,
        Kind::CopyOnWrite if name == "overlay" => Some(
            "overlay filesystem: a write to a file from a lower layer copies it up first, and the lower copy is never overwritten",
        ),
        Kind::CopyOnWrite => Some("copy-on-write or log-structured filesystem may retain old data"),
        Kind::Memory => Some("memory filesystem: old pages may remain in swap"),
        Kind::Network => Some("network filesystem may retain copies on the server"),
        Kind::FuseOrUnknown => Some("filesystem is fuse or unrecognized"),
    }
}

/// warnings for every path that lives on a copy-on-write, network, fuse or unrecognized filesystem.
pub fn warnings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().flat_map(|path| path_warnings(path)).collect()
}

const REFUSE_NETWORK: &str =
    "a network filesystem, its free space belongs to the server and other users";
const REFUSE_MEMORY: &str = "a memory filesystem (tmpfs or ramfs), it has no disk blocks to wipe and filling it uses ram and swap";
const REFUSE_UNKNOWN: &str = "a fuse, unknown or unreadable filesystem";

/// why filling the free space of a filesystem of this kind is refused, `None` when it is allowed.
/// no flag lifts these: the space belongs to a server or to other users, or is ram.
fn wipe_refusal_for(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Local | Kind::CopyOnWrite => None,
        Kind::Network => Some(REFUSE_NETWORK),
        Kind::Memory => Some(REFUSE_MEMORY),
        Kind::FuseOrUnknown => Some(REFUSE_UNKNOWN),
    }
}

/// why filling the free space of `path` is refused: network filesystems and remote drives, memory
/// filesystems, fuse mounts and types this module cannot classify. erasing there is allowed with a
/// warning; filling the free space is not. unix fills check the opened directory instead
/// (`opened_wipe_refusal`), so this is the path-based check for the other platforms.
#[cfg(not(unix))]
pub fn wipe_refused(path: &Path) -> Option<&'static str> {
    if remote_drive(path) {
        return Some(REFUSE_NETWORK);
    }
    match filesystem_name(path) {
        Some(name) => wipe_refusal_for(classify(&name)),
        None => Some(REFUSE_UNKNOWN),
    }
}

#[cfg(unix)]
pub(crate) fn opened_wipe_refusal(file: &std::fs::File) -> Option<&'static str> {
    match fs_name::opened_filesystem_name(file) {
        Some(name) => wipe_refusal_for(classify(&name)),
        None => Some(REFUSE_UNKNOWN),
    }
}

fn path_warnings(path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    // a link is erased as a link: the storage that matters is the one that holds the link itself.
    let holder = holder_of(path);
    let holder = holder.as_path();
    match filesystem_name(holder) {
        Some(name) => found.extend(fs_warning(path, &name)),
        None => found.push(unreadable_warning(path)),
    }
    if is_flash(holder) {
        found.push(format!(
            "{}: on a flash (non-rotational) device, the device may keep old data in blocks it remapped",
            crate::ui::shown(path)
        ));
    }
    if remote_drive(holder) {
        found.push(format!(
            "{}: remote drive, overwriting happens on the server, which may keep copies",
            crate::ui::shown(path)
        ));
    }
    found
}

/// the path whose storage is described for `path`: the directory that holds a symbolic link, because
/// erasing a link never touches its target (and a dangling target has no filesystem to read).
fn holder_of(path: &Path) -> PathBuf {
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
    }
    path.to_path_buf()
}

/// the type of `path` could not be read: statfs failed, or a fuse mount had no mountinfo entry.
fn unreadable_warning(path: &Path) -> String {
    format!(
        "{}: could not read the filesystem type, it may be a remote one",
        crate::ui::shown(path)
    )
}

fn fs_warning(path: &Path, fs_name: &str) -> Option<String> {
    let message = match classify(fs_name) {
        Kind::Local => return None,
        Kind::CopyOnWrite if fs_name == "overlay" => "overlay filesystem: a write to a file from a lower layer copies it up first, and the lower copy is never overwritten".to_owned(),
        Kind::CopyOnWrite => {
            format!("{fs_name} is copy-on-write or log-structured, old data may survive overwriting")
        }
        Kind::Memory => format!(
            "{fs_name} is a memory filesystem, old pages may remain in swap"
        ),
        Kind::Network => format!(
            "{fs_name} is a network filesystem, overwriting happens on the server, which may keep copies"
        ),
        Kind::FuseOrUnknown => format!(
            "{fs_name} is a fuse or unrecognized filesystem, overwriting may happen on a remote server"
        ),
    };
    Some(format!("{}: {message}", crate::ui::shown(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_copy_on_write_filesystems() {
        for name in [
            "apfs", "btrfs", "zfs", "refs", "bcachefs", "f2fs", "nilfs2", "overlay",
        ] {
            assert_eq!(classify(name), Kind::CopyOnWrite, "{name}");
        }
    }

    #[test]
    fn classifies_local_filesystems() {
        for name in [
            "ext2", "ext4", "xfs", "vfat", "msdos", "fat32", "exfat", "ntfs", "ntfs3", "hfs",
            "hfsplus", "jfs", "reiserfs", "iso9660", "cd9660", "cdfs", "udf", "squashfs", "erofs",
            "cramfs", "minix", "ocfs2", "ecryptfs", "ufs",
        ] {
            assert_eq!(classify(name), Kind::Local, "{name}");
        }
    }

    #[test]
    fn classifies_memory_filesystems() {
        for name in ["tmpfs", "ramfs"] {
            assert_eq!(classify(name), Kind::Memory, "{name}");
        }
    }

    #[test]
    fn memory_and_overlay_get_their_own_wording() {
        let path = Path::new("/run/x");
        assert!(
            fs_warning(path, "tmpfs").is_some_and(|w| w.contains("old pages may remain in swap"))
        );
        assert!(
            fs_warning(path, "overlay")
                .is_some_and(|w| w.contains("lower copy is never overwritten"))
        );
        assert!(assessment_fs_reason("ramfs").is_some_and(|r| r.contains("swap")));
        assert!(assessment_fs_reason("overlay").is_some_and(|r| r.contains("copies it up")));
        assert!(
            assessment_fs_reason("f2fs")
                .is_some_and(|r| r.contains("copy-on-write or log-structured"))
        );
        assert!(
            fs_warning(path, "nilfs2")
                .is_some_and(|w| w.contains("copy-on-write or log-structured"))
        );
    }

    #[test]
    fn wipe_refusals_name_their_reason() {
        assert_eq!(wipe_refusal_for(Kind::Local), None);
        assert_eq!(wipe_refusal_for(Kind::CopyOnWrite), None);
        assert!(wipe_refusal_for(Kind::Network).is_some_and(|r| r.contains("server")));
        assert!(wipe_refusal_for(Kind::Memory).is_some_and(|r| r.contains("tmpfs or ramfs")));
        assert!(wipe_refusal_for(Kind::FuseOrUnknown).is_some_and(|r| r.contains("fuse")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_described_by_its_directory_and_a_dangling_one_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("/nonexistent/target", &link).unwrap();

        assert_eq!(holder_of(&link), dir.path());
        assert!(
            warnings(&[link])
                .iter()
                .all(|w| !w.contains("could not read the filesystem type"))
        );
        assert_eq!(holder_of(Path::new("plain")), Path::new("plain"));
    }

    #[test]
    fn classifies_network_filesystems() {
        for name in [
            "nfs",
            "smb",
            "cifs",
            "smb2",
            "smbfs",
            "afpfs",
            "webdav",
            "ceph",
            "glusterfs",
            "9p",
            "afs",
            "coda",
            "lustre",
        ] {
            assert_eq!(classify(name), Kind::Network, "{name}");
        }
    }

    #[test]
    fn classifies_fuse_as_fuse_or_unknown() {
        for name in [
            "fuse",
            "fuse.sshfs",
            "fuse.rclone",
            "fuseblk",
            "macfuse",
            "osxfuse",
        ] {
            assert_eq!(classify(name), Kind::FuseOrUnknown, "{name}");
        }
    }

    #[test]
    fn classifies_unlisted_filesystems_as_unknown() {
        for name in ["", "weirdfs", "unknown"] {
            assert_eq!(classify(name), Kind::FuseOrUnknown, "{name}");
        }
    }

    #[test]
    fn warns_about_network_and_fuse_but_not_local() {
        let path = Path::new("/mnt/share/file");
        assert!(fs_warning(path, "ceph").is_some_and(|w| w.contains("is a network filesystem")));
        assert!(
            fs_warning(path, "fuse.sshfs")
                .is_some_and(|w| w.contains("is a fuse or unrecognized filesystem"))
        );
        assert!(fs_warning(path, "btrfs").is_some_and(|w| w.contains("copy-on-write")));
        assert!(fs_warning(path, "ext4").is_none());
        assert!(
            unreadable_warning(path)
                .contains("could not read the filesystem type, it may be a remote one")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_device_filesystem_is_refused_for_a_wipe() {
        // /dev is tmpfs-like or devfs: never a filesystem this module lists as safe to fill.
        let dev_null = std::fs::File::open("/dev/null").unwrap();
        assert!(opened_wipe_refusal(&dev_null).is_some());
    }
}
