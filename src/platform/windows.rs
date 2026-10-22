use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::iter::once;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_HANDLE_EOF, ERROR_MORE_DATA, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE,
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileDispositionInfoEx, GetDiskFreeSpaceW, GetVolumePathNameW,
    MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW, SetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_GET_RETRIEVAL_POINTERS, RETRIEVAL_POINTERS_BUFFER, RETRIEVAL_POINTERS_BUFFER_0,
    STARTING_VCN_INPUT_BUFFER,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmForceShutdown, RmGetList,
    RmRegisterResources, RmShutdown, RmStartSession,
};

/// access for `open_for_erase`: read and write to overwrite, delete to unlink later.
const ERASE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;
/// full sharing, so our handles never block other normal handles.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RETRIEVAL_BUF_LEN: usize = 4096;
const RM_LIST_ATTEMPTS: usize = 3;
/// lcn value that marks a sparse hole in fsctl output.
const HOLE_LCN: i64 = -1;

const STARTING_VCN_OFFSET: usize = offset_of!(RETRIEVAL_POINTERS_BUFFER, StartingVcn);
const EXTENTS_OFFSET: usize = offset_of!(RETRIEVAL_POINTERS_BUFFER, Extents);
const EXTENT_SIZE: usize = size_of::<RETRIEVAL_POINTERS_BUFFER_0>();
const NEXT_VCN_OFFSET: usize = offset_of!(RETRIEVAL_POINTERS_BUFFER_0, NextVcn);
const LCN_OFFSET: usize = offset_of!(RETRIEVAL_POINTERS_BUFFER_0, Lcn);

/// a process that holds a file open, as reported by the restart manager.
pub struct Holder {
    pub pid: u32,
    pub name: String,
}

/// a byte range on a volume, offset and length in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

/// where a file's data lives on disk.
pub struct PhysicalLayout {
    /// volume path to write to, for example `\\.\C:`.
    pub volume: PathBuf,
    /// cluster-aligned byte ranges on that volume that hold the file's data. sparse holes are skipped.
    pub ranges: Vec<ByteRange>,
}

/// one fsctl extent: vcns up to `next_vcn` map to clusters starting at `lcn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    next_vcn: i64,
    lcn: i64,
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

/// physical layout of `path`'s data on its volume.
///
/// covers the default data stream only. compressed and sparse files are refused,
/// and so is a non-empty file with no extents (resident in the mft).
pub fn physical_layout(path: &Path) -> io::Result<PhysicalLayout> {
    let canonical = fs::canonicalize(path)?;
    let letter = drive_letter(&canonical)?;
    check_drive_root(&canonical, letter)?;
    let cluster = cluster_bytes(letter)?;
    // attribute-only access with full sharing: the handle must not block the process that locks the file.
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(SHARE_ALL)
        .open(&canonical)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(unsupported(
            "link, junction, dedup or cloud file: its data is not mapped here",
        ));
    }
    if crate::storage::shares_clusters(&canonical) {
        return Err(unsupported(
            "refs file: its clusters may be shared with other files",
        ));
    }
    if meta.file_attributes() & (FILE_ATTRIBUTE_COMPRESSED | FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
        return Err(unsupported(
            "compressed or sparse file: on-disk layout is not mapped",
        ));
    }
    let ranges = if meta.len() == 0 {
        Vec::new()
    } else {
        collect_ranges(&file, cluster)?
    };
    Ok(PhysicalLayout {
        volume: PathBuf::from(format!(r"\\.\{letter}:")),
        ranges,
    })
}

/// opens a volume such as `\\.\C:` for raw read and write. needs administrator rights.
pub fn open_volume_for_write(volume: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(volume)
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

/// bytes per cluster of the volume with drive letter `letter`.
fn cluster_bytes(letter: char) -> io::Result<u64> {
    let root = wide_nul(format!("{letter}:\\"));
    let mut sectors_per_cluster = 0u32;
    let mut bytes_per_sector = 0u32;
    let mut free_clusters = 0u32;
    let mut total_clusters = 0u32;
    // safety: root is nul-terminated; all four out pointers are valid for writes.
    let ok = unsafe {
        GetDiskFreeSpaceW(
            root.as_ptr(),
            &mut sectors_per_cluster,
            &mut bytes_per_sector,
            &mut free_clusters,
            &mut total_clusters,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(sectors_per_cluster) * u64::from(bytes_per_sector))
}

/// fails unless `path` is on the volume mounted at `letter:\`. A volume mounted in a folder
/// keeps the drive letter in its path but has other clusters, so raw writes would hit the wrong disk.
fn check_drive_root(path: &Path, letter: char) -> io::Result<()> {
    let wide = rm_path(path)?;
    let mut root = [0u16; 512];
    // safety: wide is nul-terminated and root is writable for the length passed.
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let len = root.iter().position(|&c| c == 0).unwrap_or(root.len());
    if String::from_utf16_lossy(&root[..len]).eq_ignore_ascii_case(&format!("{letter}:\\")) {
        Ok(())
    } else {
        Err(unsupported(
            "file is on a volume mounted in a folder; only drive-letter volumes are supported",
        ))
    }
}

/// drive letter of a local path, from its canonical form. unc and device paths are rejected.
fn drive_letter(path: &Path) -> io::Result<char> {
    let not_local = || unsupported("only local drive paths (C:\\...) are supported");
    let letter = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => letter,
            _ => return Err(not_local()),
        },
        _ => return Err(not_local()),
    };
    Ok(char::from(letter.to_ascii_uppercase()))
}

