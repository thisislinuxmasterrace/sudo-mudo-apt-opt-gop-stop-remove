//! advisory search for other copies of an object's data in a few local places: a git work tree, editor
//! backup files, a trash folder and a sync folder. nothing is read or changed. a copy that is found is a
//! hint; finding nothing is not proof that no copy exists.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

/// how many ancestors of a path are looked at.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    GitWorkTree,
    SyncFolder,
    Trash,
    EditorBackup,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::GitWorkTree => "git_work_tree",
            Self::SyncFolder => "sync_folder",
            Self::Trash => "trash",
            Self::EditorBackup => "editor_backup",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::GitWorkTree => {
                "the path is in a git work tree, whose history may keep earlier versions"
            }
            Self::SyncFolder => {
                "the path is in a sync folder, and a sync client may keep copies and old versions elsewhere"
            }
            Self::Trash => "the path is inside a trash folder",
            Self::EditorBackup => "an editor backup or swap file with a related name exists",
        }
    }
}

/// a place that may hold another copy: `path` is the git work tree root, the sync or trash folder, or the backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub kind: Kind,
    pub path: PathBuf,
}

/// what the ancestors of one directory say. cached, because the walk visits the entries of a
/// directory one after another and each lookup costs a few stats per ancestor.
#[derive(Debug, Clone, Default)]
struct Ancestry {
    git: Option<PathBuf>,
    sync: Option<PathBuf>,
    trash: Option<PathBuf>,
}

/// how many directories the cache holds.
const CACHE_LIMIT: usize = 256;

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, Ancestry>> = RefCell::new(HashMap::new());
}

/// looks for other copies of a file or directory by its path. links and special objects are not looked at.
pub(crate) fn find(path: &Path, meta: &Metadata) -> Vec<Found> {
    if !meta.is_file() && !meta.is_dir() {
        return Vec::new();
    }
    let dir = if meta.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    let mut found = Vec::new();
    if let Some(dir) = dir {
        let ancestry = ancestry(dir);
        let places = [
            (Kind::GitWorkTree, ancestry.git),
            (Kind::SyncFolder, ancestry.sync),
            (Kind::Trash, ancestry.trash),
        ];
        for (kind, place) in places {
            if let Some(path) = place {
                found.push(Found { kind, path });
            }
        }
    }
    if meta.is_file()
        && let (Some(dir), Some(name)) = (dir, path.file_name())
    {
        for backup in backup_names(name) {
            let candidate = dir.join(backup);
            if fs::symlink_metadata(&candidate).is_ok() {
                found.push(Found {
                    kind: Kind::EditorBackup,
                    path: candidate,
                });
            }
        }
    }
    found
}

fn ancestry(dir: &Path) -> Ancestry {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(ancestry) = cache.get(dir) {
            return ancestry.clone();
        }
        let ancestry = scan(dir);
        // a pre-order walk goes back and forth between a directory and its children, so a few
        // directories are kept; the map is dropped when full to bound its memory.
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(dir.to_path_buf(), ancestry.clone());
        ancestry
    })
}

fn scan(dir: &Path) -> Ancestry {
    let mut ancestry = Ancestry::default();
    // the files inside `.git` are the history itself, not a work tree that holds a copy.
    let mut inside_git_dir = false;
    for ancestor in dir.ancestors().take(MAX_DEPTH) {
        let name = ancestor
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name == ".git" {
            inside_git_dir = true;
        }
        if ancestry.git.is_none() && fs::symlink_metadata(ancestor.join(".git")).is_ok() {
            ancestry.git = Some(ancestor.to_path_buf());
        }
        if ancestry.sync.is_none() && is_sync_root(ancestor, &name) {
            ancestry.sync = Some(ancestor.to_path_buf());
        }
        if ancestry.trash.is_none() && is_trash(ancestor, &name) {
            ancestry.trash = Some(ancestor.to_path_buf());
        }
    }
    if inside_git_dir {
        ancestry.git = None;
    }
    ancestry
}

