mod cli;
mod erase;
mod platform;
mod preview;
mod prompt;
mod safety;
mod storage;
mod wipe;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use cli::Cli;

const NAME: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let cli = Cli::parse();

    if !cfg!(windows) && (cli.kill_holders || cli.raw_volume) {
        eprintln!("{NAME}: --kill-holders and --raw-volume are only supported on windows");
        return ExitCode::FAILURE;
    }

    let opts = erase::Options {
        passes: cli.passes,
        verbose: cli.verbose,
        force: cli.force,
        kill_holders: cli.kill_holders,
        raw_volume: cli.raw_volume,
    };

    // trailing slashes and `.` make `lstat` follow a symlink (`link/` is the target directory),
    // so every path is normalized before it is checked or used.
    let paths: Vec<PathBuf> = cli.paths.iter().map(|path| normalize(path)).collect();

    match &cli.wipe_free {
        Some(dir) => run_wipe_free(&normalize(dir), &cli, &opts),
        None => run_erase(&paths, &cli, &opts),
    }
}

fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

fn run_erase(paths: &[PathBuf], cli: &Cli, opts: &erase::Options) -> ExitCode {
    // validate every path before touching any file: one bad path aborts the whole run.
    let mut invalid = false;
    for path in paths {
        if let Err(err) = safety::validate_target(path, cli.recursive) {
            eprintln!("{NAME}: {err}");
            invalid = true;
        }
    }
    if invalid {
        return ExitCode::FAILURE;
    }

    for warning in storage::warnings(paths) {
        eprintln!("{NAME}: warning: {warning}");
    }

    // counting walks the whole tree, so it only happens when someone is going to read it.
    let summary = if cli.dry_run || !cli.force {
        preview::summarize(paths, cli.recursive).lines()
    } else {
        Vec::new()
    };

    if cli.dry_run {
        for path in paths {
            println!("would erase {} ({} passes)", path.display(), cli.passes);
        }
        for line in &summary {
            println!("  {line}");
        }
        return ExitCode::SUCCESS;
    }

    if !cli.force && !confirm(paths, &summary, cli.passes) {
        eprintln!("aborted, nothing was erased.");
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for path in paths {
        if cli.verbose {
            eprintln!("erasing {} ({} passes)", path.display(), cli.passes);
        }
        if let Err(err) = erase::erase_path(path, opts) {
            eprintln!("{NAME}: {}: {err}", path.display());
            failed = true;
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_wipe_free(dir: &Path, cli: &Cli, opts: &erase::Options) -> ExitCode {
    if let Err(err) = safety::validate_wipe_dir(dir) {
        eprintln!("{NAME}: {err}");
        return ExitCode::FAILURE;
    }

    if storage::is_network(dir) {
        eprintln!(
            "{NAME}: {}: refusing to fill a network filesystem, its free space belongs to the server and other users",
            dir.display()
        );
        return ExitCode::FAILURE;
    }

    for warning in storage::warnings(&[dir.to_path_buf()]) {
        eprintln!("{NAME}: warning: {warning}");
    }

    let space = space_lines(dir);

    if cli.dry_run {
        println!(
            "would fill and erase the free space of {} ({} passes)",
            dir.display(),
            cli.passes
        );
        for line in &space {
            println!("  {line}");
        }
        return ExitCode::SUCCESS;
    }

    if !cli.force && !confirm_wipe(dir, cli.passes, &space) {
        eprintln!("aborted, nothing was erased.");
        return ExitCode::FAILURE;
    }

    match wipe::wipe_free_space(dir, opts) {
        Ok(0) => {
            eprintln!(
                "{NAME}: {}: no free space was written, nothing was wiped (the volume is full, or there is no space for this user)",
                dir.display()
            );
            ExitCode::FAILURE
        }
        Ok(written) => {
            eprintln!(
                "free space of {} erased ({} written)",
                dir.display(),
                preview::human_bytes(written)
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{NAME}: {}: {err}", dir.display());
            ExitCode::FAILURE
        }
    }
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

fn confirm(paths: &[PathBuf], summary: &[String], passes: u32) -> bool {
    eprintln!(
        "note: overwriting does not guarantee erasure on ssds, with snapshots, or on copy-on-write filesystems"
    );
    eprintln!("the following will be irreversibly erased ({passes} passes):");
    for path in paths {
        eprintln!("  {}", path.display());
    }
    for line in summary {
        eprintln!("  {line}");
    }
    prompt::ask("continue?")
}

fn confirm_wipe(dir: &Path, passes: u32, space: &[String]) -> bool {
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
