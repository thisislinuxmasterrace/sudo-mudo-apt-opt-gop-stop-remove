//! openat primitives of the unix erase: opens and mode changes with an identity check (dev, ino, type),
//! and the rename to a hidden name before every delete. `hide_entry` verifies the entry after the
//! rename and puts a swapped one back; the delete then goes by the hidden name.

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

/// renames the entry to a random hidden name in `parent`, then checks it against `st`. the check
/// comes after the rename, so a swapped entry is moved first and then put back. on linux the rename
/// uses RENAME_NOREPLACE, so it never replaces an entry that took the hidden name; elsewhere, and on
/// filesystems without it, the hidden name was only free just before.
#[cfg(unix)]
pub(super) fn hide_entry(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
) -> io::Result<CString> {
    #[cfg(test)]
    seam::fire(seam::Stage::Hide, name);
    let hidden = rename_to_hidden(parent, name)?;
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
    #[cfg(test)]
    seam::fire(seam::Stage::Open, name);
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
    // O_PATH needs no access to the entry, only search permission on `parent`.
    let handle = open_checked(
        parent,
        name,
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    let proc_path = format!("/proc/self/fd/{}", handle.as_raw_fd());
    // the path is absolute, so the dirfd is ignored; the /proc link resolves to the checked inode.
    fchmodat(
        parent,
        proc_path.as_str(),
        mode,
        FchmodatFlags::FollowSymlink,
    )
    .map_err(io::Error::from)
}

/// other unix systems have no path handle without access to the entry, so the change goes by
/// name. the identity is checked just before it, which leaves a small race. it ignores the file type.
#[cfg(all(unix, not(target_os = "linux")))]
fn set_mode_at(parent: &File, name: &CStr, st: &FileStat, mode: Mode) -> io::Result<()> {
    let now = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if now.st_dev != st.st_dev || now.st_ino != st.st_ino {
        return Err(io::Error::other("entry changed while it was being erased"));
    }
    fchmodat(parent, name, mode, FchmodatFlags::NoFollowSymlink).map_err(io::Error::from)
}

/// renames the entry to a hidden name, checks it against `st` (`hide_entry`), then deletes the hidden
/// name, so the original name does not stay in the directory and a swapped entry is put back, not
/// deleted. used for links and for directories that were already emptied.
#[cfg(unix)]
pub(super) fn remove_checked(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
) -> io::Result<()> {
    let hidden = hide_entry(parent, name, st, shown)?;
    remove_hidden(
        parent,
        name,
        &hidden,
        file_kind(st) == SFlag::S_IFDIR,
        shown,
    )
}

/// deletes the entry under `hidden`. the hidden name was checked by `hide_entry`; the delete itself
/// goes by that name. if it fails, the name is put back.
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

/// renames the hidden entry back to `name`, unless `name` was taken meanwhile. on failure returns
/// where the entry is now.
#[cfg(unix)]
fn put_back(parent: &File, name: &CStr, hidden: &CStr, shown: &Path) -> Result<(), PathBuf> {
    #[cfg(test)]
    seam::fire(seam::Stage::PutBack, name);
    match rename_noreplace(parent, hidden, name) {
        Ok(true) => return Ok(()),
        Ok(false) => {
            // no RENAME_NOREPLACE: check-then-rename, so a name created in between could still be
            // replaced.
            let name_free = matches!(
                fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW),
                Err(Errno::ENOENT)
            );
            if name_free && renameat(parent, hidden, parent, name).is_ok() {
                return Ok(());
            }
        }
        Err(_) => {}
    }
    Err(shown.with_file_name(OsStr::from_bytes(hidden.to_bytes())))
}