/// walks the extents of `file` from vcn 0, following ERROR_MORE_DATA. fails if there are no extents.
fn collect_ranges(file: &File, cluster: u64) -> io::Result<Vec<ByteRange>> {
    let mut ranges = Vec::new();
    let mut vcn = 0i64;
    let mut seen_extent = false;
    loop {
        let (status, buf) = fsctl_retrieval(file, vcn)?;
        if status == ERROR_HANDLE_EOF {
            break;
        }
        let (first_vcn, extents) = parse_retrieval(&buf)?;
        if first_vcn != vcn {
            return Err(invalid_data(
                "retrieval started at a different vcn than requested",
            ));
        }
        if extents.is_empty() {
            if status == ERROR_MORE_DATA {
                return Err(invalid_data("retrieval reported more data but no extents"));
            }
            break;
        }
        seen_extent = true;
        ranges_from_extents(&mut ranges, first_vcn, &extents, cluster)?;
        let last_vcn = extents.last().map_or(vcn, |extent| extent.next_vcn);
        if last_vcn <= vcn {
            return Err(invalid_data("retrieval made no progress"));
        }
        if status == ERROR_SUCCESS {
            break;
        }
        vcn = last_vcn;
    }
    if !seen_extent {
        return Err(unsupported(
            "file data is stored in the MFT (resident); cannot erase through the raw volume",
        ));
    }
    Ok(ranges)
}

