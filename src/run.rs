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
                crate::ui::shown(path)
            );
            true
        }
        Err(err) => {
            crate::ui::message!("{NAME}: {}: {err}", crate::ui::shown(path));
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
                        crate::ui::shown(path)
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
    #[cfg(unix)]
    let selected = match paths
        .iter()
        .map(|path| {
            let selected = erase::Selected::open(path)?;
            selected.check(cli.recursive, cli.allow_protected)?;
            Ok(selected)
        })
        .collect::<std::io::Result<Vec<_>>>()
    {
        Ok(selected) => selected,
        Err(err) => {
            crate::ui::message!("{NAME}: could not retain selected paths: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if let Some(log_path) = &cli.log
        && !cli.dry_run
        && let Err(err) = log::check_path(log_path, paths)
    {
        crate::ui::message!("{NAME}: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    // warnings about these paths are always shown; the general notes below are left out by `-q -f`.
    let mut warnings: Vec<_> = storage::warnings(paths)
        .into_iter()
        .map(|warning| format!("warning: {warning}"))
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
        .map_or_else(Vec::new, |total| total.lines(cli.passes));

    if cli.dry_run {
        for warning in &warnings {
            crate::ui::message!("{warning}");
        }
        if cli.log.is_some() {
            crate::ui::message!("note: --log is not written by --dry-run");
        }
        if let Some(total) = total.as_ref().filter(|_| cli.json) {
            println!("{}", preview::to_json(paths, cli.passes, total));
            return ExitCode::SUCCESS;
        }
        crate::ui::section("dry run: would erase", false);
        for path in paths {
            crate::ui::stdout_line(format_args!("  {}", crate::ui::shown(path)));
        }
        crate::ui::section("summary", false);
        for line in &summary {
            crate::ui::stdout_line(format_args!("  {line}"));
        }
        if cli.explain
            && let Some(total) = &total
        {
            crate::ui::section("advice", false);
            let targets = explain::Targets {
                paths,
                recursive: cli.recursive,
                excludes: &cli.exclude,
            };
            for line in explain::lines(&targets, &total.assessments) {
                crate::ui::stdout_line(format_args!("{line}"));
            }
        }
        return ExitCode::SUCCESS;
    }

    if erase::running_as_root()
        && !cli.allow_protected
        && total.as_ref().is_some_and(|total| total.hard_linked > 0)
    {
        warnings.push(
            "note: under root, hard-linked files are refused unless every name is inside the same erased tree; --i-know-what-i-am-doing allows other links"
        .to_owned());
    }
    let mut notes = warnings.clone();
    notes.push("note: overwriting is best effort: ssds, snapshots, copy-on-write filesystems and backups can keep old data. see --explain.".to_owned());
    if cli.verify {
        notes.push(
            "note: --verify checks the new contents, not whether old copies are gone".to_owned(),
        );
    }
    let strict = match &counted {
        Some(parts) if cli.recursive => strict_paths(paths, parts),
        _ => Vec::new(),
    };
    if cli.force {
        crate::ui::risks(if cli.quiet { &warnings } else { &notes }, false);
    }
    if !cli.force && !confirm(paths, &summary, &strict, &notes) {
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
                crate::ui::shown(path)
            );
            return ExitCode::from(EXIT_INCOMPLETE);
        }
        if cli.verbose {
            crate::ui::message!(
                "erasing {} ({})",
                crate::ui::shown(path),
                crate::ui::count(cli.passes.into(), "pass", "passes")
            );
        }
        #[cfg(unix)]
        let result = erase::erase_selected(
            path,
            &selected[index],
            opts,
            &cli.exclude,
            log_file.as_ref(),
        );
        #[cfg(not(unix))]
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
                        crate::ui::shown(path)
                    );
                } else {
                    crate::ui::message!("interrupted: {}: {err}", crate::ui::shown(path));
                }
                return ExitCode::from(EXIT_INCOMPLETE);
            }
            crate::ui::message!("{NAME}: {}: {err}", crate::ui::shown(path));
            failed = true;
        }
    }

    // a line that could not be written leaves its object erased without a line: that is not a success.
    let failed = failed || log_file.as_ref().is_some_and(|log| log.failed());
    crate::ui::result(paths, erased, kept, failed, cli.quiet, !cli.force);
    if failed {
        ExitCode::from(EXIT_INCOMPLETE)
    } else {
        ExitCode::SUCCESS
    }
}

pub fn run_wipe_free(dir: &Path, cli: &Cli, opts: &erase::Options) -> ExitCode {
    let selected = match wipe::Directory::open(dir, cli.allow_protected) {
        Ok(selected) => selected,
        Err(err) => {
            crate::ui::message!("{NAME}: {}: {err}", crate::ui::shown(dir));
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if selected.is_protected() {
        crate::ui::message!(
            "{NAME}: warning: {}: protected system path, filling it because --i-know-what-i-am-doing was given",
            crate::ui::shown(dir)
        );
    }

    // warnings describe the filesystem that gets filled, so a link is resolved first.
    let filled = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for warning in storage::warnings(&[filled]) {
        crate::ui::message!("{NAME}: warning: {warning}");
    }

    let space = space_lines(dir);

    if cli.dry_run {
        crate::ui::section("dry run: would fill the free space of", false);
        crate::ui::stdout_line(format_args!("  {}", crate::ui::shown(dir)));
        crate::ui::section("summary", false);
        for line in &space {
            crate::ui::stdout_line(format_args!("  {line}"));
        }
        crate::ui::stdout_line(format_args!(
            "  {}",
            crate::ui::count(cli.passes.into(), "pass", "passes")
        ));
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

    match wipe::wipe_selected(&selected, opts) {
        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
            crate::ui::message!(
                "interrupted: {}: free space was partly wiped, the temporary files are removed",
                crate::ui::shown(dir)
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(outcome) if outcome.exceeded_pass.is_some() => {
            crate::ui::message!(
                "{NAME}: {}: free space may not be wiped, the temporary files are removed",
                crate::ui::shown(dir)
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(outcome) if outcome.written == 0 => {
            crate::ui::message!(
                "{NAME}: {}: no free space was written, nothing was wiped (the volume has no free blocks left)",
                crate::ui::shown(dir)
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(outcome) => {
            let written = outcome.written;
            if !cli.force {
                crate::ui::section("result", true);
            }
            crate::ui::message!(
                "free space of {} erased ({} written)",
                crate::ui::shown(dir),
                preview::human_bytes(written)
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            crate::ui::message!("{NAME}: {}: {err}", crate::ui::shown(dir));
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
            crate::ui::shown(path)
        );
        ExitCode::from(EXIT_INVALID)
    })?;
    if let Err(err) = log::check_location(path, targets) {
        drop(created);
        if std::fs::remove_file(path).is_err() {
            crate::ui::message!(
                "{NAME}: {}: could not remove the log",
                crate::ui::shown(path)
            );
        }
        crate::ui::message!("{NAME}: {err}");
        return Err(ExitCode::from(EXIT_INVALID));
    }
    Ok(created)
}

/// the space a wipe can use on this volume, for the dry run and the prompt. the run is root, so
/// the figure includes the blocks reserved for root.
fn space_lines(dir: &Path) -> Vec<String> {
    match storage::free_bytes(dir) {
        Some(0) => vec!["free space: none, a wipe would write nothing".to_owned()],
        Some(bytes) => {
            let label = if cfg!(unix) {
                "free space (including blocks reserved for root)"
            } else {
                "free space"
            };
            vec![format!("{label}: {}", preview::human_bytes(bytes))]
        }
        None => vec!["free space: unknown".to_owned()],
    }
}
