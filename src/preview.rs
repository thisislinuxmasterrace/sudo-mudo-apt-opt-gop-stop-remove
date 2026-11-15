//! what an erase would touch, counted before anything is changed.

use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::assessment::Assessment;
use crate::erase::{self, MountTable};

mod json_output;
pub use json_output::to_json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryAssessment {
    pub path: PathBuf,
    pub assessment: Assessment,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// each visited entry has its own facts; a directory never supplies facts for its children.
    pub assessments: Vec<EntryAssessment>,
    /// the root hard-link rule can refuse files otherwise counted by the preview.
    pub coverage_limited: bool,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
    /// files that also have hard links; the other names lose their content too.
    pub hard_linked: u64,
    /// mount points and other filesystems inside a tree, skipped by the erase.
    pub skipped: u64,
    /// entries matched by `--exclude`. they are not walked, so their contents are not counted.
    pub excluded: u64,
    /// directories that stay because an entry inside them (at any depth) is excluded.
    pub kept_dirs: u64,
    /// targets that are mount points or on another filesystem; the erase refuses them outright.
    pub refused: u64,
    /// entries that could not be inspected; the erase may fail on them as well.
    pub unreadable: u64,
    /// fifos, sockets and device nodes: the erase refuses them and keeps their directory.
    pub other: u64,
}

/// state shared by the walks of one `summarize` call.
struct Walk<'a> {
    recursive: bool,
    refuse_hard_links: bool,
    excludes: &'a [String],
    /// data already counted, so hard links add their size once, also across paths.
    seen: HashSet<(u64, u64)>,
    /// read again for each path, as the erase does.
    mounts: MountTable,
}

/// counts each path the way the erase walks it, one summary per path in the same order. symlinks are
/// counted as links and never followed, mount points inside a tree are skipped, entries matching
/// `excludes` are not walked, and a target that is a mount point is refused.
/// the paths must be normalized and validated by the caller (`main::normalize`, `safety::validate_target`).
pub fn summarize(
    paths: &[PathBuf],
    recursive: bool,
    excludes: &[String],
    allow_protected: bool,
) -> Vec<Summary> {
    let mut walk = Walk {
        recursive,
        refuse_hard_links: erase::running_as_root() && !allow_protected,
        excludes,
        seen: HashSet::new(),
        mounts: MountTable::read(),
    };
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            if index > 0 {
                walk.mounts = MountTable::read();
            }
            let mut summary = Summary::default();
            match is_target_boundary(path) {
                Ok(true) => summary.refused += 1,
                Ok(false) => add(&mut walk, &mut summary, path, Path::new("")),
                Err(_) => summary.unreadable += 1,
            }
            summary
        })
        .collect()
}

/// adds up the summaries of several paths.
pub fn total(parts: &[Summary]) -> Summary {
    let mut total = Summary::default();
    for part in parts {
        total.coverage_limited |= part.coverage_limited;
        total.assessments.extend(part.assessments.iter().cloned());
        total.files += part.files;
        total.dirs += part.dirs;
        total.links += part.links;
        total.bytes += part.bytes;
        total.hard_linked += part.hard_linked;
        total.skipped += part.skipped;
        total.excluded += part.excluded;
        total.kept_dirs += part.kept_dirs;
        total.refused += part.refused;
        total.unreadable += part.unreadable;
        total.other += part.other;
    }
    total
}

/// the erase checks a target against its parent before touching anything inside it, so the
/// same check is made here. the parent is followed like the erase follows it.
#[cfg(unix)]
pub(crate) fn is_target_boundary(path: &Path) -> io::Result<bool> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let parent_dev = erase::device(&File::open(parent)?.metadata()?);
    erase::is_mount_boundary_in(path, parent_dev, &MountTable::read())
}

/// windows cannot open a directory as a file, and its erase never refuses a target as a mount
/// point, so there is nothing to check here.
#[cfg(not(unix))]
pub(crate) fn is_target_boundary(_path: &Path) -> io::Result<bool> {
    Ok(false)
}

