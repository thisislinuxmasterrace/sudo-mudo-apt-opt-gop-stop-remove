//! the questions asked before an erase: the confirmation, the strict one for large trees, and the wipe one.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{preview, prompt};

// a recursive erase of one path above either limit needs the directory name typed instead of "y".
const STRICT_OBJECTS: u64 = 1000;
const STRICT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

// the confirmation lists this many paths in full; the rest are counted, so a long list stays readable.
const LISTED_PATHS: usize = 20;

/// true when a recursive erase of one path is large enough to need its name typed instead of "y".
fn needs_strict_confirmation(objects: u64, bytes: u64) -> bool {
    objects > STRICT_OBJECTS || bytes > STRICT_BYTES
}

/// the paths whose names are typed before a recursive erase. a path above the limits by itself asks
/// for its own name. when only the total is above them, the path with the most objects asks instead.
pub fn strict_paths<'a>(
    paths: &'a [PathBuf],
    parts: &'a [preview::Summary],
) -> Vec<(&'a PathBuf, &'a preview::Summary)> {
    let own: Vec<_> = paths
        .iter()
        .zip(parts)
        .filter(|(_, part)| needs_strict_confirmation(part.objects(), part.bytes))
        .collect();
    if !own.is_empty() {
        return own;
    }
    let total = preview::total(parts);
    if needs_strict_confirmation(total.objects(), total.bytes) {
        paths
            .iter()
            .zip(parts)
            .max_by_key(|(_, part)| part.objects())
            .into_iter()
            .collect()
    } else {
        Vec::new()
    }
}

/// with no strict path the run asks "continue?". with strict paths the names come first, and
/// "continue?" follows when some path was not named, so the whole run is confirmed.
/// the header carries the number of paths, so it is still known when the list is cut.
pub fn confirm(
    paths: &[PathBuf],
    summary: &[String],
    passes: u32,
    strict: &[(&PathBuf, &preview::Summary)],
) -> bool {
    let noun = if paths.len() == 1 { "path" } else { "paths" };
    eprintln!(
        "the following {} {noun} will be irreversibly erased ({passes} passes):",
        paths.len()
    );
    for path in paths.iter().take(LISTED_PATHS) {
        eprintln!("  {}", path.display());
    }
    if paths.len() > LISTED_PATHS {
        eprintln!("  ... and {} more", paths.len() - LISTED_PATHS);
    }
    for line in summary {
        eprintln!("  {line}");
    }
    if strict.is_empty() {
        return prompt::ask("continue?");
    }
    strict.iter().all(|(path, part)| confirm_name(path, part))
        && (strict.len() == paths.len() || prompt::ask("continue?"))
}

/// a large path is confirmed by typing its last path component, exactly as it is named.
fn confirm_name(path: &Path, part: &preview::Summary) -> bool {
    let name = path.file_name().unwrap_or(path.as_os_str());
    let kind = if part.dirs > 0 { "directory" } else { "file" };
    let question = format!(
        "this is a large erase ({} objects, {}): type the name of the {kind} ({}) to continue:",
        part.objects(),
        preview::human_bytes(part.bytes),
        name.to_string_lossy()
    );
    prompt::read_line(&format!("{question} "))
        .as_deref()
        .map(OsStr::new)
        == Some(name)
}

pub fn confirm_wipe(dir: &Path, passes: u32, space: &[String]) -> bool {
    eprintln!(
        "note: the volume is filled completely for a while, this can take a long time on large disks"
    );
    eprintln!("note: other programs writing to this volume may fail while it is full");
    eprintln!(
        "the free space of {} will be filled and overwritten ({passes} passes)",
        dir.display()
    );
    for line in space {
        eprintln!("  {line}");
    }
    prompt::ask("continue?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_confirmation_starts_above_a_thousand_objects() {
        assert!(!needs_strict_confirmation(1000, 0));
        assert!(needs_strict_confirmation(1001, 0));
    }

    #[test]
    fn strict_confirmation_starts_above_ten_gib() {
        const GIB: u64 = 1024 * 1024 * 1024;

        assert!(!needs_strict_confirmation(0, 10 * GIB));
        assert!(needs_strict_confirmation(0, 10 * GIB + 1));
    }
}
