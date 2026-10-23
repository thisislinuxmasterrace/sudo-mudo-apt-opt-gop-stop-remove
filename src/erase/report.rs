//! log lines for the objects an erase handles, and the stop of an erase once the log cannot be written.

use std::io;
use std::path::Path;

use crate::log::{Kind, Log, Outcome};

/// writes one line to the log, when there is one. `passes` is recorded for regular files only.
pub(super) fn log_object(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    outcome: Outcome,
    detail: &str,
) {
    if let Some(log) = log {
        log.record(shown, kind, size, outcome, detail);
    }
}

/// writes the line of an object whose erase returned `result`. `refused` marks an object the erase
/// refused before touching it; `warning` is the note for an erase that went ahead anyway.
pub(super) fn log_done(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    result: &io::Result<bool>,
    refused: bool,
    warning: &str,
) {
    match result {
        Ok(false) => log_object(log, shown, kind, size, Outcome::Ok, warning),
        Ok(true) => log_object(
            log,
            shown,
            kind,
            size,
            Outcome::Kept,
            "kept: an entry inside is excluded by --exclude",
        ),
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            if let Some(log) = log {
                log.record_interrupted(shown, kind, size);
            }
        }
        Err(err) => {
            let outcome = if refused {
                Outcome::Refused
            } else {
                Outcome::Error
            };
            log_object(log, shown, kind, size, outcome, &err.to_string());
        }
    }
}

/// writes the line of an entry that could not be inspected, so its kind is not known.
pub(super) fn unseen(log: Option<&Log>, shown: &Path, err: io::Error) -> io::Error {
    log_object(log, shown, Kind::Other, 0, Outcome::Error, &err.to_string());
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
        );
        log_done(
            Some(&log),
            Path::new("dir"),
            Kind::Dir,
            0,
            &interrupted(),
            false,
            "",
        );

        let text = memory.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"path\":\"dir/a.txt\""));
        assert!(text.contains("\"result\":\"error\",\"detail\":\"interrupted\""));
    }
}
