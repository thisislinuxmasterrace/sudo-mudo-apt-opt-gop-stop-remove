//! advisory checks on the storage behind a path. warnings never block an erase.

mod flash;
mod fs_name;
mod space;

use std::path::{Path, PathBuf};

use flash::is_flash;
use fs_name::{filesystem_name, remote_drive};
pub use space::available_bytes;

/// filesystems that write to new blocks instead of overwriting in place.
/// refs block cloning makes several files share the same clusters.
const COPY_ON_WRITE: &[&str] = &["apfs", "btrfs", "zfs", "refs", "bcachefs"];

/// local filesystems: overwriting stays on this machine.
const LOCAL: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "f2fs", "tmpfs", "ramfs", "overlay", "jfs", "reiserfs",
    "squashfs", "iso9660", "cd9660", "cdfs", "udf", "vfat", "msdos", "fat", "fat32", "exfat",
    "ntfs", "ntfs3", "hfs", "hfsplus", "erofs", "nilfs2", "cramfs", "minix", "ocfs2", "ecryptfs",
    "ufs",
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
enum Kind {
    Local,
    CopyOnWrite,
    Network,
    FuseOrUnknown,
}

fn classify(fs_name: &str) -> Kind {
    if COPY_ON_WRITE.contains(&fs_name) {
        Kind::CopyOnWrite
    } else if LOCAL.contains(&fs_name) {
        Kind::Local
    } else if NETWORK.contains(&fs_name) {
        Kind::Network
    } else {
        Kind::FuseOrUnknown
    }
}

/// warnings for every path that lives on a copy-on-write, network, fuse or unrecognized filesystem.
pub fn warnings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().flat_map(|path| path_warnings(path)).collect()
}

/// true when filling the free space of `path` would write to a server, or to a filesystem this
/// module cannot classify: network filesystems, fuse mounts, unlisted types and remote drives.
/// erasing there is allowed with a warning; filling its free space is not, because that space
/// belongs to the server and other users.
pub fn is_network(path: &Path) -> bool {
    filesystem_name(path)
        .is_none_or(|name| matches!(classify(&name), Kind::Network | Kind::FuseOrUnknown))
        || remote_drive(path)
}

fn path_warnings(path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    match filesystem_name(path) {
        Some(name) => found.extend(fs_warning(path, &name)),
        None => found.push(unreadable_warning(path)),
    }
    if is_flash(path) {
        found.push(format!(
            "{}: on a flash (non-rotational) device, the device may keep old data in blocks it remapped",
            path.display()
        ));
    }
    if remote_drive(path) {
        found.push(format!(
            "{}: remote drive, overwriting happens on the server, which may keep copies",
            path.display()
        ));
    }
    found
}

/// the type of `path` could not be read: statfs failed, or a fuse mount had no mountinfo entry.
fn unreadable_warning(path: &Path) -> String {
    format!(
        "{}: could not read the filesystem type, it may be a remote one",
        path.display()
    )
}

fn fs_warning(path: &Path, fs_name: &str) -> Option<String> {
    let message = match classify(fs_name) {
        Kind::Local => return None,
        Kind::CopyOnWrite => {
            format!("{fs_name} is copy-on-write, old data may survive overwriting")
        }
        Kind::Network => format!(
            "{fs_name} is a network filesystem, overwriting happens on the server, which may keep copies"
        ),
        Kind::FuseOrUnknown => format!(
            "{fs_name} is a fuse or unrecognized filesystem, overwriting may happen on a remote server"
        ),
    };
    Some(format!("{}: {message}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_copy_on_write_filesystems() {
        for name in ["apfs", "btrfs", "zfs", "refs", "bcachefs"] {
            assert_eq!(classify(name), Kind::CopyOnWrite, "{name}");
        }
    }

    #[test]
    fn classifies_local_filesystems() {
        for name in [
            "ext2", "ext4", "xfs", "f2fs", "tmpfs", "ramfs", "overlay", "vfat", "msdos", "fat32",
            "exfat", "ntfs", "ntfs3", "hfs", "hfsplus", "jfs", "reiserfs", "iso9660", "cd9660",
            "cdfs", "udf", "squashfs", "erofs", "nilfs2", "cramfs", "minix", "ocfs2", "ecryptfs",
            "ufs",
        ] {
            assert_eq!(classify(name), Kind::Local, "{name}");
        }
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
    fn unreadable_filesystem_counts_as_network() {
        assert!(is_network(Path::new(
            "/nonexistent/sudo-mudo-apt-opt-gop-stop-remove"
        )));
    }
}
