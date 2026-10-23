//! the openat primitives of the unix erase: lookups by name inside an open directory, identity checks,
//! mode changes, and the rename to a hidden name that comes before every delete.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{FchmodatFlags, FileStat, Mode, SFlag, fchmodat, fstat, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};

/// renames the entry to a random hidden name in `parent`. the rename is undone when the entry
/// under the hidden name is not the one `st` describes.
#[cfg(unix)]
pub(super) fn hide_entry(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
) -> io::Result<CString> {
    let hidden = hidden_name(parent)?;
    renameat(parent, name, parent, hidden.as_c_str())?;
    let checked = fstatat(parent, hidden.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .and_then(|now| {
            if same_entry(&now, st) {
                Ok(())
            } else {
                Err(io::Error::other("entry changed while it was being erased"))
            }
        });
    match checked {
        Ok(()) => Ok(hidden),
        Err(err) => Err(restore_name(parent, name, &hidden, shown, err)),
    }
}

/// opens `name` in `parent` without following links, then checks the descriptor is still the
/// entry that was stat'ed, so a name swapped after the stat is refused.
#[cfg(unix)]
pub(super) fn open_checked(
    parent: &File,
    name: &CStr,
    flags: OFlag,
    st: &FileStat,
) -> io::Result<File> {
    let file = File::from(openat(parent, name, flags, Mode::empty())?);
    if !same_entry(&fstat(&file)?, st) {
        return Err(io::Error::other("entry changed while it was being erased"));
    }
    Ok(file)
}

/// true when `now` is the entry `st` describes: same device, inode and file type.
#[cfg(unix)]
fn same_entry(now: &FileStat, st: &FileStat) -> bool {
    now.st_dev == st.st_dev && now.st_ino == st.st_ino && file_kind(now) == file_kind(st)
}

/// file type bits of a stat result: S_IFDIR, S_IFREG, S_IFLNK and the rest.
#[cfg(unix)]
pub(super) fn file_kind(st: &FileStat) -> SFlag {
    SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT
}

/// adds `owner_bits` to the owner's permissions when they are missing; other bits are kept.
/// works on mode-000 entries too, because it needs no read or write access to the entry.
#[cfg(unix)]
pub(super) fn make_owner_accessible(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    owner_bits: Mode,
) -> io::Result<()> {
    let mode = Mode::from_bits_truncate(st.st_mode);
    if mode.contains(owner_bits) {
        return Ok(());
    }
    set_mode_at(parent, name, st, mode | owner_bits)
}

/// changes the mode of the inode that `st` describes. linux goes through a path handle and
/// /proc, so the change lands on that inode even if the name is swapped meanwhile.
#[cfg(target_os = "linux")]
fn set_mode_at(parent: &File, name: &CStr, st: &FileStat, mode: Mode) -> io::Result<()> {
    // O_PATH needs no access to the entry itself, only search permission on `parent`.
    let handle = open_checked(
        parent,
        name,
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    let proc_path = format!("/proc/self/fd/{}", handle.as_raw_fd());
    // the path is absolute, so `parent` is not used for the lookup.
    fchmodat(
        parent,
        proc_path.as_str(),
        mode,
        FchmodatFlags::FollowSymlink,
    )
    .map_err(io::Error::from)
}

/// other unix systems have no path handle without access to the entry, so the change goes by
/// name, with the identity checked right before it.
#[cfg(all(unix, not(target_os = "linux")))]
fn set_mode_at(parent: &File, name: &CStr, st: &FileStat, mode: Mode) -> io::Result<()> {
    let now = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if now.st_dev != st.st_dev || now.st_ino != st.st_ino {
        return Err(io::Error::other("entry changed while it was being erased"));
    }
    fchmodat(parent, name, mode, FchmodatFlags::NoFollowSymlink).map_err(io::Error::from)
}

/// renames the entry to a random hidden name in `parent`, then deletes it, so the original name
/// does not remain in the directory.
#[cfg(unix)]
pub(super) fn remove_at(parent: &File, name: &CStr, is_dir: bool, shown: &Path) -> io::Result<()> {
    let hidden = hidden_name(parent)?;
    renameat(parent, name, parent, hidden.as_c_str())?;
    remove_hidden(parent, name, &hidden, is_dir, shown)
}

/// deletes the entry that was renamed to `hidden`. if that fails, the name is put back.
#[cfg(unix)]
pub(super) fn remove_hidden(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    is_dir: bool,
    shown: &Path,
) -> io::Result<()> {
    let flags = if is_dir {
        UnlinkatFlags::RemoveDir
    } else {
        UnlinkatFlags::NoRemoveDir
    };
    unlinkat(parent, hidden, flags)
        .map_err(|err| restore_name(parent, name, hidden, shown, io::Error::from(err)))
}

/// renames the hidden entry back to `name`. on failure returns where the entry is now.
#[cfg(unix)]
fn put_back(parent: &File, name: &CStr, hidden: &CStr, shown: &Path) -> Result<(), PathBuf> {
    // a name taken in the meantime is not replaced by the rename back.
    let name_free = matches!(
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW),
        Err(Errno::ENOENT)
    );
    if name_free && renameat(parent, hidden, parent, name).is_ok() {
        return Ok(());
    }
    Err(shown.with_file_name(OsStr::from_bytes(hidden.to_bytes())))
}

/// puts a hidden entry back under `name` after a failed step. if that fails too, the error says
/// where the entry is.
#[cfg(unix)]
fn restore_name(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    shown: &Path,
    err: io::Error,
) -> io::Error {
    match put_back(parent, name, hidden, shown) {
        Ok(()) => err,
        Err(left) => io::Error::new(
            err.kind(),
            format!("{err}; the entry is left as {}", left.display()),
        ),
    }
}

/// puts back a file whose overwrite failed. the error says the file is kept under its name, or where
/// it is left.
#[cfg(unix)]
pub(super) fn keep_partly_overwritten(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    shown: &Path,
    err: io::Error,
) -> io::Error {
    match put_back(parent, name, hidden, shown) {
        Ok(()) => io::Error::new(err.kind(), format!("{err}, the file is kept")),
        Err(left) => io::Error::new(
            err.kind(),
            format!("{err}; the entry is left as {}", left.display()),
        ),
    }
}

/// true when `name` in `dir` is a directory, without following links.
#[cfg(unix)]
pub(super) fn is_directory_at(dir: &File, name: &CStr) -> bool {
    matches!(
        fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW),
        Ok(st) if file_kind(&st) == SFlag::S_IFDIR
    )
}

/// a random name that is free in `parent`. only ENOENT from fstatat on the directory counts as free.
#[cfg(unix)]
fn hidden_name(parent: &File) -> io::Result<CString> {
    loop {
        let name =
            CString::new(format!(".{:016x}", rand::random::<u64>())).map_err(io::Error::other)?;
        match fstatat(parent, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Err(Errno::ENOENT) => return Ok(name),
            Ok(_) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }
}
