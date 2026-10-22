use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::iter::once;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, Prefix};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileDispositionInfoEx, MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmForceShutdown, RmGetList,
    RmRegisterResources, RmShutdown, RmStartSession,
};
use windows_sys::core::BOOL;

use crate::erase;

/// access for `open_for_erase`: read and write to overwrite, delete to unlink later.
const ERASE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;
/// full sharing, so our handles never block other normal handles.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RM_LIST_ATTEMPTS: usize = 3;

/// a process that holds a file open, as reported by the restart manager.
pub struct Holder {
    pub pid: u32,
    pub name: String,
}

/// processes holding `path` open, according to the restart manager. read-only.
pub fn holders(path: &Path) -> io::Result<Vec<Holder>> {
    let session = registered_session(path)?;
    Ok(session.process_list()?.iter().map(holder_from).collect())
}

/// force-closes the processes returned by `holders(path)` and returns them.
pub fn close_holders(path: &Path) -> io::Result<Vec<Holder>> {
    let session = registered_session(path)?;
    let holders: Vec<Holder> = session.process_list()?.iter().map(holder_from).collect();
    rm_force_shutdown(&session)?;
    Ok(holders)
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

/// removes the directory entry at once. the file must have been opened with delete access.
///
/// posix semantics unlink the name immediately, so the path can be reused while
/// the data stays alive until the last handle closes.
pub fn posix_delete(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    set_disposition(file, &info)
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
    Holder {
        pid: info.Process.dwProcessId,
        name: String::from_utf16_lossy(&info.strAppName[..len]),
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

fn set_disposition(file: &File, info: &FILE_DISPOSITION_INFO_EX) -> io::Result<()> {
    // safety: the handle is valid while `file` is borrowed; info is live and its exact size is passed.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            ptr::from_ref(info).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
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

/// registers or removes the console handler that turns ctrl+c, ctrl+break and closing the console
/// into the erase interrupt flag.
pub fn set_interrupt_handler(on: bool) -> io::Result<()> {
    // safety: console_handler is a plain function that lives for the whole process. removing it
    // passes the same routine with add set to false.
    let ok = unsafe { SetConsoleCtrlHandler(Some(console_handler), BOOL::from(on)) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// runs on a thread that windows creates. the first event sets the flag; a second one exits at once,
/// because a write may be half-way through a file.
extern "system" fn console_handler(ctrl_type: u32) -> BOOL {
    if !matches!(
        ctrl_type,
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT
    ) {
        return 0;
    }
    if erase::raise_interrupt() {
        std::process::exit(130);
    }
    1
}

/// nul-terminated UTF-16 form of `s`.
fn wide_nul(s: impl AsRef<OsStr>) -> Vec<u16> {
    let s: &OsStr = s.as_ref();
    s.encode_wide().chain(once(0)).collect()
}
