//! advice printed by `--explain`: what an overwrite can and cannot do for the paths, taken from the
//! assessment facts. commands are only printed. the text never says that data is unrecoverable.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::copies::{self, git};
use crate::preview::EntryAssessment;
use crate::storage::{self, Compression, Encryption, Kind};

/// the command-line targets of one run, as the erase will walk them.
pub(crate) struct Targets<'a> {
    pub paths: &'a [PathBuf],
    pub recursive: bool,
    pub excludes: &'a [String],
}

const GENERAL: &str = "  - an overwrite is best effort: it replaces what the filesystem keeps for this name, not copies made elsewhere. a disk that held secrets is best protected by full-disk encryption whose key is destroyed. nothing printed here proves that data is unrecoverable.";

const REWRITE_HISTORY: &str = "to remove them, rewrite history in a fresh clone (for example `git filter-repo --path <file> --invert-paths`), then run `git reflog expire --expire=now --all` and `git gc --prune=now`. clones and remotes keep their own copies.";

// a long list of backups would bury the rest of the advice; json lists every one.
const LISTED_BACKUPS: usize = 5;

/// the whole advice block, as lines ready to print: what was found for these paths first, the general
/// limits last.
pub(crate) fn lines(targets: &Targets<'_>, entries: &[EntryAssessment]) -> Vec<String> {
    let mut lines = storage_advice(entries);
    lines.extend(copy_advice(targets, entries));
    for path in targets.paths {
        lines.extend(crate::snapshots::explain(path));
    }
    lines.push(GENERAL.to_owned());
    lines
}

/// advice about the storage of the visited entries. every condition is listed once, however many entries
/// share it.
fn storage_advice(entries: &[EntryAssessment]) -> Vec<String> {
    let mut flash = false;
    let mut unknown_medium = false;
    let mut copy_on_write = false;
    let mut network = false;
    let mut fuse_or_unknown = false;
    let mut link = false;
    let mut dm_crypt = false;
    let mut ecryptfs = false;
    let mut fscrypt = false;
    let mut compressed = false;
    for entry in entries {
        let assessment = &entry.assessment;
        link |= assessment.verdict() == "cosmetic";
        compressed |= assessment.compression() == Compression::Enabled;
        match assessment.encryption() {
            Encryption::DmCrypt => dm_crypt = true,
            Encryption::Ecryptfs => ecryptfs = true,
            Encryption::Fscrypt => fscrypt = true,
            Encryption::FscryptDmCrypt => {
                fscrypt = true;
                dm_crypt = true;
            }
            Encryption::NotChecked | Encryption::NoneDetected => {}
        }
        match assessment.medium() {
            "flash" => flash = true,
            // an object with no filesystem fact is special or unreadable: the medium advice would not help.
            "unknown" if assessment.filesystem().is_some() => unknown_medium = true,
            _ => {}
        }
        match assessment.filesystem().map(storage::classify) {
            Some(Kind::CopyOnWrite) => copy_on_write = true,
            Some(Kind::Network) => network = true,
            Some(Kind::FuseOrUnknown) => fuse_or_unknown = true,
            Some(Kind::Local) | None => {}
        }
    }

    let mut lines = Vec::new();
    if link {
        lines.push("  - symbolic link: erasing it removes only the link. erase the target itself to overwrite its data.".to_owned());
    }
    if flash {
        lines.push("  - flash storage (ssd, usb stick, sd card): the drive may keep old blocks that an overwrite never reaches. only the drive's own erase of the whole disk, or destroying the key of encryption that was on before the data was written, reaches them. the documentation explains this under \"before selling or giving away a device\".".to_owned());
    }
    if dm_crypt {
        lines.push("  - dm-crypt volume: what is unreadable without the key is the whole volume, not one file. for a luks volume `cryptsetup luksErase /dev/<device>` removes every key slot, which makes the volume unreadable unless a header or key backup exists, so check `<device>` first. other dm-crypt types have no such command. an overwrite of one file only reaches the encrypted blocks.".to_owned());
    }
    if ecryptfs {
        lines.push("  - ecryptfs mount: the files below it are encrypted. destroying the key is what makes them unreadable. an overwrite only reaches the encrypted lower files.".to_owned());
    }
    if unknown_medium {
        lines.push("  - storage medium unknown: check it with `lsblk -d -o NAME,ROTA`. 1 means a spinning disk, 0 means flash or a virtual disk.".to_owned());
    }
    if fscrypt {
        lines.push("  - fscrypt: the filesystem encrypts this object. removing one name does not destroy its encryption key; other files and key backups may still allow access. an overwrite remains best effort.".to_owned());
    }
    if copy_on_write {
        lines.push("  - copy-on-write filesystem: a write goes to new blocks and the old ones may stay until they are reused. remove snapshots first (`btrfs subvolume list <mount>`, `zfs list -t snapshot`, `tmutil listlocalsnapshots /`), and consider erasing the whole device or destroying an encryption key.".to_owned());
    }
    if compressed {
        lines.push("  - transparent compression is on for this object or mount: new data may occupy smaller extents than old data, so overwriting need not reach the old blocks. on windows directories, the flag is a default for new files. random data does not prove an in-place rewrite.".to_owned());
    }
    if network {
        lines.push("  - network filesystem: the server stores the data and may keep copies and snapshots. erase on the server, or ask its administrator.".to_owned());
    }
    if fuse_or_unknown {
        lines.push("  - fuse or unrecognized filesystem: writes may go to a remote service. find out what is behind it before relying on an overwrite.".to_owned());
    }
    lines
}