/// `rel` is the path of `path` from its command-line argument, empty for the argument itself.
fn add(walk: &mut Walk<'_>, summary: &mut Summary, path: &Path, rel: &Path) {
    let (meta, mut assessment) = match Assessment::capture(path) {
        Ok(result) => result,
        Err(_) => {
            summary.unreadable += 1;
            return;
        }
    };

    if meta.is_file() && erase::link_count(&meta) > 1 && walk.refuse_hard_links {
        summary.coverage_limited = true;
        assessment = assessment.unattempted();
    }
    summary.assessments.push(EntryAssessment {
        path: path.to_path_buf(),
        assessment,
    });
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        summary.links += 1;
    } else if crate::entry::is_unsupported_reparse(&meta) {
        summary.other += 1;
    } else if file_type.is_dir() {
        summary.dirs += 1;
        if walk.recursive && add_children(walk, summary, path, &meta, rel) {
            summary.kept_dirs += 1;
        }
    } else if file_type.is_file() {
        summary.files += 1;
        if erase::link_count(&meta) > 1 {
            summary.hard_linked += 1;
            if walk.seen.insert(data_id(&meta)) {
                summary.bytes += meta.len();
            }
        } else {
            summary.bytes += meta.len();
        }
    } else {
        summary.other += 1;
    }
}

/// identity of the data behind a file.
#[cfg(unix)]
fn data_id(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;

    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn data_id(_meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// counts the entries of `dir`. returns true when something inside was excluded, at any depth, which
/// means the erase keeps `dir`.
fn add_children(
    walk: &mut Walk<'_>,
    summary: &mut Summary,
    dir: &Path,
    dir_meta: &fs::Metadata,
    rel: &Path,
) -> bool {
    let excluded_before = summary.excluded;
    let parent_dev = erase::device(dir_meta);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            summary.unreadable += 1;
            return false;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.unreadable += 1;
                continue;
            }
        };
        let child = entry.path();
        let child_rel = rel.join(entry.file_name());
        if erase::is_excluded(walk.excludes, &child_rel) {
            summary.excluded += 1;
            continue;
        }
        match erase::is_mount_boundary_in(&child, parent_dev, &walk.mounts) {
            Ok(true) => summary.skipped += 1,
            Ok(false) => add(walk, summary, &child, &child_rel),
            Err(_) => summary.unreadable += 1,
        }
    }
    summary.excluded > excluded_before
}

impl Summary {
    pub fn incomplete(&self) -> bool {
        self.coverage_limited
            || self.skipped + self.excluded + self.refused + self.unreadable + self.other > 0
    }

    /// entries the erase would remove: files, directories and links. an upper estimate for the strict
    /// confirmation: directories kept because of `--exclude` are counted too.
    pub fn objects(&self) -> u64 {
        self.files + self.dirs + self.links
    }

    /// one line per kind of thing that will be touched, for the dry run and the prompt.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "files: {}, directories: {}, links: {}, size: {}",
            self.files,
            self.dirs,
            self.links,
            human_bytes(self.bytes)
        )];
        if self.hard_linked > 0 {
            lines.push(format!(
                "hard-linked files: {} (the other names lose their content too)",
                self.hard_linked
            ));
        }
        if self.skipped > 0 {
            lines.push(format!(
                "skipped mount points or other filesystems: {}",
                self.skipped
            ));
        }
        if self.excluded > 0 {
            lines.push(format!("excluded by --exclude: {}", self.excluded));
        }
        if self.kept_dirs > 0 {
            lines.push(format!(
                "directories kept because of --exclude: {}",
                self.kept_dirs
            ));
        }
        if self.refused > 0 {
            lines.push(format!(
                "refused: mount point given as target: {}",
                self.refused
            ));
        }
        if self.unreadable > 0 {
            lines.push(format!("unreadable entries: {}", self.unreadable));
        }
        if self.other > 0 {
            lines.push(format!(
                "special entries (fifo, sockets, devices, unsupported reparse points), which are refused: {}",
                self.other
            ));
        }
        if !self.assessments.is_empty() {
            let mut counts = std::collections::BTreeMap::new();
            for entry in &self.assessments {
                *counts.entry(entry.assessment.verdict()).or_insert(0usize) += 1;
            }
            let counts: Vec<_> = counts
                .iter()
                .map(|(verdict, count)| format!("{count} {verdict}"))
                .collect();
            lines.push(format!(
                "assessments: {} (use --explain for risks or --dry-run --json for details)",
                counts.join(", ")
            ));
        }
        if self.incomplete() {
            lines.push("assessment coverage: incomplete (skipped, excluded, refused, special or unreadable entries are not fully assessed for overwrite)".to_owned());
        }
        lines
    }
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["b", "kib", "mib", "gib", "tib"];

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} b")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests;
