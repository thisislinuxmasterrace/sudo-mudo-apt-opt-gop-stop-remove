//! `--verify`: the last overwrite pass writes bytes that are reproduced from a random seed, so after the
//! pass the file can be read back and compared with them, without keeping a copy of the data.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use rand::RngCore;

use crate::erase::check_interrupted;

pub(crate) const SEED_LEN: usize = 32;
const BUFFER_SIZE: usize = 1024 * 1024;
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// the bytes the last pass should have written. byte `i` depends only on the seed and `i`, so the check
/// can read the file in chunks, in any order.
///
/// this is not a cryptographic generator, and erasing does not need one: the bytes only have to differ
/// from the old contents. the seed is random for each file, lives in memory during one erase and is
/// never written anywhere.
pub struct Expected {
    key: [u64; 4],
}

impl Expected {
    pub fn new() -> Self {
        let mut seed = [0u8; SEED_LEN];
        rand::rng().fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    pub(crate) fn from_seed(seed: [u8; SEED_LEN]) -> Self {
        let (words, _) = seed.as_chunks::<8>();
        let mut key = [0u64; 4];
        for (word, bytes) in key.iter_mut().zip(words) {
            *word = u64::from_le_bytes(*bytes);
        }
        Self { key }
    }

    /// fills `buf` with the bytes at file offsets `offset..offset + buf.len()`.
    pub fn fill_at(&self, offset: u64, buf: &mut [u8]) {
        let mut position = offset;
        let mut done = 0;
        while done < buf.len() {
            let word = self.word(position / 8).to_le_bytes();
            let skip = (position % 8) as usize;
            let take = (8 - skip).min(buf.len() - done);
            buf[done..done + take].copy_from_slice(&word[skip..skip + take]);
            done += take;
            position += take as u64;
        }
    }

    /// the 64-bit word with index `index`: the key is mixed in four times, so every key word matters.
    fn word(&self, index: u64) -> u64 {
        let [a, b, c, d] = self.key;
        let mut z = mix(a.wrapping_add(index.wrapping_mul(GAMMA)));
        for word in [b, c, d] {
            z = mix(z ^ word);
        }
        z
    }
}

/// the splitmix64 finalizer: a bijection on u64 that spreads every input bit over the output.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// compares the first `len` bytes of `file` with `expected`. it runs right after the last pass, through the
/// same handle. the caller keeps the file under its name when this fails, and reports the error.
pub fn check(file: &mut File, expected: &Expected, len: u64) -> io::Result<()> {
    match first_difference(file, expected, len) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "verify failed: contents differ after the last pass",
        )),
        Err(err) if err.kind() == io::ErrorKind::Interrupted => Err(err),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("verify could not read the file back: {err}"),
        )),
    }
}

/// offset of the first byte that differs from `expected`, or `None` when the file matches. a file that is
/// shorter or longer than `len` differs at its first missing or extra byte.
fn first_difference(file: &mut File, expected: &Expected, len: u64) -> io::Result<Option<u64>> {
    drop_cache(file);
    file.seek(SeekFrom::Start(0))?;
    let mut actual = vec![0u8; BUFFER_SIZE];
    let mut want = vec![0u8; BUFFER_SIZE];
    let mut offset = 0u64;
    while offset < len {
        check_interrupted()?;
        let chunk = (len - offset).min(BUFFER_SIZE as u64) as usize;
        let read = read_up_to(file, &mut actual[..chunk])?;
        expected.fill_at(offset, &mut want[..chunk]);
        if let Some(at) = actual[..read]
            .iter()
            .zip(&want[..read])
            .position(|(got, wanted)| got != wanted)
        {
            return Ok(Some(offset + at as u64));
        }
        if read < chunk {
            return Ok(Some(offset + read as u64));
        }
        offset += chunk as u64;
    }
    let mut extra = [0u8; 1];
    Ok(match read_up_to(file, &mut extra)? {
        0 => None,
        _ => Some(len),
    })
}

