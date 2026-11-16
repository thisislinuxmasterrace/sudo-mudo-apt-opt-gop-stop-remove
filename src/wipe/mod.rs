//! erases the free space of a filesystem: one pass per pass count, each pass fills the free blocks
//! with temporary files that disappear by themselves.

mod fill;
mod temp;

#[cfg(unix)]
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::erase::{Options, Pattern};
use crate::preview::human_bytes;
use crate::progress::Progress;

/// keeps every fill file on the directory selected before confirmation.
pub struct Directory {
    path: PathBuf,
    #[cfg(unix)]
    file: File,
    allow_protected: bool,
    /// the directory is a protected system path that `--i-know-what-i-am-doing` let through.
    protected: bool,
}

impl Directory {
    pub fn open(path: &Path, allow_protected: bool) -> io::Result<Self> {
        #[cfg(unix)]
        let file = open_directory(path)?;
        let mut selected = Self {
            path: path.to_path_buf(),
            #[cfg(unix)]
            file,
            allow_protected,
            protected: false,
        };
        selected.protected = selected.check()?;
        Ok(selected)
    }

    pub fn is_protected(&self) -> bool {
        self.protected
    }

    /// refuses a directory that must not be filled. returns true for a protected path that was let through.
    /// runs again before every fill file: the check is on the opened directory, not on the path.
    fn check(&self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            // the safety checks word their refusal for erasing; this is a fill.
            crate::safety::unix::check_wipe(&self.file, self.allow_protected).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    err.to_string()
                        .replacen("refusing to change", "refusing to fill", 1),
                )
            })?;
            if let Some(reason) = crate::storage::opened_wipe_refusal(&self.file) {
                return Err(io::Error::other(format!("refusing to fill {reason}")));
            }
            // with the flag the first check passes anything but a root; ask again without it.
            Ok(self.allow_protected && crate::safety::unix::check_wipe(&self.file, false).is_err())
        }
        #[cfg(not(unix))]
        {
            let protected = crate::safety::validate_wipe_dir(&self.path, self.allow_protected)
                .map_err(io::Error::other)?;
            if let Some(reason) = crate::storage::wipe_refused(&self.path) {
                return Err(io::Error::other(format!("refusing to fill {reason}")));
            }
            Ok(protected)
        }
    }
}

/// opens `path` as a directory. a symlink is followed on purpose: the filesystem behind it is the one that gets filled.
#[cfg(unix)]
fn open_directory(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|err| {
            if err.raw_os_error() == Some(libc::ENOTDIR) {
                io::Error::other("not a directory")
            } else {
                err
            }
        })
}

/// how a wipe ended.
pub struct Outcome {
    /// bytes written over all passes.
    pub written: u64,
    /// the pass that wrote more than the volume holds, which ends the run: the data never reached free blocks.
    pub exceeded_pass: Option<u32>,
}

/// wipes the free space visible to `dir`, `opts.passes` times.
///
/// a pass writes random data into new blocks until the volume is full, then removes the file.
/// the removed data stays in the free blocks, so each pass replaces the previous one. nothing is
/// overwritten in place, so this also works on copy-on-write filesystems.
///
/// the blocks reserved for root are filled too: sudo is mandatory, so this runs as root.
pub fn wipe_selected(dir: &Directory, opts: &Options) -> io::Result<Outcome> {
    // without a readable size a pass has no bound and runs until the volume is full, as before.
    // on unix the size comes from the retained descriptor, so a swapped path cannot change it.
    #[cfg(unix)]
    let bound = crate::storage::total_bytes_of_open(&dir.file);
    #[cfg(not(unix))]
    let bound = crate::storage::total_bytes(&dir.path);
    wipe_passes(dir, opts, None, bound)
}

fn wipe_passes(
    dir: &Directory,
    opts: &Options,
    limit: Option<u64>,
    bound: Option<u64>,
) -> io::Result<Outcome> {
    // the size is only known when a limit is given (tests); a wipe runs until the volume is full.
    let total = limit.map(|max| max.saturating_mul(u64::from(opts.passes)));
    let mut progress = Progress::new(total, opts.quiet);
    let result = wipe_each_pass(dir, opts, limit, bound, &mut progress);
    progress.finish();
    result
}

