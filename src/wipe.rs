//! erases the free space of a filesystem: one pass per pattern, each pass fills the free blocks
//! with temporary files that disappear by themselves.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::erase::{self, Options, Pattern, pattern_for_pass};
use crate::preview::human_bytes;
use crate::progress::Progress;

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
    // the size is only known when a limit is given (tests); a wipe runs until the volume is full.
    let total = limit.map(|max| max.saturating_mul(u64::from(opts.passes)));
    let mut progress = Progress::new(total, opts.quiet);
    let result = wipe_each_pass(dir, opts, limit, &mut progress);
    progress.finish();
    result
}

fn wipe_each_pass(
    dir: &Path,
    opts: &Options,
    limit: Option<u64>,
    progress: &mut Progress,
) -> io::Result<u64> {
    let mut total = 0u64;
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            progress.finish();
            crate::ui::message!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        let written = fill_pass(dir, pattern, limit, opts.verbose, progress)?;
        total += written;
        if opts.verbose {
            progress.finish();
            crate::ui::message!(
                "  pass {}/{} done: {} written",
                pass + 1,
                opts.passes,
                human_bytes(written)
            );
        }
    }
    Ok(total)
}

/// fills the free space with one pattern, using more files when a file reaches its size limit.
/// returns the bytes written in this pass.
fn fill_pass(
    dir: &Path,
    pattern: Pattern,
    limit: Option<u64>,
    verbose: bool,
    progress: &mut Progress,
) -> io::Result<u64> {
    #[cfg(windows)]
    // fat32 reports disk full when a write crosses its per-file limit, even with free space left.
    let file_limit = match crate::storage::assessment_filesystem(dir).as_deref() {
        Some("fat32") | None => Some(4 * GIB - CHUNK as u64),
        _ => None,
    };
    #[cfg(not(windows))]
    let file_limit = None;
    fill_pass_with(limit, |remaining| {
        fill_file(dir, pattern, remaining, file_limit, verbose, progress)
    })
}

fn fill_pass_with(
    limit: Option<u64>,
    mut fill_file: impl FnMut(Option<u64>) -> io::Result<Fill>,
) -> io::Result<u64> {
    let mut total = 0u64;
    // closing an earlier file here would let the allocator reuse its blocks instead of filling new ones.
    let mut files = Vec::new();
    loop {
        let remaining = limit.map(|max| max - total);
        if remaining == Some(0) {
            break;
        }
        let fill = match fill_file(remaining) {
            Err(err) if total > 0 && is_out_of_space(&err) => break,
            result => result?,
        };
        total += fill.written;
        files.push(fill.file);
        if !fill.file_too_large || fill.written == 0 {
            break;
        }
    }
    Ok(total)
}

struct Fill {
    file: File,
    written: u64,
    file_too_large: bool,
}

