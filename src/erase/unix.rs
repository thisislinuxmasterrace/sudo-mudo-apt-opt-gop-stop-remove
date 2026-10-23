//! unix erase: the walk of a tree through open directory descriptors, and the erase of each entry.
//! no entry is looked up by a path below an open directory, so a link swapped in during the walk
//! cannot redirect it.

use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::dir::Dir;
use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use crate::log::{Kind, Log, Outcome};
use crate::verify::{self, Expected};

use super::Options;
use super::at::{
    file_kind, hide_entry, is_directory_at, keep_partly_overwritten, make_owner_accessible,
    open_checked, remove_at, remove_hidden,
};
use super::hard_link::{HardLinkRule, check_hard_links, hard_link_rule, running_as_root};
use super::interrupt::check_interrupted;
use super::mount::is_boundary;
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
    // the parent is named by the user, so links in it are followed. the entry is looked up
    // only relative to open directory descriptors, so no later lookup can follow a link.
    let dir = File::open(parent).map_err(|err| unseen(log, path, err))?;
    let entry =
        CString::new(name.as_bytes()).map_err(|err| unseen(log, path, io::Error::other(err)))?;
    let st = fstatat(&dir, entry.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|err| unseen(log, path, io::Error::from(err)))?;
    let real = fs::canonicalize(parent)
        .map_err(|err| unseen(log, path, err))?
        .join(name);
    // a mount point cannot be renamed away, so it is refused before anything inside it is erased.
    let boundary = fstat(&dir)
        .map_err(io::Error::from)
        .and_then(|parent_st| is_boundary(&st, &parent_st, &real))
        .map_err(|err| unseen(log, path, err))?;
    if boundary {
        let err =
            io::Error::other("is a mount point or on another filesystem, refusing to erase it");
        let (kind, size) = stat_kind(&st);
        log_object(log, path, kind, size, Outcome::Refused, &err.to_string());
        return Err(err);
    }
    let walk = Walk {
        rel: Path::new(""),
        excludes,
        log,
    };
    erase_object(&dir, entry.as_c_str(), &st, path, &real, walk, opts)
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
    let rule = hard_link_rule(running_as_root(), st.st_nlink > 1, opts.allow_protected);
    let refused = kind == Kind::Other || (kind == Kind::File && rule == HardLinkRule::Refuse);
    let warning = if kind == Kind::File && rule == HardLinkRule::EraseWithWarning {
        "hard-linked file under root, erased because --i-know-what-i-am-doing was given"
    } else {
        ""
    };
    // checked before each object, so an object the signal reaches is not touched.
    let result =
        check_interrupted().and_then(|()| erase_entry(parent, name, st, shown, real, walk, opts));
    log_done(walk.log, shown, kind, size, &result, refused, warning);
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

/// writes a skipped line for an entry that is not erased. the entry is only stat'ed for the log.
#[cfg(unix)]
fn log_skipped_at(log: Option<&Log>, dir: &File, name: &CStr, shown: &Path, detail: &str) {
    if log.is_none() {
        return;
    }
    let (kind, size) = fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_or((Kind::Other, 0), |st| stat_kind(&st));
    log_object(log, shown, kind, size, Outcome::Skipped, detail);
}

/// erases `name` inside the open directory `parent`. `st` is its stat, taken without following links.
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
        unlinkat(parent, name, UnlinkatFlags::NoRemoveDir)
            .map(|()| false)
            .map_err(io::Error::from)
    } else if kind == SFlag::S_IFDIR {
        erase_dir_at(parent, name, st, shown, real, walk, opts)
    } else if kind == SFlag::S_IFREG {
        erase_file_at(parent, name, st, shown, opts).map(|()| false)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not a regular file, directory or symbolic link",
        ))
    }
}

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
        eprintln!("  removing directory {}", shown.display());
    }
    remove_at(parent, name, true, shown).map(|()| false)
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
        // excluded entries are not walked, so an excluded directory is not walked either.
        if is_excluded(walk.excludes, &entry_rel) {
            excluded += 1;
            log_skipped_at(walk.log, dir, name, &entry_shown, "excluded by --exclude");
            continue;
        }
        let child = Walk {
            rel: &entry_rel,
            excludes: walk.excludes,
            log: walk.log,
        };
        let result = match fstatat(dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(st) => match is_boundary(&st, dir_st, &entry_real) {
                Ok(true) => {
                    let (kind, size) = stat_kind(&st);
                    log_object(
                        walk.log,
                        &entry_shown,
                        kind,
                        size,
                        Outcome::Skipped,
                        "mount point or other filesystem, skipped",
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
                // an interrupt ends the walk: the entries after this one are left as they are. only the
                // innermost entry is named, the directories above it are not repeated.
                if err.kind() == io::ErrorKind::Interrupted {
                    if !is_directory_at(dir, name) {
                        eprintln!("{}: {err}", entry_shown.display());
                    }
                    return Err(err);
                }
                eprintln!("{}: {err}", entry_shown.display());
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
        eprintln!(
            "kept {}: {excluded} entries excluded by --exclude",
            shown.display()
        );
    }
    Ok(excluded > 0 || kept_inside)
}

/// names in `dir` except `.` and `..`, read before anything is erased.
#[cfg(unix)]
fn list_names(dir: &File) -> io::Result<Vec<CString>> {
    // fdopendir takes ownership of its descriptor, so the listing reads a duplicate.
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
    // the read-back needs the owner's read bit as well as the write bit.
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
    // nonblock: a fifo swapped in for the file must not block the open. noctty: it must not
    // become the controlling terminal of this process.
    let mut file = open_checked(
        parent,
        hidden,
        access | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_NOCTTY | OFlag::O_CLOEXEC,
        st,
    )?;
    // the length comes from the handle being overwritten, not from a second lookup by name.
    let whole = 0..file.metadata()?.len();
    let expected = opts.verify.then(Expected::new);
    overwrite(
        &mut file,
        std::slice::from_ref(&whole),
        opts,
        expected.as_ref(),
    )
    .map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("{err}; the contents may be partly overwritten"),
        )
    })?;
    match &expected {
        Some(expected) => verify::check(&mut file, expected, whole.end),
        None => Ok(()),
    }
}
