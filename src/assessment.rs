//! conservative assessment of a single filesystem object.

use std::fs::Metadata;
use std::path::Path;

use crate::copies::{self, Found};
use crate::storage::Encryption;
use crate::{json, storage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assessment {
    verdict: &'static str,
    reasons: Vec<&'static str>,
    filesystem: Option<String>,
    medium: &'static str,
    encryption: Encryption,
    /// `None` when the object was not looked at for other copies.
    copies: Option<Vec<Found>>,
}

impl Assessment {
    pub(crate) fn for_path(path: &Path, meta: &Metadata) -> Self {
        if meta.file_type().is_symlink() {
            return Self {
                verdict: "cosmetic",
                reasons: vec!["removing the link does not overwrite its target"],
                filesystem: None,
                medium: "unknown",
                encryption: Encryption::NotChecked,
                copies: None,
            };
        }
        // special objects are refused by the erase; do not open them for advisory facts.
        if !meta.is_file() && !meta.is_dir() {
            return Self::from_facts(None, "unknown", Encryption::NotChecked, false, false, None);
        }
        let (filesystem, medium, encryption) = storage_facts(path, meta);
        let medium = match medium {
            Some(true) => "flash",
            Some(false) => "rotational",
            None => "unknown",
        };
        let copies = Some(copies::find(path, meta));
        Self::from_facts(
            filesystem,
            medium,
            encryption,
            meta.is_dir(),
            meta.is_file(),
            copies,
        )
    }

    pub(crate) fn encryption(&self) -> Encryption {
        self.encryption
    }

    pub(crate) fn filesystem(&self) -> Option<&str> {
        self.filesystem.as_deref()
    }

    pub(crate) fn medium(&self) -> &'static str {
        self.medium
    }

    pub(crate) fn verdict(&self) -> &'static str {
        self.verdict
    }

    /// the kinds of other copies that were found, empty when none were found or none were looked for.
    pub(crate) fn copy_kinds(&self) -> impl Iterator<Item = copies::Kind> + '_ {
        self.copies.iter().flatten().map(|copy| copy.kind)
    }

    pub(crate) fn from_facts(
        filesystem: Option<String>,
        medium: &'static str,
        encryption: Encryption,
        is_dir: bool,
        is_file: bool,
        copies: Option<Vec<Found>>,
    ) -> Self {
        let mut reasons = Vec::new();
        if is_dir {
            reasons.push("directory assessment covers metadata only; descendants are assessed separately and coverage may be incomplete");
        } else if !is_file {
            reasons.push("object contents are not assessed");
        }
        match filesystem.as_deref() {
            Some(name) => reasons.extend(storage::assessment_fs_reason(name)),
            None => reasons.push("filesystem is unknown"),
        }
        match medium {
            "flash" => reasons.push("flash may retain remapped blocks"),
            "unknown" => reasons.push("storage medium is unknown"),
            _ => {}
        }
        reasons.push("compression and snapshots are unknown");
        reasons.push(encryption.reason());
        match &copies {
            Some(found) if !found.is_empty() => {
                let mut kinds: Vec<_> = found.iter().map(|copy| copy.kind).collect();
                kinds.dedup();
                reasons.extend(kinds.into_iter().map(copies::Kind::reason));
            }
            Some(_) => reasons.push(
                "no other copy was found in the few places checked (git work tree, editor backups, trash, sync folders); others may exist",
            ),
            None => reasons.push("other copies are not checked"),
        }
        Self {
            verdict: "weak",
            reasons,
            filesystem,
            medium,
            encryption,
            copies,
        }
    }

    pub(crate) fn unknown() -> Self {
        Self {
            verdict: "weak",
            reasons: vec!["object could not be assessed; coverage is incomplete"],
            filesystem: None,
            medium: "unknown",
            encryption: Encryption::NotChecked,
            copies: None,
        }
    }

    pub(crate) fn unattempted(&self) -> Self {
        let mut assessment = self.clone();
        assessment
            .reasons
            .push("object contents are not scheduled for overwrite");
        assessment
    }

    pub(crate) fn line(&self) -> String {
        format!("{}: {}", self.verdict, self.reasons.join("; "))
    }

    pub(crate) fn write_fields(&self, out: &mut String) {
        out.push_str(",\"verdict\":");
        json::push_str(out, self.verdict);
        out.push_str(",\"reasons\":[");
        for (i, reason) in self.reasons.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            json::push_str(out, reason);
        }
        out.push_str("],\"facts\":{\"filesystem\":");
        if let Some(name) = &self.filesystem {
            json::push_str(out, name);
        } else {
            out.push_str("null");
        }
        out.push_str(",\"medium\":");
        json::push_str(out, self.medium);
        out.push_str(",\"compression\":\"unknown\",\"snapshots\":\"unknown\",\"encryption\":");
        json::push_str(out, self.encryption.as_str());
        out.push_str(",\"other_copies\":");
        let state = match &self.copies {
            None => "not_checked",
            Some(found) if found.is_empty() => "none_found",
            Some(_) => "found",
        };
        json::push_str(out, state);
        out.push_str(",\"copies\":[");
        for (i, copy) in self.copies.iter().flatten().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"kind\":");
            json::push_str(out, copy.kind.as_str());
            out.push(',');
            json::push_path_fields(out, &copy.path);
            out.push('}');
        }
        out.push_str("]}");
    }
}

