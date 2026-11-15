//! discovers allocated data ranges through the already checked file descriptor.

use std::fs::File;
use std::io;
use std::ops::Range;

#[cfg(target_os = "linux")]
pub(super) fn data_ranges(file: &File, len: u64) -> io::Result<Vec<Range<u64>>> {
    use nix::unistd::{Whence, lseek};

    discover(len, |offset, data| {
        let offset =
            i64::try_from(offset).map_err(|_| io::Error::other("file offset out of range"))?;
        let whence = if data {
            Whence::SeekData
        } else {
            Whence::SeekHole
        };
        lseek(file, offset, whence)
            .map(|n| n as u64)
            .map_err(io::Error::from)
    })
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
pub(super) fn data_ranges(_file: &File, len: u64) -> io::Result<Vec<Range<u64>>> {
    Ok(std::iter::once(0..len).collect())
}

#[cfg(target_os = "linux")]
fn discover(
    len: u64,
    mut seek: impl FnMut(u64, bool) -> io::Result<u64>,
) -> io::Result<Vec<Range<u64>>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while offset < len {
        super::check_interrupted()?;
        let data = match seek(offset, true) {
            Ok(data) => data,
            Err(err) if err.raw_os_error() == Some(libc::ENXIO) => break,
            Err(err) if unsupported(&err) => return Ok(std::iter::once(0..len).collect()),
            Err(err) => return Err(err),
        };
        if data < offset || data >= len {
            return Ok(std::iter::once(0..len).collect());
        }
        let hole = match seek(data, false) {
            Ok(hole) => hole,
            Err(err) if unsupported(&err) => return Ok(std::iter::once(0..len).collect()),
            Err(err) => return Err(err),
        };
        if hole <= data || hole > len {
            return Ok(std::iter::once(0..len).collect());
        }
        ranges.push(data..hole);
        offset = hole;
    }
    Ok(ranges)
}

#[cfg(target_os = "linux")]
fn unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn synthetic_ranges_and_fallbacks() {
        let mut answers = [
            Ok(10),
            Ok(20),
            Ok(40),
            Ok(50),
            Err(io::Error::from_raw_os_error(libc::ENXIO)),
        ]
        .into_iter();
        assert_eq!(
            discover(100, |_, _| answers.next().unwrap()).unwrap(),
            vec![10..20, 40..50]
        );
        for errno in [libc::EINVAL, libc::ENOSYS, libc::EOPNOTSUPP] {
            assert_eq!(
                discover(100, |_, _| Err(io::Error::from_raw_os_error(errno))).unwrap(),
                vec![0..100]
            );
            assert_eq!(
                discover(100, |_, data| if data {
                    Ok(10)
                } else {
                    Err(io::Error::from_raw_os_error(errno))
                })
                .unwrap(),
                vec![0..100]
            );
        }
        assert!(discover(100, |_, _| Err(io::Error::from_raw_os_error(libc::EIO))).is_err());
        for answers in [vec![0, 0], vec![101, 101], vec![10, 101], vec![10, 20, 15]] {
            let mut answers = answers.into_iter();
            assert_eq!(
                discover(100, |_, _| Ok(answers.next().unwrap())).unwrap(),
                vec![0..100]
            );
        }
    }

    #[test]
    fn sparse_overwrite_and_verify_whole_file() {
        use crate::erase::{Options, overwrite::overwrite};
        use crate::verify::{self, Expected, SEED_LEN};

        for populated in [false, true] {
            let mut file = tempfile::tempfile().unwrap();
            let len = 128 * 1024;
            file.set_len(len).unwrap();
            if populated {
                for offset in [16 * 1024, 64 * 1024] {
                    file.seek(SeekFrom::Start(offset)).unwrap();
                    file.write_all(&[0x55; 4096]).unwrap();
                }
            }
            file.sync_all().unwrap();
            let ranges = data_ranges(&file, len).unwrap();
            if populated {
                for offset in [16 * 1024, 64 * 1024] {
                    assert!(ranges.iter().any(|range| range.contains(&offset)));
                }
            }
            let blocks = file.metadata().unwrap().blocks();
            let data_bytes: u64 = ranges.iter().map(|range| range.end - range.start).sum();
            let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
            let ext4 = crate::storage::assessment_filesystem(std::path::Path::new(&descriptor))
                .as_deref()
                == Some("ext4");
            let expected = Expected::from_seed([7; SEED_LEN]);
            let opts = Options {
                passes: 1,
                zero: false,
                verbose: false,
                force: false,
                kill_holders: false,
                allow_protected: false,
                quiet: true,
                verify: true,
            };
            overwrite(&mut file, &ranges, &opts, Some(&expected)).unwrap();
            if ext4 && data_bytes < len {
                assert_eq!(file.metadata().unwrap().blocks(), blocks);
            }
            verify::check_ranges(&mut file, &expected, len, &ranges).unwrap();
            if populated {
                file.seek(SeekFrom::Start(ranges[0].start)).unwrap();
                let mut data = [0; 4096];
                file.read_exact(&mut data).unwrap();
                assert_ne!(data, [0x55; 4096]);
                file.seek(SeekFrom::Start(ranges[0].start)).unwrap();
                file.write_all(&[data[0] ^ 0xff]).unwrap();
                assert!(verify::check_ranges(&mut file, &expected, len, &ranges).is_err());
                file.seek(SeekFrom::Start(ranges[0].start)).unwrap();
                file.write_all(&[data[0]]).unwrap();
            }
            file.seek(SeekFrom::Start(0)).unwrap();
            let mut original = Vec::new();
            file.read_to_end(&mut original).unwrap();
            file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(&[original[0] ^ 0xff]).unwrap();
            assert!(verify::check_ranges(&mut file, &expected, len, &ranges).is_err());
            file.seek(SeekFrom::Start(0)).unwrap();
            file.write_all(&[original[0]]).unwrap();
            file.set_len(len - 1).unwrap();
            assert!(verify::check_ranges(&mut file, &expected, len, &ranges).is_err());
            file.set_len(len).unwrap();
            file.seek(SeekFrom::Start(len - 1)).unwrap();
            file.write_all(&[original[len as usize - 1]]).unwrap();
            verify::check_ranges(&mut file, &expected, len, &ranges).unwrap();
            file.set_len(len + 1).unwrap();
            assert!(verify::check_ranges(&mut file, &expected, len, &ranges).is_err());
        }
    }
}
