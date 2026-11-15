//! linux encryption facts: dm-crypt through sysfs, ecryptfs by mount name, and fscrypt through statx.
//! file-level encryption can coexist with an encrypted block-device stack.

#[cfg(target_os = "linux")]
use std::path::Path;

// only linux detects encryption; elsewhere the other variants exist for the shared reports.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encryption {
    /// nothing was looked at: not linux, or the device behind the path is unknown.
    NotChecked,
    /// the device stack was read and holds no dm-crypt mapping.
    NoneDetected,
    DmCrypt,
    Ecryptfs,
    Fscrypt,
    FscryptDmCrypt,
}

impl Encryption {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotChecked => "not_checked",
            Self::NoneDetected => "none_detected",
            Self::DmCrypt => "dm_crypt",
            Self::Ecryptfs => "ecryptfs",
            Self::Fscrypt => "fscrypt",
            Self::FscryptDmCrypt => "fscrypt_dm_crypt",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NotChecked => "encryption is not checked",
            Self::NoneDetected => {
                "no encryption was found by the available dm-crypt, ecryptfs and fscrypt checks"
            }
            Self::DmCrypt => {
                "the data is on a dm-crypt volume: what makes it unreadable is destroying every copy of the key, and an overwrite only reaches the encrypted blocks"
            }
            Self::Ecryptfs => {
                "the data is in an ecryptfs mount: destroying its key is what makes it unreadable, and an overwrite only reaches the encrypted lower files"
            }
            Self::Fscrypt => {
                "the filesystem encrypts this object with fscrypt; other files and key backups may retain access to its contents"
            }
            Self::FscryptDmCrypt => {
                "fscrypt encrypts this object on a dm-crypt volume; file keys and volume keys are separate, and backups may retain access"
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn with_fscrypt(self, encrypted: Option<bool>) -> Self {
        if encrypted != Some(true) {
            return self;
        }
        if self == Self::DmCrypt {
            Self::FscryptDmCrypt
        } else {
            Self::Fscrypt
        }
    }
}

/// reads attributes from the already checked descriptor, without opening file contents.
#[cfg(target_os = "linux")]
pub(crate) fn encrypted_file(file: &std::fs::File) -> Option<bool> {
    use std::os::fd::AsRawFd;

    let mut stat = std::mem::MaybeUninit::<libc::statx>::uninit();
    // safety: file is live, the empty path is nul terminated, and stat is a writable output.
    let result = unsafe {
        libc::statx(
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
            libc::STATX_BASIC_STATS,
            stat.as_mut_ptr(),
        )
    };
    if result != 0 {
        return None;
    }
    // safety: statx succeeded and initialized the output structure.
    let stat = unsafe { stat.assume_init() };
    encrypted_attribute(stat.stx_attributes, stat.stx_attributes_mask)
}

#[cfg(target_os = "linux")]
fn encrypted_attribute(attributes: u64, supported: u64) -> Option<bool> {
    let flag = libc::STATX_ATTR_ENCRYPTED as u64;
    (supported & flag != 0).then_some(attributes & flag != 0)
}

/// what is known about encryption under `path`, whose filesystem name is `filesystem`.
#[cfg(target_os = "linux")]
pub(crate) fn detect(path: &Path, filesystem: Option<&str>) -> Encryption {
    if filesystem == Some("ecryptfs") {
        return Encryption::Ecryptfs;
    }
    let Some(block) = super::flash::block_dir(path) else {
        return Encryption::NotChecked;
    };
    match has_dm_crypt(&block, 0) {
        Some(true) => Encryption::DmCrypt,
        Some(false) => Encryption::NoneDetected,
        None => Encryption::NotChecked,
    }
}

/// true for the uuid of a mapping that encrypts. libcryptsetup puts `CRYPT-` on every mapping it
/// creates, including the verity and integrity ones, which only check data and do not encrypt it.
#[cfg(target_os = "linux")]
fn is_encrypting_uuid(uuid: &str) -> bool {
    let uuid = uuid.trim();
    uuid.starts_with("CRYPT-")
        && !uuid.starts_with("CRYPT-VERITY-")
        && !uuid.starts_with("CRYPT-INTEGRITY-")
}

/// true when `block_dir` or any device below it (its `slaves`) is a dm-crypt mapping. `None` when sysfs
/// cannot be read or the stack is too deep to follow. a device without a `slaves` directory is a leaf.
#[cfg(target_os = "linux")]
fn has_dm_crypt(block_dir: &Path, depth: u32) -> Option<bool> {
    if depth > 8 {
        return None;
    }
    let dir = std::fs::canonicalize(block_dir).ok()?;
    if std::fs::read_to_string(dir.join("dm").join("uuid"))
        .is_ok_and(|uuid| is_encrypting_uuid(&uuid))
    {
        return Some(true);
    }
    let slaves = match std::fs::read_dir(dir.join("slaves")) {
        Ok(slaves) => slaves,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    let mut unknown = false;
    for slave in slaves {
        let Ok(slave) = slave else {
            unknown = true;
            continue;
        };
        match has_dm_crypt(&slave.path(), depth + 1) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown { None } else { Some(false) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn encryption_attribute_requires_support_even_when_set() {
        let flag = libc::STATX_ATTR_ENCRYPTED as u64;
        assert_eq!(encrypted_attribute(flag, 0), None);
        assert_eq!(encrypted_attribute(0, 0), None);
        assert_eq!(encrypted_attribute(flag, flag), Some(true));
        assert_eq!(encrypted_attribute(0, flag), Some(false));
    }

    #[test]
    fn file_encryption_keeps_the_volume_encryption_layer() {
        assert_eq!(
            Encryption::DmCrypt.with_fscrypt(Some(true)),
            Encryption::FscryptDmCrypt
        );
        assert_eq!(
            Encryption::NoneDetected.with_fscrypt(Some(true)),
            Encryption::Fscrypt
        );
        assert_eq!(Encryption::DmCrypt.with_fscrypt(None), Encryption::DmCrypt);
    }

    #[test]
    fn metadata_only_descriptor_supports_statx() {
        use nix::fcntl::{OFlag, open};
        use nix::sys::stat::Mode;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain");
        fs::write(&path, b"data").unwrap();
        let fd = open(&path, OFlag::O_PATH | OFlag::O_NOFOLLOW, Mode::empty()).unwrap();
        let file = std::fs::File::from(fd);
        if super::super::fs_name::filesystem_name(&path).as_deref() == Some("ext4") {
            assert_eq!(encrypted_file(&file), Some(false));
        } else {
            assert_ne!(encrypted_file(&file), Some(true));
        }
    }

    fn dm(root: &Path, name: &str, uuid: &str) -> std::path::PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("dm")).unwrap();
        fs::write(dir.join("dm").join("uuid"), format!("{uuid}\n")).unwrap();
        dir
    }

    #[test]
    fn crypt_mapping_is_found_directly_and_below_another_mapping() {
        let root = tempfile::tempdir().unwrap();
        let crypt = dm(root.path(), "dm-0", "CRYPT-LUKS2-abc-name");
        assert_eq!(has_dm_crypt(&crypt, 0), Some(true));

        let lvm = dm(root.path(), "dm-1", "LVM-xyz");
        fs::create_dir_all(lvm.join("slaves")).unwrap();
        std::os::unix::fs::symlink(&crypt, lvm.join("slaves").join("dm-0")).unwrap();
        assert_eq!(has_dm_crypt(&lvm, 0), Some(true));
    }

    #[test]
    fn verity_and_integrity_mappings_do_not_encrypt() {
        let root = tempfile::tempdir().unwrap();
        for (name, uuid) in [
            ("dm-0", "CRYPT-VERITY-abc-name"),
            ("dm-1", "CRYPT-INTEGRITY-abc-name"),
        ] {
            assert_eq!(has_dm_crypt(&dm(root.path(), name, uuid), 0), Some(false));
        }
        for uuid in [
            "CRYPT-PLAIN-name",
            "CRYPT-LUKS1-abc-name",
            "CRYPT-BITLK-abc",
        ] {
            assert!(is_encrypting_uuid(uuid), "{uuid}");
        }
    }

    #[test]
    fn plain_devices_and_plain_mappings_have_none() {
        let root = tempfile::tempdir().unwrap();
        let disk = root.path().join("sda");
        fs::create_dir_all(disk.join("queue")).unwrap();
        assert_eq!(has_dm_crypt(&disk, 0), Some(false));

        let lvm = dm(root.path(), "dm-1", "LVM-xyz");
        fs::create_dir_all(lvm.join("slaves")).unwrap();
        std::os::unix::fs::symlink(&disk, lvm.join("slaves").join("sda")).unwrap();
        assert_eq!(has_dm_crypt(&lvm, 0), Some(false));
    }

    #[test]
    fn unreadable_sysfs_is_unknown_not_none() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(has_dm_crypt(&root.path().join("missing"), 0), None);
    }

    #[test]
    fn a_loop_of_slaves_stops_instead_of_recursing_forever() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        fs::create_dir_all(a.join("slaves")).unwrap();
        std::os::unix::fs::symlink(&a, a.join("slaves").join("a")).unwrap();
        assert_eq!(has_dm_crypt(&a, 0), None);
    }

    #[test]
    fn ecryptfs_is_recognized_by_name() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(detect(root.path(), Some("ecryptfs")), Encryption::Ecryptfs);
    }
}
