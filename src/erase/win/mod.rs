//! windows erase: walks by path (no descriptor-relative lookups, so a swap during the walk is not
//! guarded against), compares every directory with the protected identities, handles files held by
//! another process, and deletes through the open handle with posix semantics.
//! exercised on windows ci and a windows vm, not on real windows hardware.

use std::fs;
use std::io;
use std::path::Path;

use crate::log::{Kind, Log, Outcome};
use crate::safety::windows::{Protected, protected};

use super::Options;
use super::mount::{device, is_mount_boundary};
use super::report::{log_done, log_object, log_stopped, unseen};
use super::walk::{Walk, is_excluded};

mod file;
mod locked;

use file::{ReadOnly, erase_file, unlink_with_random_name};

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
pub fn erase_path(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
) -> io::Result<bool> {
    erase_path_with(path, opts, excludes, log, protected())
}

/// `erase_path` with the directories the walk must not enter given by the caller.
fn erase_path_with(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
    protected: &Protected,
) -> io::Result<bool> {
    erase_within(
        path,
        Walk {
            rel: Path::new(""),
            excludes,
            log,
        },
        opts,
        protected,
    )
}

fn erase_within(
    path: &Path,
    walk: Walk<'_>,
    opts: &Options,
    protected: &Protected,
) -> io::Result<bool> {
    super::check_depth(walk.rel)?;
    log_stopped(walk.log)?;
    let (meta, assessment) =
        crate::assessment::Assessment::capture(path).map_err(|err| unseen(walk.log, path, err))?;
    let (kind, size) = meta_kind(&meta);
    let result =
        super::check_interrupted().and_then(|()| erase_kind(path, &meta, walk, opts, protected));
    log_done(
        walk.log,
        path,
        kind,
        size,
        &result,
        kind == Kind::Other,
        "",
        &assessment,
    );
    result
}

fn erase_kind(
    path: &Path,
    meta: &fs::Metadata,
    walk: Walk<'_>,
    opts: &Options,
    protected: &Protected,
) -> io::Result<bool> {
    let file_type = meta.file_type();

    if file_type.is_symlink() {
        return fs::remove_file(path)
            .or_else(|file_err| fs::remove_dir(path).map_err(|_| file_err))
            .map(|()| false);
    }
    if crate::entry::is_unsupported_reparse(meta) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported reparse point, refusing to erase it",
        ));
    }
    if file_type.is_dir() {
        return erase_dir(path, walk, opts, protected);
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
fn meta_kind(meta: &fs::Metadata) -> (Kind, u64) {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        (Kind::Link, 0)
    } else if crate::entry::is_unsupported_reparse(meta) {
        (Kind::Other, 0)
    } else if file_type.is_dir() {
        (Kind::Dir, 0)
    } else if file_type.is_file() {
        (Kind::File, meta.len())
    } else {
        (Kind::Other, 0)
    }
}

/// captures skipped-entry metadata and facts together so the log cannot combine different objects.
fn log_skipped_path(log: Option<&Log>, path: &Path, detail: &str) {
    let (kind, size, assessment) = crate::assessment::Assessment::capture(path).map_or_else(
        |_| (Kind::Other, 0, crate::assessment::Assessment::unknown()),
        |(meta, assessment)| {
            let (kind, size) = meta_kind(&meta);
            (kind, size, assessment)
        },
    );
    log_object(log, path, kind, size, Outcome::Skipped, detail, &assessment);
}

fn erase_dir(
    path: &Path,
    walk: Walk<'_>,
    opts: &Options,
    protected: &Protected,
) -> io::Result<bool> {
    // the identity of every directory the walk enters, so a protected one is kept however it is reached.
    protected.check_dir(path, opts.allow_protected)?;
    if !super::streams::names(path)?.is_empty() {
        return Err(io::Error::other(
            "directory has named data streams; refusing to erase it without full stream coverage",
        ));
    }
    // dropped armed on every return but the removal, so a directory that stays keeps its flag.
    let mut keep = ReadOnly::clear(path)?;

    // list first, so the directory is not changed while it is iterated.
    let entries: Vec<fs::DirEntry> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
    // entries on another device (a mount) are skipped, not erased.
    let parent_dev = device(&fs::symlink_metadata(path)?);
    let mut failed = 0usize;
    let mut excluded = 0usize;
    let mut kept_inside = false;
    for entry in &entries {
        let entry_path = entry.path();
        let entry_rel = walk.rel.join(entry.file_name());
        // an excluded directory is not entered either.
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
            Ok(false) => erase_within(&entry_path, child, opts, protected),
            Err(err) => Err(unseen(walk.log, &entry_path, err)),
        };
        match result {
            Ok(inside_kept) => kept_inside |= inside_kept,
            Err(err) => {
                if err.kind() == io::ErrorKind::Interrupted {
                    if !entry_path.is_dir() {
                        crate::ui::failure!("{}: {err}", crate::ui::shown(&entry_path));
                    }
                    return Err(err);
                }
                crate::ui::failure!("{}: {err}", crate::ui::shown(&entry_path));
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            crate::ui::shown(path)
        )));
    }
    if excluded > 0 {
        crate::ui::message!(
            "kept {}: {} excluded by --exclude",
            crate::ui::shown(path),
            crate::ui::count(excluded as u64, "entry", "entries")
        );
    }
    if excluded > 0 || kept_inside {
        return Ok(true);
    }

    if opts.verbose {
        crate::ui::message!("  removing directory {}", crate::ui::shown(path));
    }
    if !super::streams::names(path)?.is_empty() {
        return Err(io::Error::other(
            "directory acquired named data streams; the directory is kept",
        ));
    }
    unlink_with_random_name(path, true)?;
    keep.done();
    Ok(false)
}

#[cfg(test)]
#[path = "../win_tests.rs"]
mod tests;
