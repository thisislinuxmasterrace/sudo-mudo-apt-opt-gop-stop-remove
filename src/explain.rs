//! advice printed by `--explain`: what an overwrite can and cannot do for the paths, taken from the
//! assessment facts. commands are only printed. the text never says that data is unrecoverable.

use std::collections::BTreeSet;

use crate::copies;
use crate::preview::EntryAssessment;
use crate::storage::{self, Compression, Encryption, Kind};

/// one block of advice for the visited entries, as lines ready to print. every condition is listed once,
/// however many entries share it.
pub(crate) fn advice(entries: &[EntryAssessment]) -> Vec<String> {
    let mut flash = false;
    let mut unknown_medium = false;
    let mut copy_on_write = false;
    let mut network = false;
    let mut fuse_or_unknown = false;
    let mut link = false;
    let mut dm_crypt = false;
    let mut ecryptfs = false;
    let mut compressed = false;
    let mut copy_kinds = BTreeSet::new();
    for entry in entries {
        let assessment = &entry.assessment;
        link |= assessment.verdict() == "cosmetic";
        compressed |= assessment.compression() == Compression::Enabled;
        match assessment.encryption() {
            Encryption::DmCrypt => dm_crypt = true,
            Encryption::Ecryptfs => ecryptfs = true,
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
        copy_kinds.extend(assessment.copy_kinds().map(copies::Kind::as_str));
    }

    let mut lines = vec![
        "explain: an overwrite is best effort. it replaces what the filesystem keeps for this name, not copies made elsewhere.".to_owned(),
    ];
    if link {
        lines.push("  - symbolic link: erasing it removes only the link. erase the target itself to overwrite its data.".to_owned());
    }
    if flash {
        lines.push("  - flash storage (ssd, usb stick, sd card): the drive may keep old blocks that an overwrite never reaches. the drive's own erase works on the whole disk and destroys everything on it, the system disk included, so check `<disk>` first. nvme: `nvme sanitize /dev/<disk> --sanact=2` or `nvme format /dev/<disk> --ses=1`. sata ssd: `hdparm --user-master u --security-set-pass <password> /dev/<disk>` and then `hdparm --user-master u --security-erase <password> /dev/<disk>`; an interrupted run or a frozen drive can leave it password-locked. data kept on an encrypted volume is better handled by destroying its key: `cryptsetup luksErase /dev/<device>` removes every key slot and makes the volume permanently unreadable. usb sticks and sd cards have no such commands.".to_owned());
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
    if copy_on_write {
        lines.push("  - copy-on-write filesystem: a write goes to new blocks and the old ones may stay until they are reused. remove snapshots first (`btrfs subvolume list <mount>`, `zfs list -t snapshot`, `tmutil listlocalsnapshots /`), and consider erasing the whole device or destroying an encryption key.".to_owned());
    }
    if compressed {
        lines.push("  - transparent compression is on for this mount: new data may be stored in smaller extents than the old data, so an overwrite need not reach the old blocks. random data does not compress, but that does not make the filesystem rewrite in place. treat it like a copy-on-write filesystem.".to_owned());
    }
    if network {
        lines.push("  - network filesystem: the server stores the data and may keep copies and snapshots. erase on the server, or ask its administrator.".to_owned());
    }
    if fuse_or_unknown {
        lines.push("  - fuse or unrecognized filesystem: writes may go to a remote service. find out what is behind it before relying on an overwrite.".to_owned());
    }
    for kind in copy_kinds {
        lines.push(copy_advice(kind).to_owned());
    }
    lines.push("  - a disk that held secrets is best protected by full-disk encryption whose key is destroyed. nothing printed here proves that data is unrecoverable.".to_owned());
    lines
}

fn copy_advice(kind: &str) -> &'static str {
    match kind {
        "git_work_tree" => {
            "  - git work tree: history keeps earlier versions of tracked files, and this tool does not touch `.git`. rewrite history in a fresh clone (for example `git filter-repo --path <file> --invert-paths`), then run `git reflog expire --expire=now --all` and `git gc --prune=now`. clones and remotes keep their own copies."
        }
        "sync_folder" => {
            "  - sync folder: the sync service and other devices keep copies and old versions. delete the file in the service too, including its trash and version history."
        }
        "trash" => {
            "  - trash folder: this path is already in a trash. other trash folders (other drives, other users) may hold more copies."
        }
        _ => {
            "  - editor backups: backup and swap files next to a file hold old content. erase them too; `--dry-run --json` lists them under `copies`."
        }
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
        advice(entries).join("\n")
    }

    #[test]
    fn flash_and_copy_on_write_get_their_own_advice_once() {
        let entries = [entry("btrfs", "flash"), entry("btrfs", "flash")];
        let text = text(&entries);
        assert_eq!(text.matches("flash storage").count(), 1);
        assert_eq!(text.matches("copy-on-write filesystem").count(), 1);
        assert!(text.contains("nvme sanitize"));
        assert!(!text.contains("network filesystem"));
    }

    #[test]
    fn plain_local_rotational_target_gets_only_the_general_lines() {
        let text = text(&[entry("ext4", "rotational")]);
        assert!(text.starts_with("explain:"));
        assert!(text.contains("nothing printed here proves"));
        for topic in ["flash", "copy-on-write", "network", "fuse", "unknown"] {
            assert!(!text.contains(topic), "{topic}: {text}");
        }
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
}
