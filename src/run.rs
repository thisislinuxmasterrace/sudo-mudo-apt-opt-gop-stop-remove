//! the two runs: erase of paths and `--wipe-free`, from validation to the exit code.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::cli::Cli;
use crate::confirm::{confirm, confirm_wipe, strict_paths};
use crate::{
    EXIT_DECLINED, EXIT_INCOMPLETE, EXIT_INVALID, NAME, device, erase, explain, log, preview,
    safety, storage, wipe,
};

/// a mount point given as a target is refused up front; inside a tree the erase skips it instead.
/// unix only: `is_target_boundary` opens the parent directory, which windows does not allow, so on
/// windows it would refuse every target.
#[cfg(unix)]
fn target_is_mount_point(path: &Path) -> bool {
    match preview::is_target_boundary(path) {
        Ok(false) => false,
        Ok(true) => {
            crate::ui::message!(
                "{NAME}: {}: is a mount point or on another filesystem, refusing to erase it",
                path.display()
            );
            true
        }
        Err(err) => {
            crate::ui::message!("{NAME}: {}: {err}", path.display());
            true
        }
    }
}

#[cfg(not(unix))]
fn target_is_mount_point(_path: &Path) -> bool {
    false
}

pub fn run_erase(paths: &[PathBuf], cli: &Cli, opts: &erase::Options) -> ExitCode {
    match device::is_device_run(paths) {
        Ok(true) => {
            if cli.explain {
                crate::ui::message!(
                    "note: --explain gives no advice for devices yet, showing the dry run"
                );
            }
            return device::run(paths, cli, opts);
        }
        Ok(false) => {}
        Err(err) => {
            crate::ui::message!("{NAME}: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
    }

    // validate every path before touching any file: one bad path aborts the whole run.
    let mut invalid = false;
    for path in paths {
        match safety::validate_target(path, cli.recursive, cli.allow_protected) {
            Ok(protected) => {
                if protected {
                    crate::ui::message!(
                        "{NAME}: warning: {}: protected system path, erasing it because --i-know-what-i-am-doing was given",
                        path.display()
                    );
                }
                if let Err(err) = erase::validate_hard_link(path, cli.allow_protected) {
                    crate::ui::message!("{NAME}: {err}");
                    invalid = true;
                }
                if target_is_mount_point(path) {
                    invalid = true;
                }
            }
            Err(err) => {
                crate::ui::message!("{NAME}: {err}");
                invalid = true;
            }
        }
    }
    if invalid {
        return ExitCode::from(EXIT_INVALID);
    }
    if let Some(log_path) = &cli.log
        && !cli.dry_run
        && let Err(err) = log::check_path(log_path, paths)
    {
        crate::ui::message!("{NAME}: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    let mut notes: Vec<_> = storage::warnings(paths)
        .into_iter()
        .map(|warning| format!("{NAME}: warning: {warning}"))
        .collect();

    // counting walks the whole tree, so it only happens when someone is going to read it.
    let counted = if cli.dry_run || !cli.force {
        Some(preview::summarize(
            paths,
            cli.recursive,
            &cli.exclude,
            cli.allow_protected,
        ))
    } else {
        None
    };
    let total = counted.as_deref().map(preview::total);
    let summary = total
        .as_ref()
        .map_or_else(Vec::new, preview::Summary::lines);

    if cli.dry_run {
        for note in &notes {
            crate::ui::message!("{note}");
        }
        if cli.log.is_some() {
            crate::ui::message!("note: --log is not written by --dry-run");
        }
        if let Some(total) = total.as_ref().filter(|_| cli.json) {
            println!("{}", preview::to_json(paths, cli.passes, total));
            return ExitCode::SUCCESS;
        }
        crate::ui::section("dry run: targets", false);
        for path in paths {
            crate::ui::stdout_line(format_args!(
                "would erase {} ({} passes)",
                path.display(),
                cli.passes
            ));
        }
        crate::ui::section("summary", false);
        for line in &summary {
            crate::ui::stdout_line(format_args!("  {line}"));
        }
        if cli.explain
            && let Some(total) = &total
        {
            crate::ui::section("advice", false);
            for line in explain::advice(&total.assessments) {
                crate::ui::stdout_line(format_args!("{line}"));
            }
        }
        return ExitCode::SUCCESS;
    }

    notes.push("note: overwriting does not guarantee erasure on ssds, with snapshots, or on copy-on-write filesystems; backups and other copies may remain (use --explain for risks or --log for per-entry assessments)".to_owned());
    if cli.verify {
        notes.push(
            "note: --verify reads the data back and compares it; a match does not prove the old data is gone (ssd, copy-on-write, caches and remapped blocks are not checked)"
        .to_owned());
    }
    if erase::running_as_root()
        && !cli.allow_protected
        && total.as_ref().is_some_and(|total| total.hard_linked > 0)
    {
        notes.push(
            "note: under root, hard-linked files are refused unless every name is selected inside the same tree; external or unproven links need --i-know-what-i-am-doing"
        .to_owned());
    }
    let strict = match &counted {
        Some(parts) if cli.recursive => strict_paths(paths, parts),
        _ => Vec::new(),
    };
    if cli.force {
        if cli.quiet {
            for note in &notes {
                crate::ui::message!("{note}");
            }
        } else {
            crate::ui::risks(&notes);
        }
    }
    if !cli.force && !confirm(paths, &summary, cli.passes, &strict, &notes) {
        crate::ui::message!("aborted, nothing was erased.");
        return ExitCode::from(EXIT_DECLINED);
    }

    let _interrupts = match start_interrupt_guard() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let log_file = match &cli.log {
        Some(log_path) => match open_log(log_path, cli.passes, paths) {
            Ok(log) => Some(log),
            Err(code) => return code,
        },
        None => None,
    };

    let mut failed = false;
    let mut erased = 0;
    let mut kept = 0;
    for (index, path) in paths.iter().enumerate() {
        if log_file.as_ref().is_some_and(|log| log.failed()) {
            crate::ui::message!(
                "{NAME}: the log could not be written: the remaining paths were not erased"
            );
            break;
        }
        if erase::interrupted() {
            if index == 0 {
                crate::ui::message!("aborted, nothing was erased.");
                return ExitCode::from(EXIT_DECLINED);
            }
            crate::ui::message!(
                "interrupted: {}: not erased, the remaining paths were skipped",
                path.display()
            );
            return ExitCode::from(EXIT_INCOMPLETE);
        }
        if cli.verbose {
            crate::ui::message!("erasing {} ({} passes)", path.display(), cli.passes);
        }
        let result = erase::erase_path(path, opts, &cli.exclude, log_file.as_ref());
        match &result {
            Ok(false) => erased += 1,
            Ok(true) => kept += 1,
            Err(_) => {}
        }
        if let Err(err) = result {
            if err.kind() == std::io::ErrorKind::Interrupted {
                if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
                    crate::ui::message!(
                        "interrupted: {}: an entry may be partly overwritten, the rest was kept",
                        path.display()
                    );
                } else {
                    crate::ui::message!("interrupted: {}: {err}", path.display());
                }
                return ExitCode::from(EXIT_INCOMPLETE);
            }
            crate::ui::message!("{NAME}: {}: {err}", path.display());
            failed = true;
        }
    }

    // a line that could not be written leaves its object erased without a line: that is not a success.
    let failed = failed || log_file.as_ref().is_some_and(|log| log.failed());
    crate::ui::result(erased, kept, failed, cli.quiet);
    if failed {
        ExitCode::from(EXIT_INCOMPLETE)
    } else {
        ExitCode::SUCCESS
    }
}

pub fn run_wipe_free(dir: &Path, cli: &Cli, opts: &erase::Options) -> ExitCode {
    match safety::validate_wipe_dir(dir, cli.allow_protected) {
        Ok(true) => crate::ui::message!(
            "{NAME}: warning: {}: protected system path, filling it because --i-know-what-i-am-doing was given",
            dir.display()
        ),
        Ok(false) => {}
        Err(err) => {
            crate::ui::message!("{NAME}: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
    }

    if storage::is_network(dir) {
        crate::ui::message!(
            "{NAME}: {}: refusing to fill a network filesystem, its free space belongs to the server and other users",
            dir.display()
        );
        return ExitCode::from(EXIT_INVALID);
    }

    for warning in storage::warnings(&[dir.to_path_buf()]) {
        crate::ui::message!("{NAME}: warning: {warning}");
    }

    let space = space_lines(dir);

    if cli.dry_run {
        crate::ui::section("dry run: free space", false);
        println!(
            "would fill and erase the free space of {} ({} passes)",
            dir.display(),
            cli.passes
        );
        crate::ui::section("summary", false);
        for line in &space {
            crate::ui::stdout_line(format_args!("  {line}"));
        }
        return ExitCode::SUCCESS;
    }

    if !cli.force && !confirm_wipe(dir, cli.passes, &space) {
        crate::ui::message!("aborted, nothing was erased.");
        return ExitCode::from(EXIT_DECLINED);
    }

    let _interrupts = match start_interrupt_guard() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    if erase::interrupted() {
        crate::ui::message!("aborted, nothing was erased.");
        return ExitCode::from(EXIT_DECLINED);
    }

    match wipe::wipe_free_space(dir, opts) {
        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
            crate::ui::message!(
                "interrupted: {}: free space was partly wiped, the temporary files are removed",
                dir.display()
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(0) => {
            crate::ui::message!(
                "{NAME}: {}: no free space was written, nothing was wiped (the volume is full, or there is no space for this user)",
                dir.display()
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(written) => {
            crate::ui::section("result", true);
            crate::ui::message!(
                "free space of {} erased ({} written)",
                dir.display(),
                preview::human_bytes(written)
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            crate::ui::message!("{NAME}: {}: {err}", dir.display());
            ExitCode::from(EXIT_INCOMPLETE)
        }
    }
}

/// routes ctrl+c, sigterm and sighup to the interrupt flag for the erase that follows the confirmation.
pub fn start_interrupt_guard() -> Result<erase::InterruptGuard, ExitCode> {
    erase::InterruptGuard::install().map_err(|err| {
        crate::ui::message!("{NAME}: could not install the interrupt handler: {err}");
        ExitCode::from(EXIT_INVALID)
    })
}

/// creates the `--log` file once the erase is confirmed. the note comes first: the log holds names.
/// the location is checked again once the file exists; if the check fails, the new file is removed.
pub fn open_log(path: &Path, passes: u32, targets: &[PathBuf]) -> Result<log::Log, ExitCode> {
    if cfg!(unix) {
        crate::ui::message!("note: the log contains file names; it is created with mode 0600");
    }
    let created = log::Log::create(path, passes).map_err(|err| {
        crate::ui::message!(
            "{NAME}: {}: could not create the log: {err}",
            path.display()
        );
        ExitCode::from(EXIT_INVALID)
    })?;
    if let Err(err) = log::check_location(path, targets) {
        drop(created);
        if std::fs::remove_file(path).is_err() {
            crate::ui::message!("{NAME}: {}: could not remove the log", path.display());
        }
        crate::ui::message!("{NAME}: {err}");
        return Err(ExitCode::from(EXIT_INVALID));
    }
    Ok(created)
}

/// the space a wipe can use on this volume, for the dry run and the prompt.
fn space_lines(dir: &Path) -> Vec<String> {
    match storage::available_bytes(dir) {
        Some(0) => vec![
            "no space is available to this user: a wipe fills nothing unless run as root"
                .to_owned(),
        ],
        Some(bytes) => vec![format!(
            "space available to this user: {}",
            preview::human_bytes(bytes)
        )],
        None => vec!["space available to this user: unknown".to_owned()],
    }
}
