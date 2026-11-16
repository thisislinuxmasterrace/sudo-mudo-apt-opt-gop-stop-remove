use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

/// profile directory of the account that started sudo.exe, read while the process ancestry is still
/// known. empty when the ancestry check did not run or the profile could not be resolved.
static INVOKING_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();

/// the invoking account's profile directory, which differs from `%USERPROFILE%` when sudo elevated
/// another account. `None` when unknown: callers then protect `%USERPROFILE%` only.
pub(crate) fn invoking_home() -> Option<PathBuf> {
    INVOKING_HOME.get().cloned().flatten()
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // safety: this wrapper owns a successfully opened, non-pseudo handle.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn check() -> Result<(), String> {
    check_native_sudo().map_err(|err| {
        format!("start an elevated console, then use native system sudo.exe; launch could not be verified: {err}")
    })
}

fn check_native_sudo() -> io::Result<()> {
    if !elevated()? {
        return Err(io::Error::other("process is not elevated"));
    }
    let expected = system_sudo()?;
    let parents = process_parents()?;
    let mut pid = std::process::id();
    let mut seen = HashSet::new();
    // node and command shims can sit between sudo and the binary; bound the ancestry walk.
    for _ in 0..32 {
        let Some(parent) = parents.get(&pid).copied().filter(|parent| *parent != 0) else {
            break;
        };
        if !seen.insert(parent) {
            break;
        }
        if process_image(parent)?.eq_ignore_ascii_case(&expected) {
            // the process that started sudo.exe runs as the account that asked for the elevation.
            let starter = parents.get(&parent).copied().filter(|pid| *pid != 0);
            let _ = INVOKING_HOME.set(starter.and_then(|pid| profile_of(pid).ok()));
            return Ok(());
        }
        pid = parent;
    }
    Err(io::Error::other(
        "native sudo ancestry was not found; use windows 11 24h2 or later with inline sudo",
    ))
}

/// profile directory of the account that owns process `pid`.
fn profile_of(pid: u32) -> io::Result<PathBuf> {
    // safety: this opens an existing process for a limited query; no handles are inherited.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(raw);
    let mut token = ptr::null_mut();
    // safety: process is valid and token points to writable storage.
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    // safety: token is valid and path and len are writable, len being the buffer size in characters.
    if unsafe { GetUserProfileDirectoryW(token.0, path.as_mut_ptr(), &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Ok(PathBuf::from(OsString::from_wide(&path[..end])))
}

fn elevated() -> io::Result<bool> {
    let mut token = ptr::null_mut();
    // safety: GetCurrentProcess returns a valid pseudo handle; token points to writable storage.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    // safety: elevation and returned are writable and their sizes match the requested information.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn system_sudo() -> io::Result<String> {
    let mut path = vec![0u16; 32768];
    // safety: path is a writable buffer of the declared length.
    let len = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    if len >= path.len() {
        return Err(io::Error::other("system directory path is too long"));
    }
    let directory = String::from_utf16(&path[..len])
        .map_err(|_| io::Error::other("system directory path is not valid unicode"))?;
    Ok(format!("{directory}\\sudo.exe"))
}

fn process_parents() -> io::Result<HashMap<u32, u32>> {
    // safety: this requests a read-only process snapshot with no pointer arguments.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Handle(raw);
    // safety: PROCESSENTRY32W is a plain data structure whose all-zero representation is valid.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut parents = HashMap::new();
    // safety: the snapshot is valid and entry has the required size and writable storage.
    let mut found = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while found != 0 {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        // safety: the snapshot and initialized entry remain valid for enumeration.
        found = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(error);
    }
    Ok(parents)
}

fn process_image(pid: u32) -> io::Result<String> {
    // safety: this opens an existing process for a read-only image query; no handles are inherited.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(raw);
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    // safety: process is valid and path and len are writable buffers of the declared sizes.
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    String::from_utf16(&path[..len as usize])
        .map_err(|_| io::Error::other("ancestor process path is not valid unicode"))
}
