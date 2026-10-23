//! the linux erase of a whole disk or partition. `erase` resolves the targets, inspects them, asks for the
//! confirmation and writes. the steps live in the modules below.

mod facts;
mod log_check;
mod mounts;
mod summary;
mod sysfs;
mod target;
mod write;

use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli::Cli;
use crate::erase::{self, Options};
use crate::log::{Kind, Outcome};
use crate::prompt;
use crate::{EXIT_DECLINED, EXIT_INCOMPLETE, EXIT_INVALID, NAME};

use facts::{inspect, layout_error};
use log_check::{check_log, check_log_location};
use summary::summary;
use target::{Target, overlap_error};
use write::{buffer_for, open_for_write, overwrite, unchanged};

const SECTOR: u64 = 512;
/// size of each write. a power of two, so it is a multiple of every logical block size it accepts.
const CHUNK: usize = 1024 * 1024;
/// the write buffer is aligned to at least a page, as `O_DIRECT` asks, and to the logical block size.
const ALIGN: usize = 4096;

pub(super) fn erase(paths: &[PathBuf], cli: &Cli, opts: &Options) -> ExitCode {
    let mut targets = Vec::new();
    let mut invalid = false;
    for shown in paths {
        match Target::resolve(shown) {
            Ok(target) => targets.push(target),
            Err(err) => {
                eprintln!("{NAME}: {err}");
                invalid = true;
            }
        }
    }
    if invalid {
        return ExitCode::from(EXIT_INVALID);
    }
    if let Some(err) = overlap_error(&targets) {
        eprintln!("{NAME}: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let mut states = Vec::new();
    let mut refused = false;
    for target in &targets {
        let shown = target.shown.display();
        let facts = match inspect(target) {
            Ok(facts) => facts,
            Err(err) => {
                eprintln!("{NAME}: {shown}: {err}");
                return ExitCode::from(EXIT_INVALID);
            }
        };
        if let Some(err) = layout_error(&facts) {
            eprintln!("{NAME}: {shown}: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
        for problem in facts.problems() {
            if cli.allow_protected {
                eprintln!(
                    "{NAME}: warning: {shown}: {problem}, erasing it because --i-know-what-i-am-doing was given"
                );
            } else {
                eprintln!(
                    "{NAME}: {shown}: {problem}, refusing to erase it (--i-know-what-i-am-doing allows it)"
                );
                refused = true;
            }
        }
        states.push(facts);
    }
    if refused {
        return ExitCode::from(EXIT_INVALID);
    }

    if let Some(log_path) = &cli.log
        && !cli.dry_run
        && let Err(err) = check_log(log_path, paths, &targets, &states)
    {
        eprintln!("{NAME}: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    if cli.dry_run {
        if cli.json {
            eprintln!("{NAME}: --dry-run --json is not supported for devices yet");
            return ExitCode::from(EXIT_INVALID);
        }
        if cli.log.is_some() {
            eprintln!("note: --log is not written by --dry-run");
        }
        for (target, facts) in targets.iter().zip(&states) {
            println!(
                "would erase {} ({} passes)",
                target.shown.display(),
                cli.passes
            );
            for line in summary(target, facts, cli.passes) {
                println!("  {line}");
            }
        }
        return ExitCode::SUCCESS;
    }

    // the path is typed at the keyboard: a pipe cannot answer it, and -f does not skip it.
    if !io::stdin().is_terminal() {
        eprintln!(
            "{NAME}: stdin is not a terminal: the device path must be typed at a terminal, nothing was erased"
        );
        return ExitCode::from(EXIT_DECLINED);
    }
    if cli.force {
        eprintln!(
            "note: -f does not skip the confirmation for devices: each one is asked for by its path"
        );
    }
    // opened before the questions, so a missing permission is reported before anything is asked.
    let mut files = Vec::new();
    for target in &targets {
        match open_for_write(target) {
            Ok(file) => files.push(file),
            Err(err) => {
                eprintln!("{NAME}: {err}");
                return ExitCode::from(EXIT_INVALID);
            }
        }
    }

    for (target, facts) in targets.iter().zip(&states) {
        eprintln!(
            "the following device will be irreversibly erased ({} passes):",
            cli.passes
        );
        for line in summary(target, facts, cli.passes) {
            eprintln!("  {line}");
        }
        let question = format!("type the device path ({}) to erase it:", target.typed);
        if !prompt::ask_exact(&question, &target.typed) {
            eprintln!("aborted, nothing was erased.");
            return ExitCode::from(EXIT_DECLINED);
        }
    }

    let _interrupts = match crate::start_interrupt_guard() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let log_file = match &cli.log {
        Some(path) => match crate::open_log(path, cli.passes, paths) {
            Ok(log) => {
                // the location was checked before the confirmation; the mount table may have changed since.
                if let Err(err) = check_log_location(path, &targets, &states) {
                    drop(log);
                    if fs::remove_file(path).is_err() {
                        eprintln!("{NAME}: {}: could not remove the log", path.display());
                    }
                    eprintln!("{NAME}: {err}");
                    return ExitCode::from(EXIT_INVALID);
                }
                Some(log)
            }
            Err(code) => return code,
        },
        None => None,
    };

    for (index, ((target, facts), file)) in targets
        .iter()
        .zip(&states)
        .zip(files.iter_mut())
        .enumerate()
    {
        let shown = target.shown.display();
        let rest = if index == 0 {
            "no device was written"
        } else {
            "the remaining devices were not erased"
        };
        if log_file.as_ref().is_some_and(|log| log.failed()) {
            eprintln!("{NAME}: the log could not be written: {rest}");
            return ExitCode::from(EXIT_INCOMPLETE);
        }
        if erase::interrupted() {
            if index == 0 {
                eprintln!("aborted, nothing was erased.");
                return ExitCode::from(EXIT_DECLINED);
            }
            eprintln!("interrupted: {shown}: not erased, the remaining devices were skipped");
            return ExitCode::from(EXIT_INCOMPLETE);
        }
        // after the log exists and the handler is installed, right before this device is written.
        match unchanged(target, facts, file) {
            Ok(true) => {}
            Ok(false) if index == 0 => {
                eprintln!(
                    "{NAME}: {shown}: device state changed while waiting for confirmation, nothing was written"
                );
                return ExitCode::from(EXIT_INVALID);
            }
            Ok(false) => {
                eprintln!(
                    "{NAME}: {shown}: device state changed, the remaining devices were not erased"
                );
                return ExitCode::from(EXIT_INCOMPLETE);
            }
            Err(err) if index == 0 => {
                eprintln!("{NAME}: {err}");
                return ExitCode::from(EXIT_INVALID);
            }
            Err(err) => {
                eprintln!("{NAME}: {err}; the remaining devices were not erased");
                return ExitCode::from(EXIT_INCOMPLETE);
            }
        }

        let kind = if target.partition {
            Kind::Partition
        } else {
            Kind::Device
        };
        let mut buf = match buffer_for(facts.logical_block) {
            Ok(buf) => buf,
            Err(err) => {
                if let Some(log) = &log_file {
                    log.record(
                        &target.shown,
                        kind,
                        facts.size,
                        Outcome::Error,
                        &err.to_string(),
                    );
                }
                eprintln!("{NAME}: {shown}: {err}; {rest}");
                return ExitCode::from(if index == 0 {
                    EXIT_INVALID
                } else {
                    EXIT_INCOMPLETE
                });
            }
        };
        if cli.verbose {
            eprintln!("erasing {shown} ({} passes)", cli.passes);
        }
        match overwrite(file, facts.size, buf.as_mut_slice(), opts) {
            Ok(()) => {
                if let Some(log) = &log_file {
                    log.record(&target.shown, kind, facts.size, Outcome::Ok, "");
                }
                eprintln!("erased {shown} ({} passes)", cli.passes);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                if let Some(log) = &log_file {
                    log.record_interrupted(&target.shown, kind, facts.size);
                }
                eprintln!(
                    "interrupted: {shown}: device partly overwritten, the data cannot be restored"
                );
                return ExitCode::from(EXIT_INCOMPLETE);
            }
            Err(err) => {
                if let Some(log) = &log_file {
                    log.record(
                        &target.shown,
                        kind,
                        facts.size,
                        Outcome::Error,
                        &err.to_string(),
                    );
                }
                eprintln!("{NAME}: {shown}: {err}");
                return ExitCode::from(EXIT_INCOMPLETE);
            }
        }
    }

    if log_file.as_ref().is_some_and(|log| log.failed()) {
        ExitCode::from(EXIT_INCOMPLETE)
    } else {
        ExitCode::SUCCESS
    }
}
