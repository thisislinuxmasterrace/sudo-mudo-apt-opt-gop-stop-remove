//! conservative assessment of a single filesystem object.

use std::fs::Metadata;
use std::path::Path;

use crate::copies::{self, Found};
use crate::storage::{Compression, Encryption};
use crate::{json, storage};

#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assessment {
    verdict: &'static str,
    reasons: Vec<&'static str>,
    filesystem: Option<String>,
    medium: &'static str,
    compression: Compression,
    encryption: Encryption,
    /// `None` when the object was not looked at for other copies.
    copies: Option<Vec<Found>>,
}

impl Assessment {
    pub(crate) fn for_path(path: &Path, meta: &Metadata) -> Self {
        let facts = if meta.file_type().is_symlink()
            || crate::entry::is_unsupported_reparse(meta)
            || (!meta.is_file() && !meta.is_dir())
        {
            NO_FACTS
        } else {
            storage_facts(path, meta)
        };
        Self::with_facts(path, meta, facts)
    }

    pub(crate) fn capture(path: &Path) -> std::io::Result<(Metadata, Self)> {
        let meta = std::fs::symlink_metadata(path)?;
        #[cfg(windows)]
        if let Some((opened, facts)) = windows::capture(path, &meta) {
            let assessment = Self::with_facts(path, &opened, facts);
            return Ok((opened, assessment));
        }
        let assessment = Self::for_path(path, &meta);
        Ok((meta, assessment))
    }

    fn with_facts(path: &Path, meta: &Metadata, facts: StorageFacts) -> Self {
        if meta.file_type().is_symlink() {
            return Self {
                verdict: "cosmetic",
                reasons: vec!["removing the link does not overwrite its target"],
                filesystem: None,
                medium: "unknown",
                compression: Compression::Unknown,
                encryption: Encryption::NotChecked,
                copies: None,
            };
        }
        // special objects are refused by the erase; do not open them for advisory facts.
        if crate::entry::is_unsupported_reparse(meta) || (!meta.is_file() && !meta.is_dir()) {
            return Self::from_facts(
                None,
                "unknown",
                Compression::Unknown,
                Encryption::NotChecked,
                false,
                false,
                None,
            );
        }
        let (filesystem, medium, compression, encryption) = facts;
        let medium = match medium {
            Some(true) => "flash",
            Some(false) => "rotational",
            None => "unknown",
        };
        let copies = Some(copies::find(path, meta));
        Self::from_facts(
            filesystem,
            medium,
            compression,
            encryption,
            meta.is_dir(),
            meta.is_file(),
            copies,
        )
    }

    pub(crate) fn encryption(&self) -> Encryption {
        self.encryption
    }

    pub(crate) fn compression(&self) -> Compression {
        self.compression
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
        compression: Compression,
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
        reasons.push(compression.reason());
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
            compression,
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
            compression: Compression::Unknown,
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

    #[cfg(test)]
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
        out.push_str(",\"compression\":");
        json::push_str(out, self.compression.as_str());
        out.push_str(",\"snapshots\":\"unknown\",\"encryption\":");
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

/// the filesystem name, the medium (`Some(true)` for flash), and the compression and encryption facts
/// for an object.
type StorageFacts = (Option<String>, Option<bool>, Compression, Encryption);

/// what is known when the object could not be opened safely.
const NO_FACTS: StorageFacts = (None, None, Compression::Unknown, Encryption::NotChecked);

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
        return NO_FACTS;
    };
    let file = std::fs::File::from(fd);
    let Ok(opened) = file.metadata() else {
        return NO_FACTS;
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() || opened.file_type().is_symlink() {
        return NO_FACTS;
    }
    let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
    let descriptor = Path::new(&descriptor);
    let filesystem = storage::assessment_filesystem(descriptor);
    let compression = storage::assessment_compression(descriptor, filesystem.as_deref());
    let mut encryption = storage::assessment_encryption(descriptor, filesystem.as_deref());
    if matches!(filesystem.as_deref(), Some("ext4" | "f2fs" | "ubifs")) {
        encryption = encryption.with_fscrypt(storage::encrypted_file(&file));
    }
    (
        filesystem,
        storage::assessment_medium(descriptor),
        compression,
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
        return NO_FACTS;
    };
    let Ok(opened) = file.metadata() else {
        return NO_FACTS;
    };
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return NO_FACTS;
    }
    let compressed =
        nix::sys::stat::fstat(&file).is_ok_and(|stat| stat.st_flags & libc::UF_COMPRESSED != 0);
    (
        nix::sys::statfs::fstatfs(&file)
            .ok()
            .map(|stat| stat.filesystem_type_name().to_owned()),
        None,
        if compressed {
            Compression::Enabled
        } else {
            Compression::Unknown
        },
        Encryption::NotChecked,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn storage_facts(_path: &Path, _meta: &Metadata) -> StorageFacts {
    NO_FACTS
}

#[cfg(test)]
mod tests;
