//! windows erase of one file: the name is changed through the open handle, the contents are
//! overwritten in place, and the file is deleted through the same handle with posix semantics.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::platform::windows::{self, FileId};
use crate::prompt;
use crate::verify::{self, Expected};

use crate::erase::hard_link::check_hard_links;
use crate::erase::interrupt::check_interrupted;
use crate::erase::overwrite::overwrite;
use crate::erase::{Options, streams};

/// the read-only flag cleared before an erase. it is put back when the guard is dropped, unless
/// `done` was called because the entry is gone, so an entry that stays keeps its flag.
pub(super) struct ReadOnly {
    /// where the entry is now: its own path, or the hidden name while it is renamed.
    at: PathBuf,
    cleared: bool,
}

impl ReadOnly {
    pub(super) fn clear(path: &Path) -> io::Result<Self> {
        Ok(Self {
            cleared: make_writable(path)?,
            at: path.to_path_buf(),
        })
    }

    fn moved(&mut self, to: &Path) {
        to.clone_into(&mut self.at);
    }

    /// the entry was deleted, or what is at its place is not it any more: there is nothing to put the
    /// flag back on.
    pub(super) fn done(&mut self) {
        self.cleared = false;
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        if self.cleared {
            restore_readonly(&self.at);
        }
    }
}

/// puts the read-only flag back on an entry that stays. best effort, and never through a link.
fn restore_readonly(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() || crate::entry::is_unsupported_reparse(&meta) {
        return;
    }
    let mut perms = meta.permissions();
    perms.set_readonly(true);
    let _ = fs::set_permissions(path, perms);
}

/// overwrites the file's contents, then deletes it. if the delete fails, the contents are already
/// gone and the file remains.
pub(super) fn erase_file(path: &Path, opts: &Options) -> io::Result<()> {
    let mut keep = ReadOnly::clear(path)?;
    match open_for_overwrite(path) {
        Ok(file) => erase_open(path, file, opts, &mut keep),
        Err(err) if is_locked(&err) => super::locked::erase_locked(path, opts, err, &mut keep),
        Err(err) => Err(err),
    }
}

/// the new name of a renamed file, and the name to give back.
struct Hidden {
    original: PathBuf,
    hidden: PathBuf,
}

/// renames the open file to a random hidden name in its directory, so the original name does not
/// stay while the contents are overwritten. `None` when that fails: the erase then goes on under
/// the original name, as best effort.
fn hide(file: &File) -> Option<Hidden> {
    // the directory and the name come from the open handle, not from the path, so a link swapped in
    // for the name or a directory meanwhile cannot change where the name is restored.
    let original = windows::final_path_of(file).ok()?;
    let dir = original.parent()?.to_path_buf();
    let hidden = dir.join(format!(".{:016x}", rand::random::<u64>()));
    windows::rename_by_handle(file, &hidden).ok()?;
    Some(Hidden { original, hidden })
}

/// gives a kept file its name back through the handle. the error says where the entry is left when
/// that fails.
fn keep_under_name(
    hidden: Option<&Hidden>,
    file: &File,
    keep: &mut ReadOnly,
    err: io::Error,
) -> io::Error {
    let Some(hidden) = hidden else {
        return err;
    };
    match windows::rename_by_handle(file, &hidden.original) {
        Ok(()) => {
            keep.moved(&hidden.original);
            err
        }
        Err(_) => left_as(err, &hidden.hidden),
    }
}

/// `err` with the name of the entry that stays under a hidden name.
fn left_as(err: io::Error, hidden: &Path) -> io::Error {
    io::Error::new(
        err.kind(),
        format!("{err}; the entry is left as {}", crate::ui::shown(hidden)),
    )
}

/// deletes through the erase handle: posix semantics first, then the classic delete-on-close for
/// filesystems that have no posix delete. true when the name is gone or goes when the handle closes.
fn delete_through_handle(file: &File) -> bool {
    delete_through_with(file, windows::posix_delete, windows::delete_on_close)
}

fn delete_through_with(
    file: &File,
    posix: impl FnOnce(&File) -> io::Result<()>,
    classic: impl FnOnce(&File) -> io::Result<()>,
) -> bool {
    posix(file).is_ok() || classic(file).is_ok()
}

/// erases an open file: renames it, overwrites it and its named streams, then deletes it through the
/// handle. the path-based delete runs only when that fails.
pub(super) fn erase_open(
    path: &Path,
    mut file: File,
    opts: &Options,
    keep: &mut ReadOnly,
) -> io::Result<()> {
    // length from the open handle, not from a second lookup.
    let len = file.metadata()?.len();
    check_hard_links(path, windows::link_count_of(&file).unwrap_or(1) > 1, opts)?;
    let (names, mut streams) = streams::open_named(path, &file)?;
    // what the path-based delete must find at the name before it removes anything.
    let id = windows::file_id_of(&file)?;
    let hidden = hide(&file);
    let at = hidden
        .as_ref()
        .map_or(path, |hidden| hidden.hidden.as_path());
    keep.moved(at);
    if let Err(err) = overwrite_all(at, &mut file, len, &names, &mut streams, opts) {
        return Err(keep_under_name(hidden.as_ref(), &file, keep, err));
    }
    // the stream handles are not needed any more; the delete should not depend on them.
    drop(streams);
    // still under the hidden name here. classic delete-on-close takes effect when the last handle
    // closes, which is this one.
    if delete_through_handle(&file) {
        drop(file);
        keep.done();
        return Ok(());
    }

    // the handle could not delete: give the name back and use the path-based fallbacks.
    let (name, stuck_hidden) = match &hidden {
        Some(hidden) if windows::rename_by_handle(&file, &hidden.original).is_ok() => {
            (hidden.original.clone(), None)
        }
        Some(hidden) => (hidden.hidden.clone(), Some(hidden.hidden.as_path())),
        None => (path.to_path_buf(), None),
    };
    keep.moved(&name);
    drop(file);
    remove_entry(&name, id, opts, keep).map_err(|err| match stuck_hidden {
        Some(hidden) => left_as(err, hidden),
        None => err,
    })?;
    keep.done();
    Ok(())
}

