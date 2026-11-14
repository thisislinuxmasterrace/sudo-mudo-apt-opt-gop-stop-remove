//! leaf policy shared by preview, advisory facts and erase.

use std::fs::Metadata;

#[cfg(windows)]
pub(crate) fn is_unsupported_reparse(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 && !meta.file_type().is_symlink()
}

#[cfg(not(windows))]
pub(crate) fn is_unsupported_reparse(_meta: &Metadata) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests;