/// other copies found near the entries, named by their paths. a backup that this run erases is left out.
fn copy_advice(targets: &Targets<'_>, entries: &[EntryAssessment]) -> Vec<String> {
    let erased: HashSet<&Path> = entries.iter().map(|entry| entry.path.as_path()).collect();
    let mut backups = BTreeSet::new();
    let mut sync = BTreeSet::new();
    let mut trash = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for found in entries.iter().flat_map(|entry| entry.assessment.copies()) {
        match found.kind {
            copies::Kind::EditorBackup => {
                if !erased.contains(found.path.as_path()) {
                    backups.insert(found.path.clone());
                }
            }
            copies::Kind::SyncFolder => {
                sync.insert(found.path.clone());
            }
            copies::Kind::Trash => {
                trash.insert(found.path.clone());
            }
            copies::Kind::GitWorkTree => {
                roots.extend(absolute(&found.path));
            }
        }
    }

    let mut lines = Vec::new();
    for path in backups.iter().take(LISTED_BACKUPS) {
        lines.push(format!(
            "  - editor backup: {} may hold older content. erase it too.",
            path.display()
        ));
    }
    if backups.len() > LISTED_BACKUPS {
        lines.push(format!(
            "  - editor backups: {} more; `--dry-run --json` lists them under `copies`.",
            backups.len() - LISTED_BACKUPS
        ));
    }
    lines.extend(git_advice(targets, roots));
    for path in sync {
        lines.push(format!(
            "  - sync folder: {}: the sync service and other devices keep copies and old versions. delete the files in the service too, including its trash and version history.",
            path.display()
        ));
    }
    for path in trash {
        lines.push(format!(
            "  - trash folder: {}: other trash folders (other drives, other users) may hold more copies.",
            path.display()
        ));
    }
    lines
}

/// one line per repository: either this run erases its history too, or the history stays and the line
/// says what git knows about each target in it.
fn git_advice(targets: &Targets<'_>, mut roots: BTreeSet<PathBuf>) -> Vec<String> {
    let details: Vec<(&PathBuf, git::Detail)> = targets
        .paths
        .iter()
        .filter_map(|path| git::inspect(path).map(|detail| (path, detail)))
        .collect();
    roots.extend(details.iter().map(|(_, detail)| detail.root.clone()));
    let trees: Vec<PathBuf> = if targets.recursive {
        targets
            .paths
            .iter()
            .filter_map(|path| absolute(path))
            .collect()
    } else {
        Vec::new()
    };

    let mut lines = Vec::new();
    for root in &roots {
        if history_is_erased(root, &trees, targets.excludes) {
            // a pattern can match inside `.git` (`*.pack`, `objects`) without matching `.git` itself.
            let except = if targets.excludes.is_empty() {
                ""
            } else {
                ", except entries that --exclude keeps"
            };
            lines.push(format!(
                "  - git: the repository at {} is erased with its history (`.git`){except}. clones and remotes keep their own copies.",
                shown(root).display()
            ));
            continue;
        }
        let mut described = false;
        for (path, detail) in details.iter().filter(|(_, detail)| &detail.root == root) {
            lines.push(target_in_repository(path, detail));
            described = true;
        }
        if !described {
            lines.push(format!(
                "  - git: the history of {} may keep earlier versions of files erased here; this erase does not touch it. {REWRITE_HISTORY}",
                shown(root).display()
            ));
        }
    }
    lines
}