/// overwrites the named streams and then the main stream. `at` is where the file is now.
fn overwrite_all(
    at: &Path,
    file: &mut File,
    len: u64,
    names: &[OsString],
    streams: &mut [File],
    opts: &Options,
) -> io::Result<()> {
    for (name, stream) in names.iter().zip(streams) {
        let size = stream.metadata()?.len();
        overwrite_in_place(stream, size, opts)
            .and_then(|()| unchanged_len(stream, size))
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "data stream {}: {err}",
                        crate::ui::shown_str(&name.to_string_lossy())
                    ),
                )
            })?;
    }
    overwrite_in_place(file, len, opts)?;
    unchanged_len(file, len)?;
    if streams::open_named(at, file)?.0 != names {
        return Err(io::Error::other(
            "data streams changed during overwrite; the file is kept",
        ));
    }
    Ok(())
}

/// another writer may have appended meanwhile: its tail was never overwritten, so the file stays.
fn unchanged_len(file: &File, len: u64) -> io::Result<()> {
    if file.metadata()?.len() != len {
        return Err(io::Error::other(
            "the file changed size during the overwrite, the file is kept",
        ));
    }
    Ok(())
}

/// overwrites a file in place; an interrupted overwrite leaves the contents partly written.
fn overwrite_in_place(file: &mut File, len: u64, opts: &Options) -> io::Result<()> {
    let whole = 0..len;
    let expected = opts.verify.then(|| Expected::new(opts.zero));
    overwrite(file, std::slice::from_ref(&whole), opts, expected.as_ref()).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("{err}; the contents may be partly overwritten, the file is kept"),
        )
    })?;
    match &expected {
        Some(expected) => verify::check(file, expected, len)
            .map_err(|err| io::Error::new(err.kind(), format!("{err}, the file is kept"))),
        None => Ok(()),
    }
}

pub(super) fn open_for_overwrite(path: &Path) -> io::Result<File> {
    windows::open_for_erase(path)
}

/// sharing violation (32), lock violation (33), or access denied (5, which windows also returns
/// for delete-pending files).
fn is_locked(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

/// deletes the entry by path, when the open handle could not: posix delete first (removes the name
/// even while other handles are open), then rename and delete, then deletion at the next reboot
/// (asked first; never with `-f`). the handle opened for the delete must be the erased file, `expected`;
/// anything else at the path is left alone.
fn remove_entry(
    path: &Path,
    expected: FileId,
    opts: &Options,
    keep: &mut ReadOnly,
) -> io::Result<()> {
    if let Ok(handle) = windows::open_for_delete(path) {
        if windows::file_id_of(&handle).ok() != Some(expected) {
            keep.done();
            return Err(io::Error::other(
                "the entry at this name is not the file that was erased, it is kept",
            ));
        }
        if windows::posix_delete(&handle).is_ok() {
            return Ok(());
        }
    }
    let delete_err = match unlink_with_random_name(path, false) {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };

    if !prompt::confirmed_action(
        || {
            if opts.force {
                Ok(false)
            } else {
                prompt::ask_interruptible(
                    "file is still in use and cannot be deleted now (contents are already erased). schedule deletion at next reboot?",
                )
            }
        },
        check_interrupted,
        || windows::delete_on_reboot(path),
    )? {
        return Err(io::Error::other(format!(
            "contents are erased, the file remains ({delete_err}) and is not scheduled for deletion at reboot"
        )));
    }
    Ok(())
}

/// clears the read-only flag so the entry can be overwritten and renamed. returns whether it was set.
fn make_writable(path: &Path) -> io::Result<bool> {
    // permissions change by path: refuse a symlink here (check-then-change, a small race remains).
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::other(
            "refusing to change permissions of a symbolic link",
        ));
    }
    if crate::entry::is_unsupported_reparse(&meta) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported reparse point, refusing to change permissions",
        ));
    }
    let mut perms = meta.permissions();
    if !perms.readonly() {
        return Ok(false);
    }
    set_writable(&mut perms);
    fs::set_permissions(path, perms)?;
    Ok(true)
}

fn set_writable(perms: &mut fs::Permissions) {
    perms.set_readonly(false);
}

/// renames the entry to a random hidden name in the same directory, then deletes it, so the
/// original name does not stay. by path, with no identity check.
pub(super) fn unlink_with_random_name(path: &Path, is_dir: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let hidden = loop {
        let candidate = dir.join(format!(".{:016x}", rand::random::<u64>()));
        if !candidate.exists() {
            break candidate;
        }
    };

    fs::rename(path, &hidden)?;
    let removed = if is_dir {
        fs::remove_dir(&hidden)
    } else {
        fs::remove_file(&hidden)
    };
    match removed {
        Ok(()) => Ok(()),
        Err(remove_err) => {
            // put the name back; if that fails, report where the entry is.
            match fs::rename(&hidden, path) {
                Ok(()) => Err(remove_err),
                Err(_) => Err(io::Error::other(format!(
                    "{remove_err}; the entry is left as {}",
                    crate::ui::shown(&hidden)
                ))),
            }
        }
    }
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
