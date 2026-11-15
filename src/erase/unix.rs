//! unix erase: walks a tree through open directory descriptors and erases each entry.
//! entries are stat'ed, opened and renamed relative to a descriptor, never by a full path. files and
//! directories are opened with an identity check, and every entry is renamed to a hidden name and
//! checked again before it is deleted. `real` (the path of the opened parent plus names) is used only
//! for mount lookups.

use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::dir::Dir;
use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat, stat};

use crate::log::{Kind, Log, Outcome};
use crate::verify::{self, Expected};

use super::Options;
use super::at::{
    file_kind, hide_entry, is_directory_at, keep_partly_overwritten, make_owner_accessible,
    open_checked, remove_checked, remove_hidden,
};
use super::hard_link::{HardLinkRule, check_hard_links, hard_link_rule, running_as_root};
use super::interrupt::check_interrupted;
use super::mount::{MountTable, is_boundary};
use super::overwrite::overwrite;
use super::report::{log_done, log_object, log_stopped, unseen};
use super::walk::{Walk, is_excluded};

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
///
/// entries matching `excludes` are kept on purpose: their directories are kept too, and the result is
/// `Ok(true)` (something was kept because of `--exclude`), which is not a failure.
#[cfg(unix)]
pub fn erase_path(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
) -> io::Result<bool> {
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
    let dir = File::open(parent).map_err(|err| unseen(log, path, err))?;
    let entry =
        CString::new(name.as_bytes()).map_err(|err| unseen(log, path, io::Error::other(err)))?;
    let st = fstatat(&dir, entry.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|err| unseen(log, path, io::Error::from(err)))?;
    let parent_st = fstat(&dir).map_err(|err| unseen(log, path, io::Error::from(err)))?;
    let real = real_path(&dir, &parent_st, parent)
        .map_err(|err| unseen(log, path, err))?
        .join(name);
    // read for each argument, so a mount made while an earlier one was erased is seen.
    let mounts = MountTable::read();
    // a mount point cannot be renamed away: refuse before erasing anything inside it.
    let boundary =
        is_boundary(&st, &parent_st, &real, &mounts).map_err(|err| unseen(log, path, err))?;
    if boundary {
        let err =
            io::Error::other("is a mount point or on another filesystem, refusing to erase it");
        let (kind, size) = stat_kind(&st);
        log_object(
            log,
            path,
            kind,
            size,
            Outcome::Refused,
            &err.to_string(),
            &assess_selected(&real, &st),
        );
        return Err(err);
    }
    let tree_links =
        if running_as_root() && !opts.allow_protected && file_kind(&st) == SFlag::S_IFDIR {
            super::tree_links::TreeLinks::collect(&dir, &entry, &st, &real, excludes, &mounts)
                .map_err(|err| unseen(log, path, err))?
        } else {
            super::tree_links::TreeLinks::default()
        };
    let walk = Walk {
        rel: Path::new(""),
        excludes,
        log,
        mounts: &mounts,
        tree_links: &tree_links,
    };
    erase_object(&dir, entry.as_c_str(), &st, path, &real, walk, opts)
}

