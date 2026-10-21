use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use rand::RngCore;

#[cfg(not(windows))]
use std::fs::OpenOptions;

#[cfg(windows)]
use crate::platform::windows;
#[cfg(windows)]
use crate::prompt;

/// size of the buffer used for each write, in bytes.
const BUFFER_SIZE: usize = 1024 * 1024;

/// shown before a raw volume write; this path bypasses the filesystem entirely.
#[cfg(windows)]
const RAW_VOLUME_WARNING: &str = "raw volume write bypasses the filesystem: it can corrupt the volume \
    if the file changes during the operation, and data cached by other programs may survive";

/// settings for one erase run.
pub struct Options {
    pub passes: u32,
    pub verbose: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub force: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub kill_holders: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub raw_volume: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Zeros,
    Ones,
    Random,
}

impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Pattern::Zeros => "zeros",
            Pattern::Ones => "ones",
            Pattern::Random => "random",
        }
    }
}

/// zeros, then ones, then random data, repeated. the last pass is always random,
/// so the final content is not a recognizable fixed pattern.
fn pattern_for_pass(pass: u32, passes: u32) -> Pattern {
    if pass + 1 == passes {
        return Pattern::Random;
    }
    match pass % 3 {
        0 => Pattern::Zeros,
        1 => Pattern::Ones,
        _ => Pattern::Random,
    }
}

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first; the first failure stops the walk of that directory.
pub fn erase_path(path: &Path, opts: &Options) -> io::Result<()> {
    let file_type = fs::symlink_metadata(path)?.file_type();

    if file_type.is_symlink() {
        return fs::remove_file(path)
            .or_else(|file_err| fs::remove_dir(path).map_err(|_| file_err));
    }
    if file_type.is_dir() {
        return erase_dir(path, opts);
    }
    if file_type.is_file() {
        return erase_file(path, opts);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not a regular file, directory or symbolic link",
    ))
}

fn erase_dir(path: &Path, opts: &Options) -> io::Result<()> {
    make_writable(path, true)?;

    // collect entries first so the directory is not modified while it is being iterated.
    let entries: Vec<PathBuf> = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()?;
    for entry in &entries {
        erase_path(entry, opts)?;
    }

    if opts.verbose {
        eprintln!("  removing directory {}", path.display());
    }
    unlink_with_random_name(path, true)
}

/// overwrites the file's contents, then deletes it.
///
/// if deletion fails after the overwrite, the contents are already destroyed
/// but the file itself remains.
fn erase_file(path: &Path, opts: &Options) -> io::Result<()> {
    make_writable(path, false)?;
    let len = fs::metadata(path)?.len();

    match open_for_overwrite(path) {
        Ok(mut file) => {
            let whole = 0..len;
            overwrite(&mut file, std::slice::from_ref(&whole), opts)?;
            drop(file);
            remove_entry(path)
        }
        Err(err) if is_locked(&err) => erase_locked(path, len, opts, err),
        Err(err) => Err(err),
    }
}

/// handles a file that another process holds open. the original error is returned
/// unchanged on platforms without lock handling.
#[cfg(not(windows))]
fn erase_locked(_path: &Path, _len: u64, _opts: &Options, err: io::Error) -> io::Result<()> {
    Err(err)
}

#[cfg(windows)]
fn erase_locked(path: &Path, len: u64, opts: &Options, err: io::Error) -> io::Result<()> {
    let holders = windows::holders(path).unwrap_or_default();
    for holder in &holders {
        eprintln!(
            "{}: locked by {} (pid {})",
            path.display(),
            holder.name,
            holder.pid
        );
    }

    if opts.kill_holders && !holders.is_empty() {
        if !opts.force && !prompt::ask("close these processes to unlock the file?") {
            return Err(io::Error::other("skipped, file is still locked"));
        }
        windows::close_holders(path)?;

        match open_for_overwrite(path) {
            Ok(mut file) => {
                let whole = 0..len;
                overwrite(&mut file, std::slice::from_ref(&whole), opts)?;
                drop(file);
                return remove_entry(path);
            }
            Err(retry_err) => {
                eprintln!(
                    "{}: still locked after closing holders: {retry_err}",
                    path.display()
                );
            }
        }
    }

    if opts.raw_volume {
        eprintln!("warning: {RAW_VOLUME_WARNING}");
        if !prompt::ask("continue with raw volume write?") {
            return Err(io::Error::other("aborted, file is still locked"));
        }
        let layout = windows::physical_layout(path)?;
        let ranges: Vec<Range<u64>> = layout
            .ranges
            .iter()
            .map(|range| range.offset..range.offset + range.len)
            .collect();
        let mut volume = windows::open_volume_for_write(&layout.volume)?;
        overwrite(&mut volume, &ranges, opts)?;
        drop(volume);
        return remove_entry(path);
    }

    Err(io::Error::other(format!(
        "{err}; use --kill-holders or --raw-volume to erase it anyway"
    )))
}

#[cfg(windows)]
fn open_for_overwrite(path: &Path) -> io::Result<File> {
    windows::open_for_erase(path)
}

#[cfg(not(windows))]
fn open_for_overwrite(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).open(path)
}

/// sharing violation, lock violation, or access denied (which windows also returns
/// for delete-pending files).
#[cfg(windows)]
fn is_locked(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(not(windows))]
fn is_locked(_err: &io::Error) -> bool {
    false
}