/// writes `pattern` into a new temporary file until the volume is full, the file reaches the
/// filesystem's size limit, or `limit` bytes are written. the caller keeps the file until the pass ends.
fn fill_file(
    dir: &Path,
    pattern: Pattern,
    limit: Option<u64>,
    file_limit: Option<u64>,
    verbose: bool,
    progress: &mut Progress,
) -> io::Result<Fill> {
    let path = dir.join(format!(".fill-{:016x}", rand::random::<u64>()));
    let mut file = create_temp(&path)?;
    let mut buf = vec![0u8; CHUNK];
    let mut written = 0u64;
    let mut reported_gib = 0u64;
    let mut file_too_large = false;

    loop {
        erase::check_interrupted()?;
        let remaining = limit
            .unwrap_or(u64::MAX)
            .min(file_limit.unwrap_or(u64::MAX))
            - written;
        let want = remaining.min(CHUNK as u64) as usize;
        if want == 0 {
            file_too_large = file_limit == Some(written) && limit != Some(written);
            break;
        }
        erase::fill(&mut buf[..want], pattern);
        match write_chunk(&mut file, &buf[..want]) {
            Ok(n) => {
                written += n as u64;
                progress.add(n as u64);
                // a long fill runs for hours on big disks; show that it is moving.
                if verbose && written / GIB > reported_gib {
                    reported_gib = written / GIB;
                    progress.finish();
                    crate::ui::message!("    {} written", human_bytes(written));
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
            file,
            written,
            file_too_large,
        }),
    }
}

fn write_chunk(file: &mut impl Write, buf: &[u8]) -> io::Result<usize> {
    match file.write(buf) {
        Ok(0) => Err(io::ErrorKind::WriteZero.into()),
        result => result,
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

    // std rejects create_new without write(true) before it looks at access_mode, which sets the real rights.
    OpenOptions::new()
        .write(true)
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

    #[test]
    fn zero_write_is_an_error() {
        struct ZeroWriter;
        impl Write for ZeroWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Ok(0)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_chunk(&mut ZeroWriter, b"x").unwrap_err().kind(),
            io::ErrorKind::WriteZero
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn files_remain_open_until_pass_ends_or_fails() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        for failure in [libc::ENOSPC, libc::EIO, libc::EINTR, libc::EMFILE] {
            let mut originals = Vec::new();
            let mut clones = Vec::new();
            let mut calls = 0;
            let result = fill_pass_with(None, |_| {
                for (fd, dev, ino) in &originals {
                    let meta = fs::metadata(format!("/proc/self/fd/{fd}")).unwrap();
                    assert_eq!((meta.dev(), meta.ino()), (*dev, *ino));
                }
                calls += 1;
                if calls == 3 {
                    return Err(io::Error::from_raw_os_error(failure));
                }
                let mut file = tempfile::tempfile_in(dir.path()).unwrap();
                file.write_all(b"x").unwrap();
                let meta = file.metadata().unwrap();
                originals.push((file.as_raw_fd(), meta.dev(), meta.ino()));
                clones.push(file.try_clone().unwrap());
                Ok(Fill {
                    file,
                    written: 1,
                    file_too_large: true,
                })
            });
            if failure == libc::ENOSPC {
                assert_eq!(result.unwrap(), 2);
            } else {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(failure));
            }
            assert_eq!(calls, 3);
            for (fd, dev, ino) in originals {
                let same = fs::metadata(format!("/proc/self/fd/{fd}"))
                    .is_ok_and(|meta| (meta.dev(), meta.ino()) == (dev, ino));
                assert!(!same);
            }
            drop(clones);
        }
    }

    #[test]
    fn zero_progress_stops_pass() {
        let dir = tempfile::tempdir().unwrap();
        let mut calls = 0;
        let written = fill_pass_with(None, |_| {
            calls += 1;
            Ok(Fill {
                file: tempfile::tempfile_in(dir.path()).unwrap(),
                written: 0,
                file_too_large: true,
            })
        })
        .unwrap();
        assert_eq!((written, calls), (0, 1));
    }

    #[test]
    fn first_allocation_storage_full_is_an_error() {
        let error = fill_pass_with(None, |_| Err(io::ErrorKind::StorageFull.into())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    }

    fn options(passes: u32) -> Options {
        Options {
            passes,
            verbose: false,
            force: true,
            kill_holders: false,
            allow_protected: false,
            quiet: true,
            verify: false,
        }
    }

    #[test]
    fn fill_stops_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let limit = 2 * CHUNK as u64 + 5;

        let fill = fill_file(
            dir.path(),
            Pattern::Zeros,
            Some(limit),
            None,
            false,
            &mut Progress::new(None, true),
        )
        .unwrap();

        assert_eq!(fill.written, limit);
        assert!(!fill.file_too_large);
    }

    #[test]
    fn bounded_files_roll_over_until_pass_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut progress = Progress::new(None, true);
        let mut calls = 0;
        let written = fill_pass_with(Some(3 * MIB), |remaining| {
            calls += 1;
            fill_file(
                dir.path(),
                Pattern::Zeros,
                remaining,
                Some(2 * MIB),
                false,
                &mut progress,
            )
        })
        .unwrap();
        assert_eq!((written, calls), (3 * MIB, 2));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn passes_leave_no_fill_file_behind() {
        let dir = tempfile::tempdir().unwrap();

        wipe_passes(dir.path(), &options(3), Some(3 * MIB)).unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
