//! what an erase would touch, counted before anything is changed.

use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::erase;
use crate::json;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
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
    excludes: &'a [String],
    /// data already counted, so hard links add their size once, also across paths.
    seen: HashSet<(u64, u64)>,
}

/// counts each path the way the erase walks it, one summary per path in the same order. symlinks are
/// counted as links and never followed, mount points inside a tree are skipped, entries matching
/// `excludes` are not walked, and a target that is a mount point is refused.
/// the paths must be normalized and validated by the caller (`main::normalize`, `safety::validate_target`).
pub fn summarize(paths: &[PathBuf], recursive: bool, excludes: &[String]) -> Vec<Summary> {
    let mut walk = Walk {
        recursive,
        excludes,
        seen: HashSet::new(),
    };
    paths
        .iter()
        .map(|path| {
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
    erase::is_mount_boundary(path, parent_dev)
}

/// windows cannot open a directory as a file, and its erase never refuses a target as a mount
/// point, so there is nothing to check here.
#[cfg(not(unix))]
pub(crate) fn is_target_boundary(_path: &Path) -> io::Result<bool> {
    Ok(false)
}

/// `rel` is the path of `path` from its command-line argument, empty for the argument itself.
fn add(walk: &mut Walk<'_>, summary: &mut Summary, path: &Path, rel: &Path) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => {
            summary.unreadable += 1;
            return;
        }
    };

    let file_type = meta.file_type();
    if file_type.is_symlink() {
        summary.links += 1;
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
        match erase::is_mount_boundary(&child, parent_dev) {
            Ok(true) => summary.skipped += 1,
            Ok(false) => add(walk, summary, &child, &child_rel),
            Err(_) => summary.unreadable += 1,
        }
    }
    summary.excluded > excluded_before
}

impl Summary {
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
                "special files (fifo, sockets, devices), which are refused: {}",
                self.other
            ));
        }
        lines
    }
}

/// the dry-run summary as one line of json, for `--dry-run --json`. `paths` are the normalized paths
/// the run would erase, and `total` is their sum.
pub fn to_json(paths: &[PathBuf], passes: u32, total: &Summary) -> String {
    let mut out = String::from("{\"paths\":[");
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        json::push_path(&mut out, path);
    }
    out.push_str("],\"passes\":");
    json::push_u64(&mut out, u64::from(passes));
    let counts = [
        ("files", total.files),
        ("dirs", total.dirs),
        ("links", total.links),
        ("bytes", total.bytes),
        ("hard_linked", total.hard_linked),
        ("skipped", total.skipped),
        ("refused", total.refused),
        ("unreadable", total.unreadable),
        ("other", total.other),
        ("excluded", total.excluded),
        ("kept_dirs", total.kept_dirs),
        ("objects", total.objects()),
    ];
    for (key, value) in counts {
        out.push_str(",\"");
        out.push_str(key);
        out.push_str("\":");
        json::push_u64(&mut out, value);
    }
    out.push('}');
    out
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
mod tests {
    use super::*;

    fn count(paths: &[PathBuf], recursive: bool, excludes: &[String]) -> Summary {
        total(&summarize(paths, recursive, excludes))
    }

    #[test]
    fn counts_files_links_and_bytes_in_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();
        fs::write(tree.join("sub/b.txt"), [0u8; 20]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&tree, tree.join("link")).unwrap();

        let summary = count(std::slice::from_ref(&tree), true, &[]);

        assert_eq!(summary.files, 2);
        assert_eq!(summary.dirs, 2);
        assert_eq!(summary.bytes, 30);
        assert_eq!(summary.unreadable, 0);
        #[cfg(unix)]
        assert_eq!(summary.links, 1);
    }

    #[test]
    fn without_recursive_only_the_top_level_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();

        let summary = count(std::slice::from_ref(&tree), false, &[]);

        assert_eq!(
            summary,
            Summary {
                dirs: 1,
                ..Summary::default()
            }
        );
    }

    #[test]
    fn excluded_entries_are_counted_and_not_walked() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("skip")).unwrap();
        fs::write(tree.join("a.log"), [0u8; 10]).unwrap();
        fs::write(tree.join("skip/b.txt"), [0u8; 20]).unwrap();
        fs::write(tree.join("c.txt"), [0u8; 30]).unwrap();
        let excludes = ["*.log".to_owned(), "skip".to_owned()];

        let summary = count(std::slice::from_ref(&tree), true, &excludes);

        assert_eq!(summary.excluded, 2);
        assert_eq!(summary.kept_dirs, 1);
        assert_eq!(summary.files, 1);
        assert_eq!(summary.dirs, 1);
        assert_eq!(summary.bytes, 30);
        let lines = summary.lines();
        assert!(lines.iter().any(|line| line == "excluded by --exclude: 2"));
        assert!(
            lines
                .iter()
                .any(|line| line == "directories kept because of --exclude: 1")
        );
    }

    #[test]
    fn command_line_argument_is_not_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();

        let summary = count(std::slice::from_ref(&tree), true, &["tree".to_owned()]);

        assert_eq!(summary.excluded, 0);
        assert_eq!(summary.dirs, 1);
        assert_eq!(summary.files, 1);
    }

    #[test]
    fn without_exclusions_no_line_is_printed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, [0u8; 10]).unwrap();

        let summary = count(std::slice::from_ref(&file), false, &[]);

        assert!(
            summary
                .lines()
                .iter()
                .all(|line| !line.contains("excluded"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_add_their_size_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, [0u8; 100]).unwrap();
        fs::hard_link(&a, &b).unwrap();

        let summary = count(&[a, b], false, &[]);

        assert_eq!(summary.hard_linked, 2);
        assert_eq!(summary.bytes, 100);
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"data").unwrap();
        fs::hard_link(&a, &b).unwrap();

        let summary = count(std::slice::from_ref(&a), false, &[]);

        assert_eq!(summary.hard_linked, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_point_given_as_target_is_refused() {
        let summary = count(&[PathBuf::from("/proc")], true, &[]);

        assert_eq!(summary.refused, 1);
        assert_eq!(summary.dirs, 0);
        assert_eq!(summary.files, 0);
        assert!(
            summary
                .lines()
                .iter()
                .any(|line| line.starts_with("refused"))
        );
    }

    #[test]
    fn missing_path_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();

        let summary = count(&[dir.path().join("missing")], false, &[]);

        assert_eq!(summary.unreadable, 1);
    }

    #[test]
    fn json_summary_is_one_object_with_every_count() {
        let total = Summary {
            files: 2,
            dirs: 1,
            bytes: 30,
            ..Summary::default()
        };

        let json = to_json(&[PathBuf::from("a\"b")], 1, &total);

        assert_eq!(
            json,
            r#"{"paths":["a\"b"],"passes":1,"files":2,"dirs":1,"links":0,"bytes":30,"hard_linked":0,"skipped":0,"refused":0,"unreadable":0,"other":0,"excluded":0,"kept_dirs":0,"objects":3}"#
        );
    }

    #[test]
    fn human_bytes_picks_a_unit() {
        assert_eq!(human_bytes(512), "512 b");
        assert_eq!(human_bytes(1536), "1.5 kib");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 mib");
    }
}