/// folders that common sync clients create: dropbox, onedrive, google drive, icloud, nextcloud,
/// owncloud, and any folder that holds a syncthing marker.
fn is_sync_root(path: &Path, lower_name: &str) -> bool {
    lower_name == "dropbox"
        || lower_name.starts_with("dropbox (")
        || lower_name == "onedrive"
        || lower_name.starts_with("onedrive - ")
        || lower_name == "google drive"
        || lower_name == "icloud drive"
        || lower_name == "com~apple~clouddocs"
        || lower_name == "nextcloud"
        || lower_name == "owncloud"
        || fs::symlink_metadata(path.join(".stfolder")).is_ok_and(|meta| meta.is_dir())
}

/// the trash folders of freedesktop (`share/Trash`, `.Trash-<uid>`), macos (`.Trash`) and windows (`$Recycle.Bin`).
fn is_trash(path: &Path, lower_name: &str) -> bool {
    if lower_name == ".trash" || lower_name == "$recycle.bin" || lower_name.starts_with(".trash-") {
        return true;
    }
    lower_name == "trash"
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| parent == "share")
}

/// names that vim, emacs and many editors give to a backup, swap or lock file of `name`.
fn backup_names(name: &OsStr) -> Vec<OsString> {
    let wrap = |prefix: &str, suffix: &str| {
        let mut wrapped = OsString::from(prefix);
        wrapped.push(name);
        wrapped.push(suffix);
        wrapped
    };
    vec![
        wrap("", "~"),
        wrap(".", ".swp"),
        wrap(".", ".swo"),
        wrap("", ".bak"),
        wrap("", ".orig"),
        wrap("#", "#"),
        wrap(".#", ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(path: &Path) -> Vec<Kind> {
        let meta = fs::symlink_metadata(path).unwrap();
        find(path, &meta)
            .into_iter()
            .map(|found| found.kind)
            .collect()
    }

    fn file_in(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        fs::write(&file, b"data").unwrap();
        file
    }

    #[test]
    fn git_work_tree_is_found_from_a_nested_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        let file = file_in(&root.path().join("a/b"), "secret");

        let found = find(&file, &fs::symlink_metadata(&file).unwrap());

        assert!(found.contains(&Found {
            kind: Kind::GitWorkTree,
            path: root.path().to_path_buf()
        }));
    }

    #[test]
    fn files_inside_the_git_directory_are_not_reported_as_a_work_tree_copy() {
        let root = tempfile::tempdir().unwrap();
        let file = file_in(&root.path().join(".git/objects"), "pack");

        assert!(!kinds(&file).contains(&Kind::GitWorkTree));
    }

    #[test]
    fn editor_backup_names_are_found_next_to_the_file() {
        let root = tempfile::tempdir().unwrap();
        let file = file_in(root.path(), "notes.txt");
        assert!(!kinds(&file).contains(&Kind::EditorBackup));

        fs::write(root.path().join("notes.txt~"), b"old").unwrap();
        fs::write(root.path().join(".notes.txt.swp"), b"old").unwrap();

        let backups = kinds(&file)
            .into_iter()
            .filter(|kind| *kind == Kind::EditorBackup)
            .count();
        assert_eq!(backups, 2);
    }

    #[test]
    fn sync_and_trash_folders_are_found_by_name() {
        let root = tempfile::tempdir().unwrap();
        let synced = file_in(&root.path().join("Dropbox/docs"), "a");
        let trashed = file_in(&root.path().join("share/Trash/files"), "a");
        let other_trash = file_in(&root.path().join(".Trash-1000/files"), "a");
        let plain_trash = file_in(&root.path().join("Trash"), "a");

        assert!(kinds(&synced).contains(&Kind::SyncFolder));
        assert!(kinds(&trashed).contains(&Kind::Trash));
        assert!(kinds(&other_trash).contains(&Kind::Trash));
        assert!(!kinds(&plain_trash).contains(&Kind::Trash));
    }

    #[test]
    fn syncthing_marker_makes_a_sync_folder() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("share/.stfolder")).unwrap();
        let file = file_in(&root.path().join("share/sub"), "a");

        assert!(kinds(&file).contains(&Kind::SyncFolder));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_looked_at() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink("missing", &link).unwrap();

        assert!(find(&link, &fs::symlink_metadata(&link).unwrap()).is_empty());
    }
}
