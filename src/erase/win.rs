//! windows erase: the walk by path, the handling of files held by another process, and the posix
//! delete. the lock handling calls into `crate::platform::windows`.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use crate::log::{Kind, Log, Outcome};
use crate::platform::windows;
use crate::prompt;
use crate::verify::{self, Expected};

use super::Options;
use super::hard_link::check_hard_links;
use super::interrupt::check_interrupted;
use super::mount::{device, is_mount_boundary, link_count};
use super::overwrite::overwrite;
use super::report::{log_done, log_object, log_stopped, unseen};
use super::walk::{Walk, is_excluded};

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
#[cfg(windows)]
pub fn erase_path(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
) -> io::Result<bool> {
    erase_within(
        path,
        Walk {
            rel: Path::new(""),
            excludes,
            log,
        },
        opts,
    )
}

#[cfg(windows)]
fn erase_within(path: &Path, walk: Walk<'_>, opts: &Options) -> io::Result<bool> {
    log_stopped(walk.log)?;
    let meta = fs::symlink_metadata(path).map_err(|err| unseen(walk.log, path, err))?;
    let (kind, size) = meta_kind(&meta);
    let result = check_interrupted().and_then(|()| erase_kind(path, &meta, walk, opts));
    log_done(walk.log, path, kind, size, &result, kind == Kind::Other, "");
    result
}

#[cfg(windows)]
fn erase_kind(
    path: &Path,
    meta: &fs::Metadata,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let file_type = meta.file_type();

    if file_type.is_symlink() {
        return fs::remove_file(path)
            .or_else(|file_err| fs::remove_dir(path).map_err(|_| file_err))
            .map(|()| false);
    }
    if file_type.is_dir() {
        return erase_dir(path, walk, opts);
    }
    if file_type.is_file() {
        return erase_file(path, opts).map(|()| false);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not a regular file, directory or symbolic link",
    ))
}

/// kind of an entry from its metadata, and its size: only regular files have one.
#[cfg(windows)]
fn meta_kind(meta: &fs::Metadata) -> (Kind, u64) {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        (Kind::Link, 0)
    } else if file_type.is_dir() {
        (Kind::Dir, 0)
    } else if file_type.is_file() {
        (Kind::File, meta.len())
    } else {
        (Kind::Other, 0)
    }
}

/// writes a skipped line for an entry that is not erased. the entry is only stat'ed for the log.
#[cfg(windows)]
fn log_skipped_path(log: Option<&Log>, path: &Path, detail: &str) {
    if log.is_none() {
        return;
    }
    let (kind, size) = fs::symlink_metadata(path).map_or((Kind::Other, 0), |meta| meta_kind(&meta));
    log_object(log, path, kind, size, Outcome::Skipped, detail);
}

#[cfg(windows)]
fn erase_dir(path: &Path, walk: Walk<'_>, opts: &Options) -> io::Result<bool> {
    make_writable(path, true)?;

    // collect entries first so the directory is not modified while it is being iterated.
    let entries: Vec<fs::DirEntry> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
    // never erase what is mounted inside the tree: that is another disk or the source of a bind mount.
    let parent_dev = device(&fs::symlink_metadata(path)?);
    let mut failed = 0usize;
    let mut excluded = 0usize;
    let mut kept_inside = false;
    for entry in &entries {
        let entry_path = entry.path();
        let entry_rel = walk.rel.join(entry.file_name());
        // excluded entries are not walked, so an excluded directory is not walked either.
        if is_excluded(walk.excludes, &entry_rel) {
            excluded += 1;
            log_skipped_path(walk.log, &entry_path, "excluded by --exclude");
            continue;
        }
        let child = Walk {
            rel: &entry_rel,
            excludes: walk.excludes,
            log: walk.log,
        };
        let result = match is_mount_boundary(&entry_path, parent_dev) {
            Ok(true) => {
                log_skipped_path(
                    walk.log,
                    &entry_path,
                    "mount point or other filesystem, skipped",
                );
                Err(io::Error::other(
                    "is a mount point or on another filesystem, skipped",
                ))
            }
            Ok(false) => erase_within(&entry_path, child, opts),
            Err(err) => Err(unseen(walk.log, &entry_path, err)),
        };
        match result {
            Ok(inside_kept) => kept_inside |= inside_kept,
            Err(err) => {
                if err.kind() == io::ErrorKind::Interrupted {
                    if !entry_path.is_dir() {
                        eprintln!("{}: {err}", entry_path.display());
                    }
                    return Err(err);
                }
                eprintln!("{}: {err}", entry_path.display());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            path.display()
        )));
    }
    if excluded > 0 {
        eprintln!(
            "kept {}: {excluded} entries excluded by --exclude",
            path.display()
        );
    }
    if excluded > 0 || kept_inside {
        return Ok(true);
    }

    if opts.verbose {
        eprintln!("  removing directory {}", path.display());
    }
    unlink_with_random_name(path, true).map(|()| false)
}

/// overwrites the file's contents, then deletes it.
///
/// if deletion fails after the overwrite, the contents are already destroyed
/// but the file itself remains.
#[cfg(windows)]
fn erase_file(path: &Path, opts: &Options) -> io::Result<()> {
    make_writable(path, false)?;

    match open_for_overwrite(path) {
        Ok(mut file) => {
            // the length comes from the handle being overwritten, not from a second lookup by name.
            let meta = file.metadata()?;
            check_hard_links(path, link_count(&meta) > 1, opts)?;
            overwrite_in_place(&mut file, meta.len(), opts)?;
            drop(file);
            remove_entry(path, opts)
        }
        Err(err) if is_locked(&err) => erase_locked(path, opts, err),
        Err(err) => Err(err),
    }
}

