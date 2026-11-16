//! one fill pass: temporary files written until the volume is full, a size bound is crossed or a limit is reached.

use std::fs::File;
use std::io::{self, Write};

use super::{Directory, temp};
use crate::erase::{self, Pattern};
use crate::preview::human_bytes;
use crate::progress::Progress;

const CHUNK: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// how a pass ended.
pub(super) struct Done {
    /// bytes written in the pass.
    pub written: u64,
    /// the pass wrote more than `bound`: the data did not land in free blocks.
    pub exceeded: bool,
}

/// fills the free space with one pattern, using more files when a file reaches its size limit.
/// `bound` is the volume size: writing past it proves the filesystem does not store the data as written.
pub(super) fn fill_pass(
    dir: &Directory,
    pattern: Pattern,
    limit: Option<u64>,
    bound: Option<u64>,
    verbose: bool,
    progress: &mut Progress,
) -> io::Result<Done> {
    #[cfg(windows)]
    // fat32 reports disk full when a write crosses its per-file limit, even with free space left.
    let file_limit = match crate::storage::assessment_filesystem(&dir.path).as_deref() {
        Some("fat32") | None => Some(4 * GIB - CHUNK as u64),
        _ => None,
    };
    #[cfg(not(windows))]
    let file_limit = None;
    fill_pass_with(limit, bound, |remaining| {
        fill_file(dir, pattern, remaining, file_limit, verbose, progress)
    })
}

fn fill_pass_with(
    limit: Option<u64>,
    bound: Option<u64>,
    mut fill_file: impl FnMut(Option<u64>) -> io::Result<Fill>,
) -> io::Result<Done> {
    // one byte past the bound is enough to prove the pass went beyond the volume.
    let past_bound = bound.map(|size| size.saturating_add(1));
    let cap = match (limit, past_bound) {
        (Some(limit), Some(past)) => Some(limit.min(past)),
        (limit, past) => limit.or(past),
    };
    let mut total = 0u64;
    // closing an earlier file here would let the allocator reuse its blocks instead of filling new ones.
    let mut files = Vec::new();
    loop {
        let remaining = cap.map(|max| max - total);
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
    Ok(Done {
        written: total,
        exceeded: bound.is_some_and(|size| total > size),
    })
}

struct Fill {
    file: File,
    written: u64,
    file_too_large: bool,
}

/// writes `pattern` into a new temporary file until the volume is full, the file reaches the
/// filesystem's size limit, or `limit` bytes are written. the caller keeps the file until the pass ends.
fn fill_file(
    dir: &Directory,
    pattern: Pattern,
    limit: Option<u64>,
    file_limit: Option<u64>,
    verbose: bool,
    progress: &mut Progress,
) -> io::Result<Fill> {
    dir.check()?;
    let mut file = temp::create(dir)?;
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

    #[cfg(target_os = "linux")]
    #[test]
    fn bounded_fill_stays_on_selected_directory_after_symlink_change() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        let replacement = dir.path().join("replacement");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&replacement).unwrap();
        let link = dir.path().join("link");
        symlink(&original, &link).unwrap();
        let selected = Directory::open(&link, false).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&replacement, &link).unwrap();
        let fill = fill_file(
            &selected,
            Pattern::Zeros,
            Some(21),
            None,
            false,
            &mut Progress::new(None, true),
        )
        .unwrap();
        assert_eq!(fill.written, 21);
        let location = fs::read_link(format!("/proc/self/fd/{}", fill.file.as_raw_fd())).unwrap();
        assert_eq!(location.parent(), Some(original.as_path()));
        assert_eq!(fs::read_dir(&replacement).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&original).unwrap().count(), 0);
    }

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
            let result = fill_pass_with(None, None, |_| {
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
                assert_eq!(result.unwrap().written, 2);
            } else {
                assert_eq!(result.err().unwrap().raw_os_error(), Some(failure));
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
        let done = fill_pass_with(None, None, |_| {
            calls += 1;
            Ok(Fill {
                file: tempfile::tempfile_in(dir.path()).unwrap(),
                written: 0,
                file_too_large: true,
            })
        })
        .unwrap();
        assert_eq!((done.written, calls), (0, 1));
    }

    #[test]
    fn first_allocation_storage_full_is_an_error() {
        let error = fill_pass_with(None, None, |_| Err(io::ErrorKind::StorageFull.into()))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    }

    #[test]
    fn fill_stops_at_limit() {
        let dir = tempfile::tempdir().unwrap();
        let limit = 2 * CHUNK as u64 + 5;

        let fill = fill_file(
            &Directory::open(dir.path(), false).unwrap(),
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
        let done = fill_pass_with(Some(3 * MIB), None, |remaining| {
            calls += 1;
            fill_file(
                &Directory::open(dir.path(), false).unwrap(),
                Pattern::Zeros,
                remaining,
                Some(2 * MIB),
                false,
                &mut progress,
            )
        })
        .unwrap();
        assert_eq!((done.written, calls), (3 * MIB, 2));
        assert!(!done.exceeded);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn writing_past_the_volume_size_stops_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        let mut progress = Progress::new(None, true);
        let done = fill_pass_with(None, Some(2 * MIB), |remaining| {
            fill_file(
                &Directory::open(dir.path(), false).unwrap(),
                Pattern::Random,
                remaining,
                None,
                false,
                &mut progress,
            )
        })
        .unwrap();
        assert!(done.exceeded);
        assert_eq!(done.written, 2 * MIB + 1);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_pass_that_fits_the_volume_is_not_exceeded() {
        let done = fill_pass_with(Some(MIB), Some(2 * MIB), |remaining| {
            Ok(Fill {
                file: tempfile::tempfile().unwrap(),
                written: remaining.unwrap(),
                file_too_large: false,
            })
        })
        .unwrap();
        assert_eq!(done.written, MIB);
        assert!(!done.exceeded);
    }
}
