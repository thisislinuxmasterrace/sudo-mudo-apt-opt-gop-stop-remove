//! overwrites file contents in place, pass by pass. the last pass is the one a read-back can check.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::ops::Range;

use crate::progress::Progress;
use crate::verify::Expected;

use super::Options;
use super::interrupt::check_interrupted;
use super::pattern::{Pattern, fill, pattern_for_pass};

/// size of the buffer used for each write, in bytes.
const BUFFER_SIZE: usize = 1024 * 1024;

/// `expected` is the seed of the read-back: the last pass writes its bytes, so the check can reproduce them.
pub(super) fn overwrite(
    file: &mut File,
    ranges: &[Range<u64>],
    opts: &Options,
    expected: Option<&Expected>,
) -> io::Result<()> {
    let size: u64 = ranges.iter().map(|range| range.end - range.start).sum();
    let total = size.saturating_mul(u64::from(opts.passes));
    let mut progress = Progress::new(Some(total), opts.quiet);
    let result = overwrite_passes(file, ranges, opts, expected, &mut progress);
    progress.finish();
    result
}

fn overwrite_passes(
    file: &mut File,
    ranges: &[Range<u64>],
    opts: &Options,
    expected: Option<&Expected>,
    progress: &mut Progress,
) -> io::Result<()> {
    let mut buf = vec![0u8; BUFFER_SIZE];
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            progress.finish();
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        // the last pass is always random, so it is the one the read-back can reproduce.
        let source = if pass + 1 == opts.passes {
            expected
        } else {
            None
        };
        for range in ranges {
            overwrite_range(file, range, pattern, source, &mut buf, progress)?;
        }
        file.sync_all()?;
    }
    Ok(())
}

fn overwrite_range(
    file: &mut File,
    range: &Range<u64>,
    pattern: Pattern,
    expected: Option<&Expected>,
    buf: &mut [u8],
    progress: &mut Progress,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(range.start))?;

    let mut offset = range.start;
    let mut remaining = range.end - range.start;
    while remaining > 0 {
        check_interrupted()?;
        let chunk = remaining.min(buf.len() as u64) as usize;
        match expected {
            Some(expected) => expected.fill_at(offset, &mut buf[..chunk]),
            None => fill(&mut buf[..chunk], pattern),
        }
        file.write_all(&buf[..chunk])?;
        progress.add(chunk as u64);
        offset += chunk as u64;
        remaining -= chunk as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erase::tests::write_file;
    use std::fs;
    use std::fs::OpenOptions;

    #[test]
    fn zeros_pass_replaces_contents_and_keeps_length() {
        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 2 * BUFFER_SIZE + 17];
        let path = write_file(dir.path(), "data.bin", &original);

        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        let len = original.len() as u64;
        overwrite_range(
            &mut file,
            &(0..len),
            Pattern::Zeros,
            None,
            &mut vec![0u8; BUFFER_SIZE],
            &mut Progress::new(None, true),
        )
        .unwrap();
        drop(file);

        let contents = fs::read(&path).unwrap();
        assert_eq!(contents.len(), original.len());
        assert!(contents.iter().all(|&b| b == 0));
    }

    #[test]
    fn random_pass_changes_contents() {
        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 4096];
        let path = write_file(dir.path(), "data.bin", &original);

        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        let len = original.len() as u64;
        overwrite_range(
            &mut file,
            &(0..len),
            Pattern::Random,
            None,
            &mut vec![0u8; BUFFER_SIZE],
            &mut Progress::new(None, true),
        )
        .unwrap();
        drop(file);

        let contents = fs::read(&path).unwrap();
        assert_eq!(contents.len(), original.len());
        assert_ne!(contents, original);
    }

    #[cfg(unix)]
    #[test]
    fn mismatch_after_the_last_pass_keeps_the_file_under_its_name() {
        use crate::erase::at::{hide_entry, keep_partly_overwritten};
        use crate::verify::{self, SEED_LEN};
        use nix::fcntl::AtFlags;
        use nix::sys::stat::fstatat;
        use std::ffi::CString;

        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 4096];
        let path = write_file(dir.path(), "data.bin", &original);
        let len = original.len() as u64;
        let parent = File::open(dir.path()).unwrap();
        let name = CString::new("data.bin").unwrap();
        let st = fstatat(&parent, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW).unwrap();
        let hidden = hide_entry(&parent, name.as_c_str(), &st, &path).unwrap();

        // the last pass as overwrite_passes writes it, with seed A; the read-back with seed A matches.
        let written = Expected::from_seed([0xA; SEED_LEN]);
        let hidden_path = dir.path().join(hidden.to_str().unwrap());
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&hidden_path)
            .unwrap();
        overwrite_range(
            &mut file,
            &(0..len),
            Pattern::Random,
            Some(&written),
            &mut vec![0u8; BUFFER_SIZE],
            &mut Progress::new(None, true),
        )
        .unwrap();
        file.sync_all().unwrap();
        verify::check(&mut file, &written, len).unwrap();

        // seed B does not match the bytes on disk: the error keeps the file under its name.
        let err = verify::check(&mut file, &Expected::from_seed([0xB; SEED_LEN]), len).unwrap_err();
        let kept = keep_partly_overwritten(&parent, name.as_c_str(), &hidden, &path, err);

        assert_eq!(kept.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            kept.to_string(),
            "verify failed: contents differ after the last pass, the file is kept"
        );
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["data.bin".to_owned()]);
    }
}
