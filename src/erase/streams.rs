//! windows named data streams. enumeration is advisory; opened streams must match the base file.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_HANDLE_EOF, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FindClose,
    FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, GetFileInformationByHandle,
    GetVolumeInformationByHandleW, WIN32_FIND_STREAM_DATA,
};
use windows_sys::Win32::System::SystemServices::FILE_NAMED_STREAMS;

struct Search(HANDLE);

impl Drop for Search {
    fn drop(&mut self) {
        // safety: this search handle was returned by FindFirstStreamW and is closed once.
        unsafe {
            FindClose(self.0);
        }
    }
}

pub(super) fn names(path: &Path) -> io::Result<Vec<OsString>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.components().any(|part| {
        matches!(part, std::path::Component::Normal(name)
        if name.encode_wide().any(|ch| ch == b':' as u16))
    }) {
        return Err(io::Error::other(
            "name the base file, not an individual data stream",
        ));
    }
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut flags = 0;
    // safety: the file handle is live, flags is writable, and unused outputs are null.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_NAMED_STREAMS == 0 {
        return Ok(Vec::new());
    }
    let mut wide = wide;
    wide.push(0);
    let mut data = WIN32_FIND_STREAM_DATA::default();
    // safety: wide is nul terminated and data is a correctly sized writable output.
    let handle = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
            Ok(Vec::new())
        } else {
            Err(err)
        };
    }
    let search = Search(handle);
    let mut names = Vec::new();
    loop {
        super::check_interrupted()?;
        let end = data
            .cStreamName
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| io::Error::other("invalid data stream name"))?;
        let name = &data.cStreamName[..end];
        if name != [58, 58, 36, 68, 65, 84, 65] {
            if name.len() < 8
                || name.first() != Some(&58)
                || !name.ends_with(&[58, 36, 68, 65, 84, 65])
                || name[1..name.len().saturating_sub(6)]
                    .iter()
                    .any(|c| matches!(*c, 47 | 92 | 58))
            {
                return Err(io::Error::other(
                    "unsupported data stream name; the file is kept",
                ));
            }
            names.push(OsString::from_wide(name));
        }
        // safety: search is live and data is writable for the next record.
        if unsafe { FindNextStreamW(search.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
            == 0
        {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_HANDLE_EOF as i32) {
                return Err(err);
            }
            break;
        }
    }
    names.sort();
    Ok(names)
}

fn identity(file: &File) -> io::Result<(u32, u32, u32)> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // safety: file is live and info is a writable output structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

/// opens all streams before any content is touched; a locked stream keeps the entire file.
pub(super) fn open_named(path: &Path, base: &File) -> io::Result<(Vec<OsString>, Vec<File>)> {
    let names = names(path)?;
    let expected = identity(base)?;
    let mut files = Vec::new();
    for name in &names {
        let mut stream_path = path.as_os_str().to_os_string();
        stream_path.push(name);
        let file =
            crate::platform::windows::open_for_erase(Path::new(&stream_path)).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "could not open data stream {}: {err}; the file is kept",
                        name.to_string_lossy()
                    ),
                )
            })?;
        if identity(&file)? != expected {
            return Err(io::Error::other(
                "data stream belongs to a replaced file; refusing to overwrite it",
            ));
        }
        files.push(file);
    }
    let current = crate::platform::windows::open_for_erase(path)?;
    if identity(&current)? != expected {
        return Err(io::Error::other(
            "file changed during stream enumeration; refusing to overwrite it",
        ));
    }
    Ok((names, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_stream_is_opened_as_the_same_file_and_kept_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"main").unwrap();
        let stream = dir.path().join("file:secret");
        std::fs::write(&stream, b"secret").unwrap();
        let base = crate::platform::windows::open_for_erase(&path).unwrap();
        let (names, files) = open_named(&path, &base).unwrap();
        assert_eq!(names, [OsString::from(":secret:$DATA")]);
        assert_eq!(files.len(), 1);
        assert_eq!(identity(&base).unwrap(), identity(&files[0]).unwrap());
        assert!(self::names(&stream).is_err());
        assert_eq!(std::fs::read(&stream).unwrap(), b"secret");
    }
}
