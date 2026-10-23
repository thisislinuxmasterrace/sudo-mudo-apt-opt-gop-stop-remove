//! the summary shown before the confirmation: what the device is, what will be done, and that it cannot be undone.

use std::path::Path;

use crate::preview::human_bytes;

use super::facts::Facts;
use super::sysfs::read_text;
use super::target::Target;

/// the lines of the summary: what the device is, what will be done, and that it cannot be undone.
pub(super) fn summary(target: &Target, facts: &Facts, passes: u32) -> Vec<String> {
    let (model, serial) = identity(target);
    let mut lines = Vec::new();
    if target.shown == target.path {
        lines.push(format!("path: {}", target.shown.display()));
    } else {
        lines.push(format!(
            "path: {} (resolved to {})",
            target.shown.display(),
            target.path.display()
        ));
    }
    if target.partition {
        lines.push(format!("partition of /dev/{}", target.disk_name()));
    } else {
        lines.push("whole disk".to_owned());
    }
    lines.push(format!("model: {model}, serial: {serial}"));
    lines.push(format!(
        "size: {}, logical block: {}",
        human_bytes(facts.size),
        human_bytes(facts.logical_block)
    ));
    if facts.rotational == "1" {
        lines.push("media: hard disk".to_owned());
    } else {
        lines.push(format!(
            "media: not known to be a hard disk (queue/rotational is {})",
            facts.rotational
        ));
    }
    if !target.partition {
        lines.push("partitions:".to_owned());
        if facts.partitions.is_empty() {
            lines.push("  none".to_owned());
        }
        for (name, size) in &facts.partitions {
            let points: Vec<String> = facts
                .mounts
                .iter()
                .filter(|(mounted, _)| mounted == name)
                .map(|(_, point)| point.display().to_string())
                .collect();
            let state = if points.is_empty() {
                "not mounted".to_owned()
            } else {
                format!("mounted at {}", points.join(", "))
            };
            lines.push(format!("  /dev/{name}: {}, {state}", human_bytes(*size)));
        }
    }
    lines.push(format!("method: overwrite, {passes} passes"));
    lines.push("the erase is irreversible: the data cannot be recovered".to_owned());
    lines
}

/// model and serial number of the disk. nvme controllers keep the serial in `/sys/class/nvme`.
/// `unknown` when the kernel does not say.
fn identity(target: &Target) -> (String, String) {
    let disk = target.disk_dir();
    let controller =
        nvme_controller(&target.disk_name()).map(|name| Path::new("/sys/class/nvme").join(name));
    let model = read_text(&disk.join("device/model")).or_else(|| {
        controller
            .as_ref()
            .and_then(|dir| read_text(&dir.join("model")))
    });
    let serial = controller
        .as_ref()
        .and_then(|dir| read_text(&dir.join("serial")))
        .or_else(|| read_text(&disk.join("device/serial")));
    (
        model.unwrap_or_else(|| "unknown".to_owned()),
        serial.unwrap_or_else(|| "unknown".to_owned()),
    )
}

/// the controller of an nvme namespace: `nvme0n1` is on `nvme0`, `nvme12n1` on `nvme12`.
fn nvme_controller(disk: &str) -> Option<String> {
    let digits: String = disk
        .strip_prefix("nvme")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then(|| format!("nvme{digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_namespaces_are_on_their_controller() {
        assert_eq!(nvme_controller("nvme0n1").as_deref(), Some("nvme0"));
        assert_eq!(nvme_controller("nvme12n1").as_deref(), Some("nvme12"));
        assert_eq!(nvme_controller("sdb"), None);
    }
}
