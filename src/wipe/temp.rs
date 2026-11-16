//! creates the temporary fill files so that none can outlive the process.

use std::fs::File;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::io;

use super::Directory;

fn random_name() -> String {
    format!(".fill-{:016x}", rand::random::<u64>())
}

/// on linux the file never gets a name when the filesystem supports `O_TMPFILE`; otherwise it is
/// created and unlinked at once. on windows it is deleted when the last handle closes.
pub(super) fn create(dir: &Directory) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    if let Some(file) = create_unnamed(dir)? {
        return Ok(file);
    }
    create_named(dir, &random_name())
}

/// `None` when the filesystem or the kernel cannot make a nameless file here.
#[cfg(target_os = "linux")]
fn create_unnamed(dir: &Directory) -> io::Result<Option<File>> {
    use nix::errno::Errno;
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;

    // o_excl keeps the file from ever being linked into the directory. an old kernel that does
    // not know o_tmpfile answers with eisdir or einval.
    match openat(
        &dir.file,
        c".",
        OFlag::O_TMPFILE | OFlag::O_WRONLY | OFlag::O_EXCL | OFlag::O_CLOEXEC,
        Mode::S_IRUSR | Mode::S_IWUSR,
    ) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(Errno::EOPNOTSUPP | Errno::EISDIR | Errno::EINVAL | Errno::ENOENT) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

#[cfg(unix)]
fn create_named(dir: &Directory, name: &str) -> io::Result<File> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};

    let file = File::from(openat(
        &dir.file,
        name,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?);
    unlinkat(&dir.file, name, UnlinkatFlags::NoRemoveDir).map_err(|err| {
        io::Error::other(format!(
            "{err}; leftover file {}",
            crate::ui::shown(&dir.path.join(name))
        ))
    })?;
    Ok(file)
}

#[cfg(windows)]
fn create_named(dir: &Directory, name: &str) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_DELETE_ON_CLOSE};

    // std rejects create_new without write(true) before it looks at access_mode, which sets the real rights.
    OpenOptions::new()
        .write(true)
        .access_mode(GENERIC_WRITE | DELETE)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(dir.path.join(name))
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    #[test]
    fn created_file_has_no_name_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let selected = Directory::open(dir.path(), false).unwrap();

        let mut file = create(&selected).unwrap();
        file.write_all(b"x").unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn named_fallback_unlinks_the_file_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let selected = Directory::open(dir.path(), false).unwrap();

        let mut file = create_named(&selected, ".fill-test").unwrap();
        file.write_all(b"x").unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
