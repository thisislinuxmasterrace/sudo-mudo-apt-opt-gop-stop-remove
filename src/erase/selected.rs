//! retains command-line selections across confirmation without following their parents again.

use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use nix::fcntl::AtFlags;
use nix::sys::stat::{FileStat, fstat, fstatat, stat};

pub struct Selected {
    pub(super) dir: File,
    pub(super) entry: CString,
    pub(super) st: FileStat,
    pub(super) real: PathBuf,
    #[cfg(target_os = "linux")]
    _entry_handle: File,
}

impl Selected {
    pub fn check(&self, recursive: bool, allow_protected: bool) -> io::Result<()> {
        if super::at::file_kind(&self.st) == nix::sys::stat::SFlag::S_IFDIR && !recursive {
            return Err(io::Error::other(
                "is a directory (use -r to erase directories)",
            ));
        }
        crate::safety::unix::check_entry(&self.dir, &self.st, allow_protected)
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let Some(name) = path.file_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to erase '.', '..' or a filesystem root",
            ));
        };
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        // the parent comes from the user, so links in it are followed; everything below it goes through
        // the open descriptor.
        let dir = File::open(parent)?;
        let entry = CString::new(name.as_bytes()).map_err(io::Error::other)?;
        let st = fstatat(&dir, entry.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
        #[cfg(target_os = "linux")]
        let entry_handle = {
            use nix::fcntl::{OFlag, openat};
            use nix::sys::stat::Mode;
            let file = File::from(openat(
                &dir,
                entry.as_c_str(),
                OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?);
            let opened = fstat(&file)?;
            if opened.st_dev != st.st_dev || opened.st_ino != st.st_ino {
                return Err(io::Error::other("selected entry changed while opening it"));
            }
            file
        };
        let real = real_path(&dir, &fstat(&dir)?, parent)?.join(name);
        Ok(Self {
            dir,
            entry,
            st,
            real,
            #[cfg(target_os = "linux")]
            _entry_handle: entry_handle,
        })
    }

    pub(super) fn current_real(&self) -> io::Result<PathBuf> {
        let parent = self
            .real
            .parent()
            .ok_or_else(|| io::Error::other("selected parent is missing"))?;
        Ok(real_path(&self.dir, &fstat(&self.dir)?, parent)?
            .join(std::ffi::OsStr::from_bytes(self.entry.as_bytes())))
    }
}

/// the absolute path of the open directory `dir`, for mount lookups. linux reads it from the
/// descriptor; elsewhere `parent` is canonicalized. either way the result must still be `dir`
/// (`dir_st`), or the lookup would be made for another directory.
fn real_path(dir: &File, dir_st: &FileStat, parent: &Path) -> io::Result<PathBuf> {
    let real = opened_path(dir, parent)?;
    let now = stat(&real)?;
    if now.st_dev != dir_st.st_dev || now.st_ino != dir_st.st_ino {
        return Err(io::Error::other(format!(
            "{} is not the directory that was opened",
            crate::ui::shown(&real)
        )));
    }
    Ok(real)
}

/// without /proc the parent is canonicalized as on other systems; `real_path` checks both alike.
#[cfg(target_os = "linux")]
fn opened_path(dir: &File, parent: &Path) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd()))
        .or_else(|_| fs::canonicalize(parent))
}

#[cfg(not(target_os = "linux"))]
fn opened_path(_dir: &File, parent: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(parent)
}