/// renames `name` to a random hidden name that is free, and returns that name.
#[cfg(unix)]
fn rename_to_hidden(parent: &File, name: &CStr) -> io::Result<CString> {
    // a taken name is only a collision of random names, or someone creating it on purpose; give up
    // after a few.
    for _ in 0..8 {
        let hidden = hidden_name(parent)?;
        #[cfg(test)]
        seam::fire(seam::Stage::HiddenName, &hidden);
        match rename_noreplace(parent, name, &hidden) {
            Ok(true) => return Ok(hidden),
            // no RENAME_NOREPLACE: the hidden name was free just before.
            Ok(false) => {
                renameat(parent, name, parent, hidden.as_c_str())?;
                return Ok(hidden);
            }
            Err(Errno::EEXIST) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }
    Err(io::Error::other("could not find a free hidden name"))
}

/// renames `from` to `to` in `parent` only if `to` does not exist. `Ok(false)` means the kernel or
/// the filesystem has no RENAME_NOREPLACE and nothing was renamed; the caller falls back.
#[cfg(target_os = "linux")]
fn rename_noreplace(parent: &File, from: &CStr, to: &CStr) -> Result<bool, Errno> {
    // safety: both names are nul-terminated and remain valid for the call; parent owns both dirfds.
    // the syscall is used directly because nix exposes renameat2 only on glibc builds.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    match Errno::result(result) {
        Ok(_) => Ok(true),
        Err(Errno::EINVAL | Errno::ENOSYS) => Ok(false),
        Err(err) => Err(err),
    }
}

/// no linux renameat2 here: the callers use check-then-rename.
#[cfg(all(unix, not(target_os = "linux")))]
fn rename_noreplace(_parent: &File, _from: &CStr, _to: &CStr) -> Result<bool, Errno> {
    Ok(false)
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
            format!("{err}; the entry is left as {}", crate::ui::shown(&left)),
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
            format!("{err}; the entry is left as {}", crate::ui::shown(&left)),
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

/// a random name that is free in `parent` (ENOENT from fstatat). free at check time only.
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

/// a test hook that runs once between a check and the act it guards, so a test can swap the entry
/// there. hooks are per thread, so tests running in parallel do not see each other's.
#[cfg(test)]
pub(super) mod seam {
    use std::cell::RefCell;
    use std::ffi::{CStr, CString};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(in crate::erase) enum Stage {
        /// in `hide_entry`, before the rename to the hidden name.
        Hide,
        /// after a free hidden name was picked, before the rename to it; the hook gets the hidden name.
        HiddenName,
        /// in `open_checked`, before the open.
        Open,
        /// in `put_back`, before the rename back.
        PutBack,
        /// after the last pass and the read-back, before the size check that precedes the delete.
        Written,
    }

    type Hook = Box<dyn FnOnce(&CStr)>;

    thread_local! {
        static HOOKS: RefCell<Vec<(Stage, Option<CString>, Hook)>> = const { RefCell::new(Vec::new()) };
    }

    /// runs `hook` the first time `stage` is reached for `name`, or for any name when `name` is `None`.
    pub(in crate::erase) fn set(
        stage: Stage,
        name: Option<&str>,
        hook: impl FnOnce(&CStr) + 'static,
    ) {
        let name = name.map(|name| CString::new(name).unwrap());
        HOOKS.with(|hooks| hooks.borrow_mut().push((stage, name, Box::new(hook))));
    }

    pub(in crate::erase) fn fire(stage: Stage, name: &CStr) {
        let hook = HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            let index = hooks.iter().position(|(at, only, _)| {
                *at == stage && only.as_deref().is_none_or(|only| only == name)
            })?;
            Some(hooks.remove(index).2)
        });
        if let Some(hook) = hook {
            hook(name);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn rename_noreplace_refuses_a_taken_name_and_keeps_both_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"a").unwrap();
        std::fs::write(dir.path().join("b"), b"b").unwrap();
        let parent = File::open(dir.path()).unwrap();

        assert_eq!(rename_noreplace(&parent, c"a", c"b"), Err(Errno::EEXIST));
        assert_eq!(std::fs::read(dir.path().join("a")).unwrap(), b"a");
        assert_eq!(std::fs::read(dir.path().join("b")).unwrap(), b"b");
        assert_eq!(rename_noreplace(&parent, c"a", c"c"), Ok(true));
        assert_eq!(std::fs::read(dir.path().join("c")).unwrap(), b"a");
    }
}
