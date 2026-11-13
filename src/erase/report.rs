//! log lines for the objects an erase handles, and the stop of an erase once the log cannot be written.

use std::io::{self, Write};
use std::path::Path;

use crate::assessment::Assessment;
use crate::log::{Kind, Log, Outcome};

#[cfg(windows)]
pub(super) fn assess(path: &Path) -> Assessment {
    std::fs::symlink_metadata(path).map_or_else(
        |_| Assessment::unknown(),
        |meta| Assessment::for_path(path, &meta),
    )
}

/// writes one line to the log, when there is one. `passes` is recorded for regular files only.
pub(super) fn log_object(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    outcome: Outcome,
    detail: &str,
    assessment: &Assessment,
) {
    log_object_to(
        log,
        shown,
        kind,
        size,
        outcome,
        detail,
        assessment,
        &mut io::stderr().lock(),
    );
}

#[allow(clippy::too_many_arguments)]
fn log_object_to(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    outcome: Outcome,
    detail: &str,
    assessment: &Assessment,
    output: &mut impl Write,
) {
    let assessment = if matches!(outcome, Outcome::Skipped | Outcome::Refused) {
        assessment.unattempted()
    } else {
        assessment.clone()
    };
    if let Some(log) = log {
        log.record_assessed(shown, kind, size, outcome, detail, Some(&assessment));
    }
    print_assessment(output, shown, &assessment);
}

fn print_assessment(output: &mut impl Write, shown: &Path, assessment: &Assessment) {
    let _ = writeln!(
        output,
        "note: assessment: {}: {}",
        shown.display(),
        assessment.line()
    );
}

/// writes the line of an object whose erase returned `result`. `refused` marks an object the erase
/// refused before touching it; `warning` is the note for an erase that went ahead anyway.
#[allow(clippy::too_many_arguments)]
pub(super) fn log_done(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    result: &io::Result<bool>,
    refused: bool,
    warning: &str,
    assessment: &Assessment,
) {
    match result {
        Ok(false) => log_object(log, shown, kind, size, Outcome::Ok, warning, assessment),
        Ok(true) => log_object(
            log,
            shown,
            kind,
            size,
            Outcome::Kept,
            "kept: an entry inside is excluded by --exclude",
            assessment,
        ),
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            if let Some(log) = log {
                log.record_interrupted_assessed(shown, kind, size, Some(assessment));
            }
            print_assessment(&mut io::stderr().lock(), shown, assessment);
        }
        Err(err) => {
            let outcome = if refused {
                Outcome::Refused
            } else {
                Outcome::Error
            };
            log_object(
                log,
                shown,
                kind,
                size,
                outcome,
                &err.to_string(),
                assessment,
            );
        }
    }
}

/// writes the line of an entry that could not be inspected, so its kind is not known.
pub(super) fn unseen(log: Option<&Log>, shown: &Path, err: io::Error) -> io::Error {
    log_object(
        log,
        shown,
        Kind::Other,
        0,
        Outcome::Error,
        &err.to_string(),
        &Assessment::unknown(),
    );
    err
}

/// refuses to erase anything once a log line could not be written: nothing is erased without its log.
pub(super) fn log_stopped(log: Option<&Log>) -> io::Result<()> {
    match log {
        Some(log) if log.failed() => Err(io::Error::other(
            "the log could not be written, erasing stopped",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::test_support::Memory;

    #[test]
    fn failed_advisory_output_does_not_prevent_log() {
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("output failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let memory = Memory::default();
        let log = Log::new(Path::new("erase.log"), 1, Box::new(memory.clone()));
        log_object_to(
            Some(&log),
            Path::new("file"),
            Kind::File,
            1,
            Outcome::Ok,
            "",
            &Assessment::unknown(),
            &mut Failed,
        );
        assert!(memory.text().contains("\"result\":\"ok\""));
    }

    #[test]
    fn interrupted_object_is_logged_once_however_many_parents_stop() {
        let memory = Memory::default();
        let log = Log::new(Path::new("erase.log"), 1, Box::new(memory.clone()));
        let interrupted = || -> io::Result<bool> {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "interrupted by signal",
            ))
        };

        log_done(
            Some(&log),
            Path::new("dir/a.txt"),
            Kind::File,
            5,
            &interrupted(),
            false,
            "",
            &Assessment::unknown(),
        );
        log_done(
            Some(&log),
            Path::new("dir"),
            Kind::Dir,
            0,
            &interrupted(),
            false,
            "",
            &Assessment::unknown(),
        );

        let text = memory.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"path\":\"dir/a.txt\""));
        assert!(text.contains("\"result\":\"error\",\"detail\":\"interrupted\""));
    }
}
