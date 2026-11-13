//! encryption facts for the assessment. only two cases are recognized: a block device stack with a
//! dm-crypt (luks) mapping, found through sysfs on linux, and an ecryptfs mount, found by its name.
//! everything else, including fscrypt and self-encrypting drives, is reported as not detected.

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
}

impl Encryption {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotChecked => "not_checked",
            Self::NoneDetected => "none_detected",
            Self::DmCrypt => "dm_crypt",
            Self::Ecryptfs => "ecryptfs",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NotChecked => "encryption is not checked",
            Self::NoneDetected => {
                "no encryption was found (only dm-crypt and ecryptfs are looked for)"
            }
            Self::DmCrypt => {
                "the data is on a dm-crypt volume: what makes it unreadable is destroying every copy of the key, and an overwrite only reaches the encrypted blocks"
            }
            Self::Ecryptfs => {
                "the data is in an ecryptfs mount: destroying its key is what makes it unreadable, and an overwrite only reaches the encrypted lower files"
            }
        }
    }
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