/// runs FSCTL_GET_RETRIEVAL_POINTERS from `vcn`. returns the status (success, more data
/// or end of file) with the output buffer.
fn fsctl_retrieval(file: &File, vcn: i64) -> io::Result<(u32, Vec<u8>)> {
    let input = STARTING_VCN_INPUT_BUFFER { StartingVcn: vcn };
    let mut buf = vec![0u8; RETRIEVAL_BUF_LEN];
    let mut returned = 0u32;
    // safety: input and buf live for the call and their exact sizes are passed. A synchronous
    // handle needs a non-null bytes-returned pointer, and no overlapped is used.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_GET_RETRIEVAL_POINTERS,
            ptr::from_ref(&input).cast(),
            size_of::<STARTING_VCN_INPUT_BUFFER>() as u32,
            buf.as_mut_ptr().cast(),
            RETRIEVAL_BUF_LEN as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok != 0 {
        return Ok((ERROR_SUCCESS, buf));
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error().map(|code| code as u32) {
        Some(code @ (ERROR_MORE_DATA | ERROR_HANDLE_EOF)) => Ok((code, buf)),
        _ => Err(err),
    }
}

/// parses a RETRIEVAL_POINTERS_BUFFER into StartingVcn and its extents. entries past the
/// end of `buf` are dropped, so a short buffer never reads out of bounds.
fn parse_retrieval(buf: &[u8]) -> io::Result<(i64, Vec<Extent>)> {
    let truncated = || invalid_data("truncated retrieval buffer");
    let count = read_u32(buf, 0).ok_or_else(truncated)? as usize;
    let starting_vcn = read_i64(buf, STARTING_VCN_OFFSET).ok_or_else(truncated)?;
    let fit = buf.len().saturating_sub(EXTENTS_OFFSET) / EXTENT_SIZE;
    let mut extents = Vec::with_capacity(count.min(fit));
    for i in 0..count.min(fit) {
        let at = EXTENTS_OFFSET + i * EXTENT_SIZE;
        extents.push(Extent {
            next_vcn: read_i64(buf, at + NEXT_VCN_OFFSET).ok_or_else(truncated)?,
            lcn: read_i64(buf, at + LCN_OFFSET).ok_or_else(truncated)?,
        });
    }
    Ok((starting_vcn, extents))
}

/// converts extents to byte ranges and appends them to `out`. holes are skipped.
fn ranges_from_extents(
    out: &mut Vec<ByteRange>,
    first_vcn: i64,
    extents: &[Extent],
    cluster: u64,
) -> io::Result<()> {
    let mut vcn = first_vcn;
    for extent in extents {
        let clusters = extent
            .next_vcn
            .checked_sub(vcn)
            .and_then(|n| u64::try_from(n).ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| invalid_data("extent does not advance"))?;
        vcn = extent.next_vcn;
        if extent.lcn == HOLE_LCN {
            continue;
        }
        let lcn = u64::try_from(extent.lcn).map_err(|_| invalid_data("negative lcn"))?;
        let range = ByteRange {
            offset: lcn
                .checked_mul(cluster)
                .ok_or_else(|| invalid_data("byte offset overflow"))?,
            len: clusters
                .checked_mul(cluster)
                .ok_or_else(|| invalid_data("byte length overflow"))?,
        };
        push_range(out, range);
    }
    Ok(())
}

/// appends `range`, extending the last range when the two are contiguous on disk.
fn push_range(out: &mut Vec<ByteRange>, range: ByteRange) {
    match out.last_mut() {
        Some(last) if last.offset.checked_add(last.len) == Some(range.offset) => {
            last.len += range.len;
        }
        _ => out.push(range),
    }
}

fn read_array<const N: usize>(buf: &[u8], at: usize) -> Option<[u8; N]> {
    buf.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    read_array(buf, at).map(u32::from_le_bytes)
}

fn read_i64(buf: &[u8], at: usize) -> Option<i64> {
    read_array(buf, at).map(i64::from_le_bytes)
}

/// nUL-terminated UTF-16 form of `s`.
fn wide_nul(s: impl AsRef<OsStr>) -> Vec<u16> {
    let s: &OsStr = s.as_ref();
    s.encode_wide().chain(once(0)).collect()
}

fn invalid_data(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn unsupported(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retrieval_buffer(starting_vcn: i64, count: u32, extents: &[(i64, i64)]) -> Vec<u8> {
        let mut buf = vec![0u8; EXTENTS_OFFSET];
        buf[0..4].copy_from_slice(&count.to_le_bytes());
        buf[STARTING_VCN_OFFSET..STARTING_VCN_OFFSET + 8]
            .copy_from_slice(&starting_vcn.to_le_bytes());
        for (next_vcn, lcn) in extents {
            buf.extend_from_slice(&next_vcn.to_le_bytes());
            buf.extend_from_slice(&lcn.to_le_bytes());
        }
        buf
    }

    fn pairs(ranges: &[ByteRange]) -> Vec<(u64, u64)> {
        ranges.iter().map(|r| (r.offset, r.len)).collect()
    }

    #[test]
    fn layout_matches_fsctl_abi() {
        assert_eq!(STARTING_VCN_OFFSET, 8);
        assert_eq!(EXTENTS_OFFSET, 16);
        assert_eq!(EXTENT_SIZE, 16);
        assert_eq!(NEXT_VCN_OFFSET, 0);
        assert_eq!(LCN_OFFSET, 8);
    }

    #[test]
    fn parses_header_and_extents() {
        let buf = retrieval_buffer(0, 2, &[(4, 100), (10, -1)]);
        let (start, extents) = parse_retrieval(&buf).unwrap();
        assert_eq!(start, 0);
        assert_eq!(
            extents,
            vec![
                Extent {
                    next_vcn: 4,
                    lcn: 100
                },
                Extent {
                    next_vcn: 10,
                    lcn: -1
                },
            ]
        );
    }

    #[test]
    fn parse_clamps_entries_to_buffer() {
        let buf = retrieval_buffer(0, 5, &[(4, 100), (10, 200)]);
        let (_, extents) = parse_retrieval(&buf).unwrap();
        assert_eq!(extents.len(), 2);
    }

    #[test]
    fn parse_rejects_truncated_header() {
        assert!(parse_retrieval(&[0u8; 10]).is_err());
    }

    #[test]
    fn ranges_convert_clusters_to_bytes_and_skip_holes() {
        let mut out = Vec::new();
        let extents = [
            Extent {
                next_vcn: 3,
                lcn: 100,
            },
            Extent {
                next_vcn: 5,
                lcn: -1,
            },
            Extent {
                next_vcn: 8,
                lcn: 20,
            },
        ];
        ranges_from_extents(&mut out, 0, &extents, 4096).unwrap();
        assert_eq!(
            pairs(&out),
            vec![(100 * 4096, 3 * 4096), (20 * 4096, 3 * 4096)]
        );
    }

    #[test]
    fn ranges_merge_contiguous_extents() {
        let mut out = Vec::new();
        let extents = [
            Extent {
                next_vcn: 2,
                lcn: 10,
            },
            Extent {
                next_vcn: 4,
                lcn: 12,
            },
        ];
        ranges_from_extents(&mut out, 0, &extents, 512).unwrap();
        assert_eq!(pairs(&out), vec![(10 * 512, 4 * 512)]);
    }

    #[test]
    fn ranges_start_from_given_vcn() {
        let mut out = Vec::new();
        let extents = [Extent {
            next_vcn: 9,
            lcn: 50,
        }];
        ranges_from_extents(&mut out, 7, &extents, 4096).unwrap();
        assert_eq!(pairs(&out), vec![(50 * 4096, 2 * 4096)]);
    }

    #[test]
    fn ranges_reject_non_advancing_extent() {
        let mut out = Vec::new();
        let extents = [Extent {
            next_vcn: 0,
            lcn: 5,
        }];
        assert!(ranges_from_extents(&mut out, 0, &extents, 4096).is_err());
    }

    #[test]
    fn drive_letter_from_verbatim_path() {
        assert_eq!(drive_letter(Path::new(r"\\?\c:\data\f.bin")).unwrap(), 'C');
    }

    #[test]
    fn unc_path_is_not_a_local_drive() {
        assert!(drive_letter(Path::new(r"\\?\UNC\server\share\f.bin")).is_err());
        assert!(drive_letter(Path::new(r"\\server\share\f.bin")).is_err());
    }
}
