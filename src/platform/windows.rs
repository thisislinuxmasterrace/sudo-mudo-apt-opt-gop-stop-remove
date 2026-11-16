use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::iter::once;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, GetFileInformationByHandle,
    GetFinalPathNameByHandleW, MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW,
    SetFileInformationByHandle, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmForceShutdown, RmGetList,
    RmRegisterResources, RmShutdown, RmStartSession,
};

pub use super::console::set_interrupt_handler;

/// access for `open_for_erase`: read and write to overwrite, delete to unlink later.
const ERASE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;
/// full sharing, so our handles never block other normal handles.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RM_LIST_ATTEMPTS: usize = 3;
const PATH_ATTEMPTS: usize = 3;

/// a process that holds a file open, as reported by the restart manager. `start` is its creation
/// time, so a reused pid is not mistaken for the process that was listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
    pub start: u64,
}

/// processes holding `path` open, according to the restart manager. read-only.
pub fn holders(path: &Path) -> io::Result<Vec<Holder>> {
    list_holders(&registered_session(path)?)
}

/// a restart manager session that stays open after listing, so the processes that get closed are the
/// ones that were listed.
pub struct HolderSession(RmSession);

/// lists the processes holding `path` open and keeps the session for `HolderSession::shut_down`.
pub fn holder_session(path: &Path) -> io::Result<(Vec<Holder>, HolderSession)> {
    let session = registered_session(path)?;
    let listed = list_holders(&session)?;
    Ok((listed, HolderSession(session)))
}

impl HolderSession {
    /// force-closes the processes of this session, but only when every one that holds the file now
    /// was in `shown`. a program the user saw and that has closed since is harmless; one that opened
    /// the file after the listing was never shown.
    pub fn shut_down(&self, shown: &[Holder]) -> io::Result<()> {
        if !only_shown(shown, &list_holders(&self.0)?) {
            return Err(io::Error::other(
                "a process that was not listed holds the file now, none were closed and the file is kept",
            ));
        }
        rm_force_shutdown(&self.0)
    }
}

fn list_holders(session: &RmSession) -> io::Result<Vec<Holder>> {
    Ok(session.process_list()?.iter().map(holder_from).collect())
}

/// true when every process in `current` is also in `shown`, matched by pid and start time.
fn only_shown(shown: &[Holder], current: &[Holder]) -> bool {
    current.iter().all(|now| {
        shown
            .iter()
            .any(|s| s.pid == now.pid && s.start == now.start)
    })
}

/// opens `path` for overwriting, with read, write and delete access.
pub fn open_for_erase(path: &Path) -> io::Result<File> {
    // open the entry itself, not a link it may have been replaced with since the check
    let file = OpenOptions::new()
        .access_mode(ERASE_ACCESS)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(
            "entry is a link or reparse point (for example a cloud file), refusing to overwrite it",
        ));
    }
    Ok(file)
}

/// opens `path` with delete access only, for `posix_delete`. the entry itself is opened, so a
/// link swapped in for the file is never followed: deleting the target would reach outside the tree.
pub fn open_for_delete(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(
            "entry is a link or reparse point, not deleting it through a handle",
        ));
    }
    Ok(file)
}

/// marks the file for a posix delete. the file must have been opened with delete access.
///
/// the name goes when this handle closes, even while other handles stay open, so the path can be
/// reused while the data stays alive until the last handle closes.
pub fn posix_delete(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    set_info(file, FileDispositionInfoEx, &info)
}

/// the classic delete-on-close: the file disappears when the last handle to it is closed. for
/// filesystems without posix delete (fat, exfat) and old windows. the file must have been opened
/// with delete access.
pub fn delete_on_close(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    set_info(file, FileDispositionInfo, &info)
}