/// the filesystem name, the medium (`Some(true)` for flash) and the encryption fact for an object.
type StorageFacts = (Option<String>, Option<bool>, Encryption);

#[cfg(target_os = "linux")]
fn storage_facts(path: &Path, meta: &Metadata) -> StorageFacts {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    let Ok(fd) = open(
        path,
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) else {
        return (None, None, Encryption::NotChecked);
    };
    let file = std::fs::File::from(fd);
    let Ok(opened) = file.metadata() else {
        return (None, None, Encryption::NotChecked);
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() || opened.file_type().is_symlink() {
        return (None, None, Encryption::NotChecked);
    }
    let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
    let descriptor = Path::new(&descriptor);
    let filesystem = storage::assessment_filesystem(descriptor);
    let encryption = storage::assessment_encryption(descriptor, filesystem.as_deref());
    (
        filesystem,
        storage::assessment_medium(descriptor),
        encryption,
    )
}

#[cfg(target_os = "macos")]
fn storage_facts(path: &Path, meta: &Metadata) -> StorageFacts {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return (None, None, Encryption::NotChecked);
    };
    let Ok(opened) = file.metadata() else {
        return (None, None, Encryption::NotChecked);
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return (None, None, Encryption::NotChecked);
    }
    (
        nix::sys::statfs::fstatfs(&file)
            .ok()
            .map(|stat| stat.filesystem_type_name().to_owned()),
        None,
        Encryption::NotChecked,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn storage_facts(_path: &Path, _meta: &Metadata) -> StorageFacts {
    (None, None, Encryption::NotChecked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_directory_are_weak_and_report_unknowns() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"data").unwrap();
        let file_assessment =
            Assessment::for_path(&file, &std::fs::symlink_metadata(&file).unwrap());
        assert!(file_assessment.line().starts_with("weak:"));
        let mut fields = String::new();
        file_assessment.write_fields(&mut fields);
        assert!(fields.contains("\"compression\":\"unknown\""));
        assert!(fields.contains("\"snapshots\":\"unknown\""));
        assert!(fields.contains("\"other_copies\":\"none_found\",\"copies\":[]"));
        assert!(file_assessment.line().contains("others may exist"));
        let directory =
            Assessment::for_path(dir.path(), &std::fs::symlink_metadata(dir.path()).unwrap());
        assert!(directory.line().contains("metadata only"));
        assert!(directory.line().contains("coverage may be incomplete"));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_link_does_not_inspect_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("missing", &link).unwrap();
        let assessment = Assessment::for_path(&link, &std::fs::symlink_metadata(&link).unwrap());
        assert!(assessment.line().starts_with("cosmetic:"));
        assert_eq!(assessment.filesystem, None);
        assert_eq!(assessment.medium, "unknown");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replaced_path_does_not_supply_facts_for_selected_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"data").unwrap();
        let meta = std::fs::symlink_metadata(&file).unwrap();
        std::fs::rename(&file, dir.path().join("old")).unwrap();
        std::os::unix::fs::symlink("/", &file).unwrap();
        let assessment = Assessment::for_path(&file, &meta);
        assert_eq!(assessment.filesystem, None);
        assert_eq!(assessment.medium, "unknown");
        assert_eq!(assessment.verdict, "weak");
    }

    #[test]
    fn unreadable_object_is_unknown() {
        let assessment = Assessment::unknown();
        assert!(assessment.line().contains("coverage is incomplete"));
        assert_eq!(assessment.medium, "unknown");
    }

    #[cfg(unix)]
    #[test]
    fn special_objects_do_not_supply_storage_facts() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
        let assessment = Assessment::for_path(&fifo, &std::fs::symlink_metadata(&fifo).unwrap());
        assert_eq!(assessment.filesystem, None);
        assert_eq!(assessment.medium, "unknown");
        assert!(
            assessment
                .line()
                .contains("object contents are not assessed")
        );
    }

    #[test]
    fn found_copies_are_listed_and_never_raise_the_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"data").unwrap();
        std::fs::write(dir.path().join("notes.txt~"), b"old").unwrap();
        let assessment = Assessment::for_path(&file, &std::fs::symlink_metadata(&file).unwrap());
        assert_eq!(assessment.verdict, "weak");
        assert!(assessment.line().contains("editor backup"));
        let mut fields = String::new();
        assessment.write_fields(&mut fields);
        assert!(fields.contains("\"other_copies\":\"found\""), "{fields}");
        assert!(
            fields.contains("{\"kind\":\"editor_backup\",\"path\":"),
            "{fields}"
        );
        assert!(fields.contains("notes.txt~"), "{fields}");
    }

    #[test]
    fn uncertain_facts_never_produce_effective() {
        for filesystem in [
            None,
            Some("ext4"),
            Some("btrfs"),
            Some("nfs"),
            Some("other"),
        ] {
            for medium in ["unknown", "flash", "rotational"] {
                for encryption in [
                    Encryption::NotChecked,
                    Encryption::NoneDetected,
                    Encryption::DmCrypt,
                    Encryption::Ecryptfs,
                ] {
                    let assessment = Assessment::from_facts(
                        filesystem.map(str::to_owned),
                        medium,
                        encryption,
                        false,
                        true,
                        None,
                    );
                    assert_eq!(assessment.verdict, "weak");
                    assert!(
                        assessment
                            .line()
                            .contains("compression and snapshots are unknown")
                    );
                }
            }
        }
    }

    #[test]
    fn encryption_is_a_fact_and_a_reason_and_never_a_verdict() {
        let assessment = Assessment::from_facts(
            Some("ext4".to_owned()),
            "flash",
            Encryption::DmCrypt,
            false,
            true,
            None,
        );
        let mut fields = String::new();
        assessment.write_fields(&mut fields);
        assert!(fields.contains("\"encryption\":\"dm_crypt\""), "{fields}");
        assert!(
            assessment
                .line()
                .contains("destroying every copy of the key")
        );
        assert_eq!(assessment.verdict, "weak");
        let plain =
            Assessment::from_facts(None, "unknown", Encryption::NotChecked, false, true, None);
        let mut fields = String::new();
        plain.write_fields(&mut fields);
        assert!(
            fields.contains("\"encryption\":\"not_checked\""),
            "{fields}"
        );
    }
}