/// reads into `buf` until it is full or the file ends; returns the number of bytes read.
fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

/// asks the kernel to drop the cached pages of `file`, so the read-back comes from the disk. a hint only:
/// its failure is ignored. elsewhere the read may still be served from the cache, so the check is weaker.
#[cfg(target_os = "linux")]
fn drop_cache(file: &File) {
    use nix::fcntl::{PosixFadviseAdvice, posix_fadvise};

    let _ = posix_fadvise(file, 0, 0, PosixFadviseAdvice::POSIX_FADV_DONTNEED);
}

#[cfg(not(target_os = "linux"))]
fn drop_cache(_file: &File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::path::Path;

    fn expected(seed: u8) -> Expected {
        Expected::from_seed([seed; SEED_LEN])
    }

    fn bytes(expected: &Expected, offset: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        expected.fill_at(offset, &mut buf);
        buf
    }

    fn write_expected(path: &Path, expected: &Expected, len: u64) {
        let mut data = vec![0u8; len as usize];
        expected.fill_at(0, &mut data);
        fs::write(path, data).unwrap();
    }

    #[test]
    fn same_seed_gives_the_same_bytes_and_another_seed_others() {
        assert_eq!(bytes(&expected(7), 0, 4096), bytes(&expected(7), 0, 4096));
        assert_ne!(bytes(&expected(7), 0, 4096), bytes(&expected(8), 0, 4096));
    }

    #[test]
    fn byte_depends_only_on_its_offset() {
        let stream = expected(3);
        let whole = bytes(&stream, 0, 5000);

        assert_eq!(bytes(&stream, 1234, 77), whole[1234..1311]);
        assert_eq!(bytes(&stream, 3, 1000), whole[3..1003]);
        assert_eq!(bytes(&stream, 4999, 1), whole[4999..]);
    }

    #[test]
    fn matching_file_passes_and_empty_file_passes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        let stream = expected(5);
        let len = 3 * BUFFER_SIZE as u64 + 5;
        write_expected(&path, &stream, len);
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();

        assert_eq!(first_difference(&mut file, &stream, len).unwrap(), None);
        assert!(check(&mut file, &stream, len).is_ok());

        let empty = dir.path().join("empty.bin");
        fs::write(&empty, b"").unwrap();
        let mut file = OpenOptions::new().read(true).open(&empty).unwrap();
        assert!(check(&mut file, &stream, 0).is_ok());
    }

    #[test]
    fn one_changed_byte_is_found_at_its_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        let stream = expected(9);
        let len = 2 * BUFFER_SIZE as u64;
        write_expected(&path, &stream, len);
        let changed = BUFFER_SIZE + 1234;
        let mut data = fs::read(&path).unwrap();
        data[changed] ^= 0xFF;
        fs::write(&path, &data).unwrap();
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();

        assert_eq!(
            first_difference(&mut file, &stream, len).unwrap(),
            Some(changed as u64)
        );
        let err = check(&mut file, &stream, len).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "verify failed: contents differ after the last pass"
        );
    }

    #[test]
    fn shorter_or_longer_file_differs_at_the_first_missing_or_extra_byte() {
        let dir = tempfile::tempdir().unwrap();
        let stream = expected(4);
        let len = 100u64;
        let path = dir.path().join("data.bin");
        write_expected(&path, &stream, len);

        let mut data = fs::read(&path).unwrap();
        data.pop();
        fs::write(&path, &data).unwrap();
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        assert_eq!(first_difference(&mut file, &stream, len).unwrap(), Some(99));

        write_expected(&path, &stream, len);
        let mut data = fs::read(&path).unwrap();
        data.push(0);
        fs::write(&path, &data).unwrap();
        let mut file = OpenOptions::new().read(true).open(&path).unwrap();
        assert_eq!(
            first_difference(&mut file, &stream, len).unwrap(),
            Some(100)
        );
    }
}
