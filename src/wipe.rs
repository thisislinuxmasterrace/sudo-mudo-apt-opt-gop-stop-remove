//! erases the free space of a filesystem: one pass per pattern, each pass fills the free blocks
//! with temporary files that disappear by themselves.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::erase::{self, Options, Pattern, pattern_for_pass};
use crate::preview::human_bytes;

const CHUNK: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// wipes the free space visible to `dir`, `opts.passes` times.
///
/// a pass writes its pattern into new blocks until the volume is full, then removes the file.
/// the removed data stays in the free blocks, so each pass replaces the previous one. nothing is
/// overwritten in place, so this also works on copy-on-write filesystems.
///
/// blocks reserved for root (ext4 reserves 5% by default) are not free for ordinary users and are
/// left alone unless this runs as root.
pub fn wipe_free_space(dir: &Path, opts: &Options) -> io::Result<u64> {
    wipe_passes(dir, opts, None)
}

fn wipe_passes(dir: &Path, opts: &Options, limit: Option<u64>) -> io::Result<u64> {
    let mut total = 0u64;
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        let written = fill_pass(dir, pattern, limit, opts.verbose)?;
        total += written;
        if opts.verbose {
            eprintln!(
                "  pass {}/{} done: {} written",
                pass + 1,
                opts.passes,
                human_bytes(written)
            );
        }
    }
    Ok(total)
}

/// fills the free space with one pattern. a file that hits the filesystem's size limit (fat32)
/// is followed by another file, until the volume is full or `limit` bytes are written.
/// returns the bytes written in this pass.
fn fill_pass(dir: &Path, pattern: Pattern, limit: Option<u64>, verbose: bool) -> io::Result<u64> {
    let mut total = 0u64;
    loop {
        let remaining = limit.map(|max| max - total);
        if remaining == Some(0) {
            break;
        }
        let fill = fill_file(dir, pattern, remaining, verbose)?;
        total += fill.written;
        if !fill.file_too_large || fill.written == 0 {
            break;
        }
    }
    Ok(total)
}

struct Fill {
    written: u64,
    file_too_large: bool,
}

/// writes `pattern` into a new temporary file until the volume is full, the file reaches the
/// filesystem's size limit, or `limit` bytes are written. the file is gone when this returns.
fn fill_file(dir: &Path, pattern: Pattern, limit: Option<u64>, verbose: bool) -> io::Result<Fill> {
    let path = dir.join(format!(".fill-{:016x}", rand::random::<u64>()));
    let mut file = create_temp(&path)?;
    let mut buf = vec![0u8; CHUNK];
    let mut written = 0u64;
    let mut reported_gib = 0u64;
    let mut file_too_large = false;

    loop {
        let want = match limit {
            Some(max) => (max - written).min(CHUNK as u64) as usize,
            None => CHUNK,
        };
        if want == 0 {
            break;
        }
        erase::fill(&mut buf[..want], pattern);
        match file.write(&buf[..want]) {
            Ok(0) => break,
            Ok(n) => {
                written += n as u64;
                // a long fill runs for hours on big disks; show that it is moving.
                if verbose && written / GIB > reported_gib {
                    reported_gib = written / GIB;
                    eprintln!("    {} written", human_bytes(written));
                }
            }
            Err(err) if is_out_of_space(&err) => break,
            Err(err) if is_file_too_large(&err) => {
                file_too_large = true;
                break;
            }
            Err(err) => return Err(err),
        }
    }

    // the data must reach the disk before the file goes away. the tail that got no blocks is
    // dropped, which is fine: there was no free space there to wipe.
    match file.sync_all() {
        Err(err) if !is_out_of_space(&err) => Err(err),
        _ => Ok(Fill {
            written,
            file_too_large,
        }),
    }
}

/// creates the temporary file so that it cannot outlive the process: on unix it is unlinked
/// right away, on windows it is deleted when the last handle closes.
#[cfg(unix)]
fn create_temp(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    std::fs::remove_file(path)
        .map_err(|err| io::Error::other(format!("{err}; leftover file {}", path.display())))?;
    Ok(file)
}

#[cfg(windows)]
fn create_temp(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_DELETE_ON_CLOSE};

    OpenOptions::new()
        .access_mode(GENERIC_WRITE | DELETE)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(target_os = "linux")]
const QUOTA_EXCEEDED: i32 = 122; // edquot

#[cfg(all(unix, not(target_os = "linux")))]
const QUOTA_EXCEEDED: i32 = 69; // edquot on macos and the bsds

#[cfg(unix)]
fn is_out_of_space(err: &io::Error) -> bool {
    // enospc, or the user's quota is used up
    err.kind() == io::ErrorKind::StorageFull
        || err
            .raw_os_error()
            .is_some_and(|code| code == 28 || code == QUOTA_EXCEEDED)
}

#[cfg(unix)]
fn is_file_too_large(err: &io::Error) -> bool {
    // efbig is 27 on linux and macos
    err.raw_os_error() == Some(27)
}

#[cfg(windows)]
fn is_out_of_space(err: &io::Error) -> bool {
    // error_disk_full, error_handle_disk_full and error_disk_quota_exceeded
    err.kind() == io::ErrorKind::StorageFull || matches!(err.raw_os_error(), Some(112 | 39 | 1295))
}

#[cfg(windows)]
fn is_file_too_large(_err: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn options(passes: u32) -> Options {
        Options {
            passes,
            verbose: false,
            force: true,
            kill_holders: false,
            raw_volume: false,
        }
    }

    #[test]
    fn fill_stops_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let limit = 2 * CHUNK as u64 + 5;

        let fill = fill_file(dir.path(), Pattern::Zeros, Some(limit), false).unwrap();

        assert_eq!(fill.written, limit);
        assert!(!fill.file_too_large);
    }

    #[test]
    fn passes_leave_no_fill_file_behind() {
        let dir = tempfile::tempdir().unwrap();

        wipe_passes(dir.path(), &options(3), Some(3 * MIB)).unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