/// random for every pass: a fixed pattern adds nothing to blocks that are allocated fresh, and
/// zeros may not allocate blocks at all on a compressing filesystem. `--zero` ends with zeros.
fn pattern_for_pass(pass: u32, passes: u32, zero: bool) -> Pattern {
    if zero && pass + 1 == passes {
        Pattern::Zeros
    } else {
        Pattern::Random
    }
}

fn wipe_each_pass(
    dir: &Directory,
    opts: &Options,
    limit: Option<u64>,
    bound: Option<u64>,
    progress: &mut Progress,
) -> io::Result<Outcome> {
    let mut total = 0u64;
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes, opts.zero);
        if opts.verbose {
            progress.finish();
            crate::ui::message!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        let done = fill::fill_pass(dir, pattern, limit, bound, opts.verbose, progress)?;
        total += done.written;
        if done.exceeded {
            progress.finish();
            crate::ui::message!(
                "{}: warning: pass {}/{} stopped after writing more than the volume size: the filesystem probably compresses or deduplicates the data, free space may not be wiped",
                crate::ui::shown(&dir.path),
                pass + 1,
                opts.passes
            );
            return Ok(Outcome {
                written: total,
                exceeded_pass: Some(pass + 1),
            });
        }
        if opts.verbose {
            progress.finish();
            crate::ui::message!(
                "  pass {}/{} done: {} written",
                pass + 1,
                opts.passes,
                human_bytes(done.written)
            );
        }
    }
    Ok(Outcome {
        written: total,
        exceeded_pass: None,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    // the unix production path checks the opened directory, not `validate_wipe_dir`. opening only.
    #[cfg(unix)]
    #[test]
    fn user_data_roots_open_for_a_fill_without_the_override() {
        for root in ["/home", "/mnt", "/media", "/srv"] {
            if !Path::new(root).is_dir() {
                continue;
            }
            assert!(Directory::open(Path::new(root), false).is_ok(), "{root}");
        }
    }

    fn options(passes: u32, zero: bool) -> Options {
        Options {
            passes,
            zero,
            verbose: false,
            force: true,
            kill_holders: false,
            allow_protected: false,
            quiet: true,
            verify: false,
        }
    }

    #[test]
    fn every_ordinary_pass_is_random() {
        for passes in 1..=5 {
            for pass in 0..passes {
                assert_eq!(pattern_for_pass(pass, passes, false), Pattern::Random);
            }
        }
    }

    #[test]
    fn zero_is_only_the_final_pass() {
        // `--zero` adds one pass to the count, so 3 means two ordinary passes and the zero pass.
        assert_eq!(pattern_for_pass(0, 3, true), Pattern::Random);
        assert_eq!(pattern_for_pass(1, 3, true), Pattern::Random);
        assert_eq!(pattern_for_pass(2, 3, true), Pattern::Zeros);
    }

    #[test]
    fn passes_leave_no_fill_file_behind() {
        let dir = tempfile::tempdir().unwrap();

        let outcome = wipe_passes(
            &Directory::open(dir.path(), false).unwrap(),
            &options(3, false),
            Some(3 * MIB),
            None,
        )
        .unwrap();

        assert_eq!(outcome.written, 9 * MIB);
        assert_eq!(outcome.exceeded_pass, None);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_pass_beyond_the_volume_size_ends_the_run() {
        let dir = tempfile::tempdir().unwrap();

        // no limit and a 2 mib "volume": only the bound can stop the pass, as on a compressing filesystem.
        let outcome = wipe_passes(
            &Directory::open(dir.path(), false).unwrap(),
            &options(3, false),
            Some(10 * MIB),
            Some(2 * MIB),
        )
        .unwrap();

        assert_eq!(outcome.exceeded_pass, Some(1));
        assert!(outcome.written > 2 * MIB);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        fs::write(&file, b"x").unwrap();

        let err = Directory::open(&file, false).err().unwrap();

        assert_eq!(err.to_string(), "not a directory");
    }
}