fn target_in_repository(path: &Path, detail: &git::Detail) -> String {
    let root = shown(&detail.root);
    let (path, root) = (path.display(), root.display());
    match (detail.tracked, detail.in_history) {
        (_, Some(true)) => format!(
            "  - git: the history of {root} holds earlier versions of {path}; this erase does not touch it. {REWRITE_HISTORY}"
        ),
        (Some(true), Some(false)) => format!(
            "  - git: the index of {root} holds a staged copy of {path}. run `git rm --cached -- <file>` there, then `git gc --prune=now`."
        ),
        (Some(false), Some(false)) => format!(
            "  - git: {path} is not in the index or the reachable history of {root}. renames, reflogs and unreachable objects are not checked."
        ),
        _ => format!(
            "  - git: git could not tell whether the history of {root} holds earlier versions of {path}; this erase does not touch it. {REWRITE_HISTORY}"
        ),
    }
}

/// true when a recursive target walks into `root/.git` and nothing on the way is excluded. a `.git` file
/// points at history stored elsewhere (worktrees, submodules), so that history is never erased here.
/// paths are compared resolved, so a symlinked current directory does not hide the relation.
fn history_is_erased(root: &Path, trees: &[PathBuf], excludes: &[String]) -> bool {
    if !std::fs::symlink_metadata(root.join(".git")).is_ok_and(|meta| meta.is_dir()) {
        return false;
    }
    let resolved = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = resolved(root);
    trees.iter().any(|tree| {
        let Ok(inside) = root.strip_prefix(resolved(tree)) else {
            return false;
        };
        let mut rel = PathBuf::new();
        inside.join(".git").components().all(|part| {
            rel.push(part);
            !crate::erase::is_excluded(excludes, &rel)
        })
    })
}

/// a repository root relative to the current directory when it lies inside it, like the targets.
fn shown(root: &Path) -> PathBuf {
    let Ok(current) = std::env::current_dir() else {
        return root.to_path_buf();
    };
    match root.strip_prefix(&current) {
        Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
        Ok(rel) => rel.to_path_buf(),
        Err(_) => root.to_path_buf(),
    }
}