/// the absolute path of the open directory `dir`, for mount lookups. linux reads it from the
/// descriptor; elsewhere `parent` is canonicalized. either way the result must still be `dir`
/// (`dir_st`), or the lookup would be made for another directory.
#[cfg(unix)]
fn real_path(dir: &File, dir_st: &FileStat, parent: &Path) -> io::Result<PathBuf> {
    let real = opened_path(dir, parent)?;
    let now = stat(&real)?;
    if now.st_dev != dir_st.st_dev || now.st_ino != dir_st.st_ino {
        return Err(io::Error::other(format!(
            "{} is not the directory that was opened",
            real.display()
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

#[cfg(all(unix, not(target_os = "linux")))]
fn opened_path(_dir: &File, parent: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(parent)
}

/// erases one entry and writes its line to the log. `st` is its stat, taken without following links.
#[cfg(unix)]
fn erase_object(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    log_stopped(walk.log)?;
    let (kind, size) = stat_kind(st);
    let assessment = assess_selected(real, st);
    let rule = hard_link_rule(running_as_root(), st.st_nlink > 1, opts.allow_protected);
    let refused = kind == Kind::Other
        || (kind == Kind::File
            && rule == HardLinkRule::Refuse
            && !walk.tree_links.contains_all(st));
    let unlink_only =
        kind == Kind::File && rule == HardLinkRule::Refuse && walk.tree_links.contains_all(st);
    let warning = if kind == Kind::File && rule == HardLinkRule::EraseWithWarning {
        "hard-linked file under root, erased because --i-know-what-i-am-doing was given"
    } else {
        ""
    };
    // checked before each object, so nothing new is started after a signal.
    let result =
        check_interrupted().and_then(|()| erase_entry(parent, name, st, shown, real, walk, opts));
    if result.as_ref().is_ok_and(|kept| !kept) && unlink_only {
        if let Some(log) = walk.log {
            log.record_unlinked(shown, size, &assessment);
        }
        return result;
    }
    log_done(
        walk.log,
        shown,
        kind,
        size,
        &result,
        refused,
        warning,
        &assessment,
    );
    result
}

/// kind of an entry from its stat, and its size: only regular files have one.
#[cfg(unix)]
fn stat_kind(st: &FileStat) -> (Kind, u64) {
    let kind = file_kind(st);
    if kind == SFlag::S_IFREG {
        (Kind::File, u64::try_from(st.st_size).unwrap_or(0))
    } else if kind == SFlag::S_IFDIR {
        (Kind::Dir, 0)
    } else if kind == SFlag::S_IFLNK {
        (Kind::Link, 0)
    } else {
        (Kind::Other, 0)
    }
}

#[allow(clippy::unnecessary_cast)]
fn assess_selected(path: &Path, st: &FileStat) -> crate::assessment::Assessment {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.dev() == st.st_dev as u64 && meta.ino() == st.st_ino as u64 => {
            crate::assessment::Assessment::for_path(path, &meta)
        }
        _ => crate::assessment::Assessment::unknown(),
    }
}

/// writes a skipped line for an entry that is not erased. the entry is only stat'ed for the log.
#[cfg(unix)]
fn log_skipped_at(
    log: Option<&Log>,
    dir: &File,
    name: &CStr,
    shown: &Path,
    real: &Path,
    detail: &str,
) {
    let stat = fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW).ok();
    let (kind, size) = stat.as_ref().map_or((Kind::Other, 0), stat_kind);
    let assessment = stat
        .as_ref()
        .map_or_else(crate::assessment::Assessment::unknown, |st| {
            assess_selected(real, st)
        });
    log_object(
        log,
        shown,
        kind,
        size,
        Outcome::Skipped,
        detail,
        &assessment,
    );
}

/// erases `name` inside the open directory `parent`. `st` is its stat, taken without following links.
/// a link is never opened: it is hidden, checked against `st` and deleted like the other entries.
#[cfg(unix)]
fn erase_entry(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let kind = file_kind(st);
    if kind == SFlag::S_IFLNK {
        remove_checked(parent, name, st, shown).map(|()| false)
    } else if kind == SFlag::S_IFDIR {
        erase_dir_at(parent, name, st, shown, real, walk, opts)
    } else if kind == SFlag::S_IFREG {
        if running_as_root()
            && !opts.allow_protected
            && st.st_nlink > 1
            && walk.tree_links.contains_all(st)
        {
            // remove redundant names without writing shared data; overwrite only the last name.
            remove_checked(parent, name, st, shown)?;
            walk.tree_links.removed(st);
            return Ok(false);
        }
        erase_file_at(parent, name, st, shown, opts).map(|()| false)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not a regular file, directory or symbolic link",
        ))
    }
}

/// erases the contents of a directory, then removes it. the directory is opened with an identity
/// check, and hidden and checked again before it is removed; the removal fails on a non-empty directory.
#[cfg(unix)]
fn erase_dir_at(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    make_owner_accessible(parent, name, st, Mode::S_IRWXU)?;
    let dir = open_checked(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    if erase_contents(&dir, st, shown, real, walk, opts)? {
        return Ok(true);
    }
    if opts.verbose {
        crate::ui::message!("  removing directory {}", shown.display());
    }
    remove_checked(parent, name, st, shown).map(|()| false)
}

/// erases every entry of the open directory `dir`, whose stat is `dir_st`. a failing entry is
/// reported and skipped, and the directory is kept. returns true when an entry was kept because of
/// `--exclude`, here or in a subdirectory; the caller then keeps this directory too.
#[cfg(unix)]
fn erase_contents(
    dir: &File,
    dir_st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let names = list_names(dir)?;
    let mut failed = 0usize;
    let mut excluded = 0usize;
    let mut kept_inside = false;
    for name in &names {
        let entry = OsStr::from_bytes(name.to_bytes());
        let entry_shown = shown.join(entry);
        let entry_real = real.join(entry);
        let entry_rel = walk.rel.join(entry);
        // an excluded directory is not entered either.
        if is_excluded(walk.excludes, &entry_rel) {
            excluded += 1;
            log_skipped_at(
                walk.log,
                dir,
                name,
                &entry_shown,
                &entry_real,
                "excluded by --exclude",
            );
            continue;
        }
        let child = Walk {
            rel: &entry_rel,
            excludes: walk.excludes,
            log: walk.log,
            mounts: walk.mounts,
            tree_links: walk.tree_links,
        };
        let result = match fstatat(dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(st) => match is_boundary(&st, dir_st, &entry_real, walk.mounts) {
                Ok(true) => {
                    let (kind, size) = stat_kind(&st);
                    log_object(
                        walk.log,
                        &entry_shown,
                        kind,
                        size,
                        Outcome::Skipped,
                        "mount point or other filesystem, skipped",
                        &assess_selected(&entry_real, &st),
                    );
                    Err(io::Error::other(
                        "is a mount point or on another filesystem, skipped",
                    ))
                }
                Ok(false) => erase_object(dir, name, &st, &entry_shown, &entry_real, child, opts),
                Err(err) => Err(unseen(walk.log, &entry_shown, err)),
            },
            Err(err) => Err(unseen(walk.log, &entry_shown, io::Error::from(err))),
        };
        match result {
            Ok(inside_kept) => kept_inside |= inside_kept,
            Err(err) => {
                // an interrupt ends the walk. only the innermost entry is reported; directories above
                // it just pass the error up.
                if err.kind() == io::ErrorKind::Interrupted {
                    if !is_directory_at(dir, name) {
                        crate::ui::failure!("{}: {err}", entry_shown.display());
                    }
                    return Err(err);
                }
                crate::ui::failure!("{}: {err}", entry_shown.display());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            shown.display()
        )));
    }
    if excluded > 0 {
        crate::ui::message!(
            "kept {}: {excluded} entries excluded by --exclude",
            shown.display()
        );
    }
    Ok(excluded > 0 || kept_inside)
}

/// names in `dir` except `.` and `..`, read before anything is erased.
#[cfg(unix)]
pub(super) fn list_names(dir: &File) -> io::Result<Vec<CString>> {
    // fdopendir takes ownership of the descriptor, so list a duplicate.
    let mut listing = Dir::from_fd(OwnedFd::from(dir.try_clone()?))?;
    let mut names = Vec::new();
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// overwrites the file's contents, then deletes it. the name is moved away first, so a file that
/// cannot be deleted is refused before anything is overwritten.
#[cfg(unix)]
fn erase_file_at(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    opts: &Options,
) -> io::Result<()> {
    check_hard_links(shown, st.st_nlink > 1, opts)?;
    // --verify reads the file back, so it needs the read bit too.
    let owner_bits = if opts.verify {
        Mode::S_IRUSR | Mode::S_IWUSR
    } else {
        Mode::S_IWUSR
    };
    make_owner_accessible(parent, name, st, owner_bits)?;
    let hidden = hide_entry(parent, name, st, shown)?;
    overwrite_hidden(parent, &hidden, st, opts)
        .map_err(|err| keep_partly_overwritten(parent, name, &hidden, shown, err))?;
    remove_hidden(parent, name, &hidden, false, shown)
}

/// overwrites the contents of the renamed entry `hidden`, through a descriptor checked against `st`.
#[cfg(unix)]
fn overwrite_hidden(parent: &File, hidden: &CStr, st: &FileStat, opts: &Options) -> io::Result<()> {
    let access = if opts.verify {
        OFlag::O_RDWR
    } else {
        OFlag::O_WRONLY
    };
    // nonblock: a fifo swapped in must not block the open. noctty: a tty swapped in must not become
    // the controlling terminal.
    let mut file = open_checked(
        parent,
        hidden,
        access | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_NOCTTY | OFlag::O_CLOEXEC,
        st,
    )?;
    if !opts.allow_protected {
        check_hard_links(Path::new("opened file"), fstat(&file)?.st_nlink > 1, opts)?;
    }
    // length from the open handle, not from a second lookup.
    let len = file.metadata()?.len();
    let ranges = super::sparse::data_ranges(&file, len)?;
    let expected = opts.verify.then(Expected::new);
    overwrite(&mut file, &ranges, opts, expected.as_ref()).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("{err}; the contents may be partly overwritten"),
        )
    })?;
    match &expected {
        Some(expected) => verify::check_ranges(&mut file, expected, len, &ranges),
        None => Ok(()),
    }
}