/// renames the open file to `new_path`, a full path, and never replaces an existing entry. the file
/// must have been opened with delete access. the name that is changed is the one the handle was opened
/// through, so a swap of the old path afterwards cannot redirect the rename.
pub fn rename_by_handle(file: &File, new_path: &Path) -> io::Result<()> {
    let name: Vec<u16> = new_path.as_os_str().encode_wide().collect();
    let (buf, size) = rename_info(&name);
    // safety: the handle is valid while `file` is borrowed; buf is live and holds at least `size` bytes.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            buf.as_ptr().cast(),
            size as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// a FILE_RENAME_INFO for `name`, and its size in bytes. the name is followed by a nul wide char
/// that the length field does not count, as the filesystem expects.
fn rename_info(name: &[u16]) -> (Vec<u64>, usize) {
    let name_bytes = std::mem::size_of_val(name);
    let header = offset_of!(FILE_RENAME_INFO, FileName);
    let size = (header + name_bytes + size_of::<u16>()).max(size_of::<FILE_RENAME_INFO>());
    // u64 words give the buffer the alignment of the structure. zeroed memory means no replacing
    // (ReplaceIfExists false), no root directory handle and the terminating nul.
    let mut buf = vec![0u64; size.div_ceil(size_of::<u64>())];
    let base = buf.as_mut_ptr();
    // safety: buf is zeroed, aligned for FILE_RENAME_INFO and at least `size` bytes long, which is
    // the header plus the name plus one wide char, so both writes stay inside it; the name is copied
    // where FileName starts, and the zeroed bytes after it are the terminator.
    unsafe {
        (*base.cast::<FILE_RENAME_INFO>()).FileNameLength = name_bytes as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            base.cast::<u8>().add(header).cast::<u16>(),
            name.len(),
        );
    }
    (buf, size)
}

/// identity of a file or directory: its volume and its index on that volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId {
    volume: u32,
    index: u64,
}

impl FileId {
    /// the identity as one comparable pair.
    pub fn pair(self) -> (u64, u64) {
        (u64::from(self.volume), self.index)
    }
}

fn handle_info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // safety: the handle is valid while `file` is borrowed and info is a writable output structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(info)
    }
}

/// identity of the open file or directory.
pub fn file_id_of(file: &File) -> io::Result<FileId> {
    let info = handle_info(file)?;
    Ok(FileId {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// number of names the open file has.
pub fn link_count_of(file: &File) -> io::Result<u32> {
    Ok(handle_info(file)?.nNumberOfLinks)
}

/// opens `path` for attribute queries only, with full sharing. a link is opened itself unless `follow`.
fn open_for_query(path: &Path, follow: bool) -> io::Result<File> {
    let flags = if follow {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
    };
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(SHARE_ALL)
        .custom_flags(flags)
        .open(path)
}

/// identity of `path`, following links when `follow` is set.
pub fn file_id(path: &Path, follow: bool) -> io::Result<FileId> {
    file_id_of(&open_for_query(path, follow)?)
}

/// number of hard links of the entry at `path`, without following links. `None` when it cannot be read.
pub fn link_count_at(path: &Path) -> Option<u64> {
    let file = open_for_query(path, false).ok()?;
    link_count_of(&file).ok().map(u64::from)
}

/// opens the entry at `path` itself, never the target of a link, for attribute queries only.
pub fn open_entry(path: &Path) -> io::Result<File> {
    open_for_query(path, false)
}

/// whether the open entry is a reparse point: a link, a junction or a placeholder.
pub fn is_reparse_point(file: &File) -> io::Result<bool> {
    Ok(handle_info(file)?.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// the current full path of the open file, in the `\\?\` form. it names the entry as it is now, even
/// when the path the handle was opened with has been replaced since.
pub fn final_path_of(file: &File) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 512];
    for _ in 0..PATH_ATTEMPTS {
        // safety: the handle is valid while `file` is borrowed; buf is writable for its whole length.
        let len = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < buf.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buf[..len])));
        }
        // too small: the result is the size needed, with the terminator.
        buf.resize(len, 0);
    }
    Err(io::Error::other("the path of the file keeps changing"))
}

/// schedules `path` for deletion at the next reboot. needs administrator rights.
pub fn delete_on_reboot(path: &Path) -> io::Result<()> {
    let canonical = fs::canonicalize(path)?;
    move_file_delay_until_reboot(&wide_nul(canonical.as_os_str()))
}

/// a restart manager session, ended on drop.
struct RmSession(u32);

impl RmSession {
    /// starts a session. sessions are never joined, so the key is not used.
    fn start() -> io::Result<Self> {
        let mut handle = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        // safety: handle and key are writable; key has CCH_RM_SESSION_KEY + 1 wide chars, as required.
        let code = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        check_rm(code)?;
        Ok(Self(handle))
    }

