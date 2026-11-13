use super::Summary;
use crate::json;
use std::path::PathBuf;

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
    out.push(']');
    #[cfg(unix)]
    if paths.iter().any(|path| path.to_str().is_none()) {
        out.push_str(",\"path_bytes\":[");
        for (index, path) in paths.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            if path.to_str().is_some() {
                out.push_str("null");
            } else {
                json::push_path_bytes(&mut out, path);
            }
        }
        out.push(']');
    }
    out.push_str(",\"assessments\":[");
    for (index, entry) in total.assessments.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        json::push_path_fields(&mut out, &entry.path);
        entry.assessment.write_fields(&mut out);
        out.push('}');
    }
    out.push_str("],\"assessment_coverage\":");
    json::push_str(
        &mut out,
        if total.incomplete() {
            "incomplete"
        } else {
            "visited_entries"
        },
    );
    out.push_str(",\"passes\":");
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
