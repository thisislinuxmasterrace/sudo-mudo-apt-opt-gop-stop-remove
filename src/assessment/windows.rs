//! filesystem facts bound to the same handle as the selected metadata.

use std::fs::{Metadata, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetVolumeInformationByHandleW,
};

use super::{NO_FACTS, StorageFacts};

pub(super) fn capture(path: &Path, initial: &Metadata) -> Option<(Metadata, StorageFacts)> {
    if (!initial.is_file() && !initial.is_dir())
        || initial.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return None;
    }
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if (!meta.is_file() && !meta.is_dir())
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return None;
    }
    let mut name = [0u16; 64];
    // safety: the handle is live and name is writable; unused outputs are null.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    let mut facts = NO_FACTS;
    if meta.file_attributes() & FILE_ATTRIBUTE_COMPRESSED != 0 {
        facts.2 = crate::storage::Compression::Enabled;
    }
    if ok != 0 {
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        facts.0 = Some(String::from_utf16_lossy(&name[..len]).to_ascii_lowercase());
    }
    Some((meta, facts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::Assessment;

    #[test]
    fn native_compression_flag_supplies_a_positive_fact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compressed");
        std::fs::write(&path, vec![b'x'; 128 * 1024]).unwrap();
        let output = std::process::Command::new("compact.exe")
            .args(["/c", "/i"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let (_, assessment) = Assessment::capture(&path).unwrap();
        assert_eq!(
            assessment.compression(),
            crate::storage::Compression::Enabled
        );
        assert_eq!(assessment.verdict(), crate::assessment::Verdict::Weak);
    }

    #[test]
    fn file_and_directory_use_handle_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"data").unwrap();
        for path in [path.as_path(), dir.path()] {
            let initial = std::fs::symlink_metadata(path).unwrap();
            let (_, facts) = capture(path, &initial).unwrap();
            let (_, assessment) = Assessment::capture(path).unwrap();
            assert!(facts.0.is_some());
            assert_eq!(assessment.filesystem(), facts.0.as_deref());
            assert_eq!(assessment.medium(), crate::storage::Medium::Unknown);
        }
    }

    #[test]
    fn old_metadata_never_supplies_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"data").unwrap();
        let old = std::fs::symlink_metadata(&path).unwrap();
        std::fs::rename(&path, dir.path().join("old")).unwrap();
        std::fs::write(&path, b"new").unwrap();
        assert_eq!(Assessment::for_path(&path, &old).filesystem(), None);
    }

    #[test]
    fn symlink_never_supplies_target_facts() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::write(&target, b"data").unwrap();
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return; // creating symlinks may require privileges on windows.
        }
        let initial = std::fs::symlink_metadata(&link).unwrap();
        assert!(capture(&link, &initial).is_none());
        let (_, assessment) = Assessment::capture(&link).unwrap();
        assert_eq!(assessment.verdict(), crate::assessment::Verdict::Cosmetic);
        assert_eq!(assessment.filesystem(), None);
        let target_meta = std::fs::symlink_metadata(&target).unwrap();
        assert!(capture(&link, &target_meta).is_none());
    }
}