/// copies are found from paths as given, so a file in the current directory has an empty parent.
fn absolute(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        std::env::current_dir().ok()
    } else {
        std::path::absolute(path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::Assessment;
    use std::path::PathBuf;

    fn entry(filesystem: &str, medium: &'static str) -> EntryAssessment {
        EntryAssessment {
            path: PathBuf::from("x"),
            assessment: Assessment::from_facts(
                Some(filesystem.to_owned()),
                medium,
                Compression::Unknown,
                Encryption::NoneDetected,
                false,
                true,
                Some(Vec::new()),
            ),
        }
    }

    #[test]
    fn enabled_compression_gets_its_own_advice() {
        let entries = [EntryAssessment {
            path: PathBuf::from("x"),
            assessment: Assessment::from_facts(
                Some("btrfs".to_owned()),
                "flash",
                Compression::Enabled,
                Encryption::NoneDetected,
                false,
                true,
                Some(Vec::new()),
            ),
        }];
        assert!(text(&entries).contains("transparent compression is on"));
        assert!(!text(&[entry("ext4", "rotational")]).contains("transparent compression"));
    }

    #[test]
    fn layered_encryption_keeps_both_explanations() {
        let entries = [EntryAssessment {
            path: PathBuf::from("x"),
            assessment: Assessment::from_facts(
                Some("ext4".to_owned()),
                "flash",
                Compression::Unknown,
                Encryption::FscryptDmCrypt,
                false,
                true,
                Some(Vec::new()),
            ),
        }];
        let text = text(&entries);
        assert!(text.contains("fscrypt:"));
        assert!(text.contains("dm-crypt volume:"));
    }

    #[test]
    fn encrypted_volumes_get_key_advice() {
        let entries = [EntryAssessment {
            path: PathBuf::from("x"),
            assessment: Assessment::from_facts(
                Some("ext4".to_owned()),
                "flash",
                Compression::Unknown,
                Encryption::DmCrypt,
                false,
                true,
                Some(Vec::new()),
            ),
        }];
        let text = text(&entries);
        assert!(text.contains("dm-crypt volume"));
        assert!(text.contains("cryptsetup luksErase"));
        assert!(!text.contains("ecryptfs mount"));
    }

    fn text(entries: &[EntryAssessment]) -> String {
        storage_advice(entries).join("\n")
    }

    #[test]
    fn flash_and_copy_on_write_get_their_own_advice_once() {
        let entries = [entry("btrfs", "flash"), entry("btrfs", "flash")];
        let text = text(&entries);
        assert_eq!(text.matches("flash storage").count(), 1);
        assert_eq!(text.matches("copy-on-write filesystem").count(), 1);
        assert!(text.contains("drive's own erase of the whole disk"));
        assert!(!text.contains("network filesystem"));
    }

    #[test]
    fn plain_local_rotational_target_gets_only_the_general_line() {
        assert!(storage_advice(&[entry("ext4", "rotational")]).is_empty());
        let targets = Targets {
            paths: &[],
            recursive: false,
            excludes: &[],
        };
        assert_eq!(
            lines(&targets, &[entry("ext4", "rotational")]),
            [GENERAL.to_owned()]
        );
        assert!(GENERAL.contains("nothing printed here proves"));
    }

    #[test]
    fn history_is_erased_only_when_the_tree_walks_into_a_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let tree = vec![dir.path().to_path_buf()];
        assert!(history_is_erased(&root, &tree, &[]));
        assert!(history_is_erased(&root, std::slice::from_ref(&root), &[]));
        assert!(!history_is_erased(&root, &tree, &[".git".to_owned()]));
        assert!(!history_is_erased(&root, &tree, &["repo".to_owned()]));
        assert!(!history_is_erased(&root, &[root.join("sub")], &[]));
        assert!(!history_is_erased(&root, &[], &[]));
        let worktree = dir.path().join("worktree");
        std::fs::create_dir(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), b"gitdir: elsewhere").unwrap();
        assert!(!history_is_erased(&worktree, &tree, &[]));
    }

    #[test]
    fn found_backups_are_named_unless_this_run_erases_them() {
        let backup = |path: &str| EntryAssessment {
            path: PathBuf::from("dir/file"),
            assessment: Assessment::from_facts(
                Some("ext4".to_owned()),
                "rotational",
                Compression::Unknown,
                Encryption::NoneDetected,
                false,
                true,
                Some(vec![copies::Found {
                    kind: copies::Kind::EditorBackup,
                    path: PathBuf::from(path),
                }]),
            ),
        };
        let targets = Targets {
            paths: &[],
            recursive: false,
            excludes: &[],
        };
        let text = copy_advice(&targets, &[backup("dir/file~")]).join("\n");
        assert!(text.contains("editor backup: dir/file~ may hold"), "{text}");
        let erased = EntryAssessment {
            path: PathBuf::from("dir/file~"),
            assessment: Assessment::unknown(),
        };
        assert!(copy_advice(&targets, &[backup("dir/file~"), erased]).is_empty());
    }

    #[test]
    fn network_and_unrecognized_filesystems_are_told_apart() {
        assert!(text(&[entry("nfs", "unknown")]).contains("network filesystem"));
        assert!(text(&[entry("fuse.sshfs", "unknown")]).contains("fuse or unrecognized"));
        assert!(text(&[entry("nfs", "unknown")]).contains("storage medium unknown"));
    }

    #[test]
    fn the_text_never_promises_unrecoverable_data() {
        let text = text(&[entry("ext4", "flash")]).to_lowercase();
        assert!(!text.contains("securely erased"));
        assert!(!text.contains("guarantee"));
    }

    #[test]
    fn flash_advice_for_a_file_prints_no_whole_disk_commands() {
        let text = text(&[entry("ext4", "flash")]);
        for command in ["nvme", "hdparm", "luksErase"] {
            assert!(!text.contains(command), "{command}: {text}");
        }
        assert!(!text.contains("permanently unreadable"), "{text}");
    }
}