/// why a locked file has no listed holder. code 5 is also an acl or delete-pending state, so it
/// does not claim a lock; 32 and 33 are sharing and lock violations, held by something unlisted.
#[cfg(windows)]
fn no_holder_reason(err: &io::Error) -> &'static str {
    match err.raw_os_error() {
        Some(32 | 33) => {
            "the file is in use, but no process is listed (a service, driver or antivirus may hold it)"
        }
        _ => {
            "no lock holder was listed, so the file may be pending deletion or its permissions may deny access"
        }
    }
}

/// handles a file that another process holds open.
#[cfg(windows)]
fn erase_locked(path: &Path, opts: &Options, err: io::Error) -> io::Result<()> {
    let len = fs::symlink_metadata(path)?.len();
    let listing = windows::holders(path);
    let holders = listing.as_deref().unwrap_or_default();
    for holder in holders {
        eprintln!(
            "{}: locked by {} (pid {})",
            path.display(),
            holder.name,
            holder.pid
        );
    }

    if opts.kill_holders && !holders.is_empty() {
        if !opts.force
            && !prompt::ask(
                "close these processes to unlock the file? unsaved data in them is lost",
            )
        {
            return Err(io::Error::other("skipped, file is still locked"));
        }
        eprintln!(
            "warning: closing {} process(es) may lose unsaved data and stop services",
            holders.len()
        );
        windows::close_holders(path)?;

        match open_for_overwrite(path) {
            Ok(mut file) => {
                overwrite_in_place(&mut file, len, opts)?;
                drop(file);
                return remove_entry(path, opts);
            }
            Err(retry_err) => {
                // the forced shutdown can leave holders or let new ones open the file: list them again.
                let remaining = windows::holders(path);
                for holder in remaining.as_deref().unwrap_or_default() {
                    eprintln!(
                        "{}: still locked by {} (pid {})",
                        path.display(),
                        holder.name,
                        holder.pid
                    );
                }
                return Err(match &remaining {
                    Ok(list) if list.is_empty() => {
                        io::Error::other(format!("{retry_err}; {}", no_holder_reason(&retry_err)))
                    }
                    Ok(_) => {
                        io::Error::other(format!("{retry_err}; still locked after closing holders"))
                    }
                    Err(list_err) => io::Error::other(format!(
                        "{retry_err}; could not list lock holders: {list_err}"
                    )),
                });
            }
        }
    }

    Err(match &listing {
        Ok(list) if list.is_empty() => {
            io::Error::other(format!("{err}; {}", no_holder_reason(&err)))
        }
        Ok(_) => io::Error::other(format!("{err}; use --kill-holders to erase it anyway")),
        Err(list_err) => {
            io::Error::other(format!("{err}; could not list lock holders: {list_err}"))
        }
    })
}

/// overwrites a file in place. windows does not rename it first, so an interrupted overwrite leaves
/// the file under its name.
#[cfg(windows)]
fn overwrite_in_place(file: &mut File, len: u64, opts: &Options) -> io::Result<()> {
    let whole = 0..len;
    let expected = opts.verify.then(Expected::new);
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

#[cfg(windows)]
fn open_for_overwrite(path: &Path) -> io::Result<File> {
    windows::open_for_erase(path)
}

/// sharing violation, lock violation, or access denied (which windows also returns
/// for delete-pending files).
#[cfg(windows)]
fn is_locked(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

/// windows: posix delete first (removes the name even while other handles are open),
/// then rename and delete, then deletion at the next reboot (only if not forced).
#[cfg(windows)]
fn remove_entry(path: &Path, opts: &Options) -> io::Result<()> {
    if let Ok(handle) = windows::open_for_delete(path)
        && windows::posix_delete(&handle).is_ok()
    {
        return Ok(());
    }
    let delete_err = match unlink_with_random_name(path, false) {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };

    if opts.force
        || !prompt::ask(
            "file is still in use and cannot be deleted now (contents are already erased). schedule deletion at next reboot?",
        )
    {
        return Err(io::Error::other(format!(
            "contents are erased, the file remains ({delete_err}) and is not scheduled for deletion at reboot"
        )));
    }
    windows::delete_on_reboot(path)
}

/// clears the read-only flag so the entry can be overwritten and renamed.
#[cfg(windows)]
fn make_writable(path: &Path, is_dir: bool) -> io::Result<()> {
    // permissions are changed through the path, so refuse a symlink that replaced it.
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::other(
            "refusing to change permissions of a symbolic link",
        ));
    }
    // cloud and dedup reparse points get their readonly flag cleared before open_for_erase refuses them.
    let mut perms = meta.permissions();
    if !perms.readonly() {
        return Ok(());
    }
    set_writable(&mut perms, is_dir);
    fs::set_permissions(path, perms)
}

#[cfg(windows)]
fn set_writable(perms: &mut fs::Permissions, _is_dir: bool) {
    perms.set_readonly(false);
}

/// renames the entry to a random hidden name in the same directory, then deletes it,
/// so the original name does not remain in the directory.
#[cfg(windows)]
fn unlink_with_random_name(path: &Path, is_dir: bool) -> io::Result<()> {
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
            // put the name back so the entry is not left hidden; if that fails, report where it is.
            match fs::rename(&hidden, path) {
                Ok(()) => Err(remove_err),
                Err(_) => Err(io::Error::other(format!(
                    "{remove_err}; the entry is left as {}",
                    hidden.display()
                ))),
            }
        }
    }
}
