//! conservative assessment of a single filesystem object.

use std::fs::Metadata;
use std::path::Path;

use crate::{json, storage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assessment {
    verdict: &'static str,
    reasons: Vec<&'static str>,
    filesystem: Option<String>,
    medium: &'static str,
}

impl Assessment {
    pub(crate) fn for_path(path: &Path, meta: &Metadata) -> Self {
        if meta.file_type().is_symlink() {
            return Self {
                verdict: "cosmetic",
                reasons: vec!["removing the link does not overwrite its target"],
                filesystem: None,
                medium: "unknown",
            };
        }
        // special objects are refused by the erase; do not open them for advisory facts.
        if !meta.is_file() && !meta.is_dir() {
            return Self::from_facts(None, "unknown", false, false);
        }
        let (filesystem, medium) = storage_facts(path, meta);
        let medium = match medium {
            Some(true) => "flash",
            Some(false) => "rotational",
            None => "unknown",
        };
        Self::from_facts(filesystem, medium, meta.is_dir(), meta.is_file())
    }

    fn from_facts(
        filesystem: Option<String>,
        medium: &'static str,
        is_dir: bool,
        is_file: bool,
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
        reasons.push(
            "compression and snapshots are unknown; other copies and encryption are not checked",
        );
        Self {
            verdict: "weak",
            reasons,
            filesystem,
            medium,
        }
    }

    pub(crate) fn unknown() -> Self {
        Self {
            verdict: "weak",
            reasons: vec!["object could not be assessed; coverage is incomplete"],
            filesystem: None,
            medium: "unknown",
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
        out.push_str(",\"compression\":\"unknown\",\"snapshots\":\"unknown\",\"encryption\":\"not_checked\",\"other_copies\":\"not_checked\"}");
    }
}

#[cfg(target_os = "linux")]
fn storage_facts(path: &Path, meta: &Metadata) -> (Option<String>, Option<bool>) {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    let Ok(fd) = open(
        path,
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) else {
        return (None, None);
    };
    let file = std::fs::File::from(fd);
    let Ok(opened) = file.metadata() else {
        return (None, None);
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() || opened.file_type().is_symlink() {
        return (None, None);
    }
    let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
    let descriptor = Path::new(&descriptor);
    (
        storage::assessment_filesystem(descriptor),
        storage::assessment_medium(descriptor),
    )
}

#[cfg(target_os = "macos")]
fn storage_facts(path: &Path, meta: &Metadata) -> (Option<String>, Option<bool>) {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return (None, None);
    };
    let Ok(opened) = file.metadata() else {
        return (None, None);
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return (None, None);
    }
    (
        nix::sys::statfs::fstatfs(&file)
            .ok()
            .map(|stat| stat.filesystem_type_name().to_owned()),
        None,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn storage_facts(_path: &Path, _meta: &Metadata) -> (Option<String>, Option<bool>) {
    (None, None)
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
        assert!(fields.contains("\"other_copies\":\"not_checked\""));
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
    fn uncertain_facts_never_produce_effective() {
        for filesystem in [
            None,
            Some("ext4"),
            Some("btrfs"),
            Some("nfs"),
            Some("other"),
        ] {
            for medium in ["unknown", "flash", "rotational"] {
                let assessment =
                    Assessment::from_facts(filesystem.map(str::to_owned), medium, false, true);
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
