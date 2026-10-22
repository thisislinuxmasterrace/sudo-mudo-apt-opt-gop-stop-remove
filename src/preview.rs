//! what an erase would touch, counted before anything is changed.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::erase;

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
    /// entries that could not be inspected; the erase may fail on them as well.
    pub unreadable: u64,
    /// fifos, sockets and device nodes: the erase refuses them and keeps their directory.
    pub other: u64,
    /// data already counted, so hard links add their size once.
    seen: HashSet<(u64, u64)>,
}

/// counts the selection the way the erase walks it: symlinks are counted as links and never
/// followed, and mount points inside a tree are skipped.
pub fn summarize(paths: &[PathBuf], recursive: bool) -> Summary {
    let mut summary = Summary::default();
    for path in paths {
        add(&mut summary, path, recursive);
    }
    summary
}

fn add(summary: &mut Summary, path: &Path, recursive: bool) {
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
        if recursive {
            add_children(summary, path, &meta);
        }
    } else if file_type.is_file() {
        summary.files += 1;
        if erase::link_count(&meta) > 1 {
            summary.hard_linked += 1;
            if summary.seen.insert(data_id(&meta)) {
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

fn add_children(summary: &mut Summary, dir: &Path, dir_meta: &fs::Metadata) {
    let parent_dev = erase::device(dir_meta);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            summary.unreadable += 1;
            return;
        }
    };

    for entry in entries {
        let child = match entry {
            Ok(entry) => entry.path(),
            Err(_) => {
                summary.unreadable += 1;
                continue;
            }
        };
        match erase::is_mount_boundary(&child, parent_dev) {
            Ok(true) => summary.skipped += 1,
            Ok(false) => add(summary, &child, true),
            Err(_) => summary.unreadable += 1,
        }
    }
}

impl Summary {
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

    #[test]
    fn counts_files_links_and_bytes_in_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("a.txt"), [0u8; 10]).unwrap();
        fs::write(tree.join("sub/b.txt"), [0u8; 20]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&tree, tree.join("link")).unwrap();

        let summary = summarize(std::slice::from_ref(&tree), true);

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

        let summary = summarize(std::slice::from_ref(&tree), false);

        assert_eq!(
            summary,
            Summary {
                dirs: 1,
                ..Summary::default()
            }
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

        let summary = summarize(&[a, b], false);

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

        let summary = summarize(std::slice::from_ref(&a), false);

        assert_eq!(summary.hard_linked, 1);
    }

    #[test]
    fn missing_path_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();

        let summary = summarize(&[dir.path().join("missing")], false);

        assert_eq!(summary.unreadable, 1);
    }

    #[test]
    fn human_bytes_picks_a_unit() {
        assert_eq!(human_bytes(512), "512 b");
        assert_eq!(human_bytes(1536), "1.5 kib");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 mib");
    }
}