/// removes a file whose contents are already overwritten.
#[cfg(not(windows))]
fn remove_entry(path: &Path) -> io::Result<()> {
    unlink_with_random_name(path, false)
}

/// windows: posix delete first (removes the name even while other handles are open),
/// then rename and delete, then, after asking, deletion at the next reboot.
#[cfg(windows)]
fn remove_entry(path: &Path) -> io::Result<()> {
    if let Ok(handle) = windows::open_for_delete(path)
        && windows::posix_delete(&handle).is_ok()
    {
        return Ok(());
    }
    if unlink_with_random_name(path, false).is_ok() {
        return Ok(());
    }

    if !prompt::ask(
        "file is still in use and cannot be deleted now (contents are already erased). schedule deletion at next reboot?",
    ) {
        return Err(io::Error::other(
            "kept, contents are erased but the file remains",
        ));
    }
    windows::delete_on_reboot(path)
}

fn overwrite(file: &mut File, ranges: &[Range<u64>], opts: &Options) -> io::Result<()> {
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        for range in ranges {
            overwrite_range(file, range, pattern)?;
        }
        file.sync_all()?;
    }
    Ok(())
}

fn overwrite_range(file: &mut File, range: &Range<u64>, pattern: Pattern) -> io::Result<()> {
    file.seek(SeekFrom::Start(range.start))?;

    let mut buf = vec![0u8; BUFFER_SIZE];
    let mut remaining = range.end - range.start;
    while remaining > 0 {
        let chunk = remaining.min(BUFFER_SIZE as u64) as usize;
        fill(&mut buf[..chunk], pattern);
        file.write_all(&buf[..chunk])?;
        remaining -= chunk as u64;
    }
    Ok(())
}

fn fill(buf: &mut [u8], pattern: Pattern) {
    match pattern {
        Pattern::Zeros => buf.fill(0x00),
        Pattern::Ones => buf.fill(0xFF),
        Pattern::Random => rand::rng().fill_bytes(buf),
    }
}

/// clears the read-only flag so the entry can be overwritten and renamed.
fn make_writable(path: &Path, is_dir: bool) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    if !perms.readonly() {
        return Ok(());
    }
    set_writable(&mut perms, is_dir);
    fs::set_permissions(path, perms)
}

#[cfg(unix)]
fn set_writable(perms: &mut fs::Permissions, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;

    // owner-only bits: rw for files, rwx for directories (needed to traverse them).
    let owner_bits = if is_dir { 0o700 } else { 0o600 };
    perms.set_mode(perms.mode() | owner_bits);
}

#[cfg(not(unix))]
fn set_writable(perms: &mut fs::Permissions, _is_dir: bool) {
    perms.set_readonly(false);
}

/// renames the entry to a random hidden name in the same directory, then deletes it,
/// so the original name does not remain in the directory.
fn unlink_with_random_name(path: &Path, is_dir: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let hidden = loop {
        let candidate = dir.join(format!(".{:016x}", rand::random::<u64>()));
        if !candidate.exists() {
            break candidate;
        }
    };

    fs::rename(path, &hidden)?;
    if is_dir {
        fs::remove_dir(&hidden)
    } else {
        fs::remove_file(&hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    fn options(passes: u32) -> Options {
        Options {
            passes,
            verbose: false,
            force: true,
            kill_holders: false,
            raw_volume: false,
        }
    }

    fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn pattern_sequence_ends_with_random() {
        assert_eq!(pattern_for_pass(0, 1), Pattern::Random);
        assert_eq!(pattern_for_pass(0, 3), Pattern::Zeros);
        assert_eq!(pattern_for_pass(1, 3), Pattern::Ones);
        assert_eq!(pattern_for_pass(2, 3), Pattern::Random);
    }

    #[test]
    fn zeros_pass_replaces_contents_and_keeps_length() {
        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 2 * BUFFER_SIZE + 17];
        let path = write_file(dir.path(), "data.bin", &original);

        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        let len = original.len() as u64;
        overwrite_range(&mut file, &(0..len), Pattern::Zeros).unwrap();
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
        overwrite_range(&mut file, &(0..len), Pattern::Random).unwrap();
        drop(file);

        let contents = fs::read(&path).unwrap();
        assert_eq!(contents.len(), original.len());
        assert_ne!(contents, original);
    }

    #[test]
    fn erase_removes_file_without_leaving_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "secret.txt", b"secret");

        erase_path(&path, &options(2)).unwrap();

        assert!(!path.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn erase_handles_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "empty.txt", b"");

        erase_path(&path, &options(1)).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn erase_removes_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("nested/deeper")).unwrap();
        write_file(&tree, "a.txt", b"a");
        write_file(&tree.join("nested/deeper"), "b.txt", b"b");

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn erase_removes_read_only_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        let file = write_file(&tree, "locked.txt", b"data");

        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();
        let mut perms = fs::metadata(&tree).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&tree, perms).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_removed_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target.txt", b"keep me");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        erase_path(&link, &options(1)).unwrap();

        assert!(!link.exists());
        assert_eq!(fs::read(&target).unwrap(), b"keep me");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_tree_does_not_erase_outside_target() {
        let outside = tempfile::tempdir().unwrap();
        let outside_file = write_file(outside.path(), "outside.txt", b"keep me");

        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(outside.path(), tree.join("link")).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
    }
}
