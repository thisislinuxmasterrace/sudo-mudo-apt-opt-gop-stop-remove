//! whole-device advice; commands are examples and are never executed.

pub(super) fn lines(partition: bool, non_rotational: bool) -> Vec<&'static str> {
    let mut lines = vec![
        "an overwrite reaches addressable blocks only; remapped blocks, snapshots and backups may retain data.",
        "the commands below are alternatives to investigate, not a recommended sequence. check device identity, backups and manufacturer support first.",
    ];
    if partition {
        lines.push("this target is a partition. firmware erase can affect the whole physical drive, including other partitions; do not substitute this partition path into a firmware command.");
    }
    if non_rotational {
        lines.push(
            "flash/non-rotational storage may keep blocks that ordinary overwrites cannot reach.",
        );
    }
    lines.extend([
        "nvme: inspect supported sanitize actions with `nvme id-ctrl /dev/<controller>`. `nvme sanitize /dev/<controller> --sanact=2` requests block erase; `nvme format /dev/<namespace> --ses=1` requests user-data erase. support and affected namespaces depend on the controller; both can destroy data beyond this target.",
        "ata: inspect security support and frozen state with `hdparm -I /dev/<whole-disk>`. security erase requires setting a password first; interruption can leave the disk locked. follow the manufacturer's procedure rather than copying a generic password sequence.",
        "luks only: `cryptsetup luksErase /dev/<luks-volume>` removes keyslots without overwriting the data area. it destroys access only if no usable key, header backup or other unlocked mapping remains. it is not a file-level erase and does not apply to every dm-crypt mapping.",
        "no command shown here proves physical erasure. usb bridges, virtual storage and unsupported firmware may prevent the intended operation.",
    ]);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_advice_preserves_scope_and_key_backup_limits() {
        let text = lines(true, true).join("\n");
        assert!(text.contains("including other partitions"));
        assert!(text.contains("header backup"));
        assert!(text.contains("unlocked mapping"));
        assert!(text.contains("not a recommended sequence"));
        assert!(
            !lines(false, false)
                .join("\n")
                .contains("this target is a partition")
        );
    }
}