    /// registers one file. `wide_path` must be nul-terminated.
    fn register(&self, wide_path: &[u16]) -> io::Result<()> {
        let files = [wide_path.as_ptr()];
        // safety: files holds one pointer to a nul-terminated string that outlives the call;
        // no applications or services are passed (null pointers with count 0).
        let code = unsafe {
            RmRegisterResources(self.0, 1, files.as_ptr(), 0, ptr::null(), 0, ptr::null())
        };
        check_rm(code)
    }

    /// returns the processes using the registered files. queries the size first and
    /// retries if the list grew in between.
    fn process_list(&self) -> io::Result<Vec<RM_PROCESS_INFO>> {
        let mut capacity = 0u32;
        let mut reasons = 0u32;
        for _ in 0..RM_LIST_ATTEMPTS {
            let mut list = vec![RM_PROCESS_INFO::default(); capacity as usize];
            let mut needed = 0u32;
            let mut written = capacity;
            let buf = if list.is_empty() {
                ptr::null_mut()
            } else {
                list.as_mut_ptr()
            };
            // safety: buf is either null with written == 0, or points to `written` writable entries;
            // needed and reasons are valid out pointers.
            let code = unsafe { RmGetList(self.0, &mut needed, &mut written, buf, &mut reasons) };
            match code {
                ERROR_SUCCESS => {
                    list.truncate(written as usize);
                    return Ok(list);
                }
                ERROR_MORE_DATA => capacity = needed,
                _ => return Err(io::Error::from_raw_os_error(code as i32)),
            }
        }
        Err(io::Error::from_raw_os_error(ERROR_MORE_DATA as i32))
    }
}

impl Drop for RmSession {
    fn drop(&mut self) {
        // safety: the handle came from a successful RmStartSession and is ended only here.
        // the result is ignored: drop cannot report it, and the session is local.
        unsafe { RmEndSession(self.0) };
    }
}

/// starts a session that has `path` registered.
fn registered_session(path: &Path) -> io::Result<RmSession> {
    let wide = rm_path(path)?;
    let session = RmSession::start()?;
    session.register(&wide)?;
    Ok(session)
}

/// force-stops the applications using the session's resources.
fn rm_force_shutdown(session: &RmSession) -> io::Result<()> {
    // safety: session.0 is a live session; no status callback is passed.
    let code = unsafe { RmShutdown(session.0, RmForceShutdown as u32, None) };
    check_rm(code)
}

/// converts a restart manager entry. the app name is a fixed buffer, read up to its first nul.
fn holder_from(info: &RM_PROCESS_INFO) -> Holder {
    let len = info
        .strAppName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.strAppName.len());
    let created = info.Process.ProcessStartTime;
    Holder {
        pid: info.Process.dwProcessId,
        name: String::from_utf16_lossy(&info.strAppName[..len]),
        start: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
    }
}

/// restart manager calls return the error code directly and do not set GetLastError.
fn check_rm(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

/// canonical path for restart manager. the `\\?\` prefix is dropped for drive paths
/// because the restart manager expects the plain dos form.
fn rm_path(path: &Path) -> io::Result<Vec<u16>> {
    let canonical = fs::canonicalize(path)?;
    let start = match canonical.components().next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) => 4,
        _ => 0,
    };
    let wide: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    Ok(wide[start..].iter().copied().chain(once(0)).collect())
}

/// sets a fixed-size information class. `class` must be the one `info`'s type belongs to.
fn set_info<T>(file: &File, class: i32, info: &T) -> io::Result<()> {
    // safety: the handle is valid while `file` is borrowed; info is live and its exact size is passed.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            class,
            ptr::from_ref(info).cast(),
            size_of::<T>() as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn move_file_delay_until_reboot(wide: &[u16]) -> io::Result<()> {
    // safety: wide is nul-terminated; a null destination with MOVEFILE_DELAY_UNTIL_REBOOT
    // schedules the deletion instead of moving the file.
    let ok = unsafe { MoveFileExW(wide.as_ptr(), ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// nul-terminated UTF-16 form of `s`.
fn wide_nul(s: impl AsRef<OsStr>) -> Vec<u16> {
    let s: &OsStr = s.as_ref();
    s.encode_wide().chain(once(0)).collect()
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
