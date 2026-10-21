mod cli;
mod erase;
mod platform;
mod prompt;
mod safety;
mod storage;

use std::path::PathBuf;
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

    // validate every path before touching any file: one bad path aborts the whole run.
    let mut invalid = false;
    for path in &cli.paths {
        if let Err(err) = safety::validate_target(path, cli.recursive) {
            eprintln!("{NAME}: {err}");
            invalid = true;
        }
    }
    if invalid {
        return ExitCode::FAILURE;
    }

    for warning in storage::copy_on_write_warnings(&cli.paths) {
        eprintln!("{NAME}: warning: {warning}");
    }

    if cli.dry_run {
        for path in &cli.paths {
            println!("would erase {} ({} passes)", path.display(), cli.passes);
        }
        return ExitCode::SUCCESS;
    }

    if !cli.force && !confirm(&cli.paths, cli.passes) {
        eprintln!("aborted, nothing was erased.");
        return ExitCode::FAILURE;
    }

    let opts = erase::Options {
        passes: cli.passes,
        verbose: cli.verbose,
        force: cli.force,
        kill_holders: cli.kill_holders,
        raw_volume: cli.raw_volume,
    };

    let mut failed = false;
    for path in &cli.paths {
        if cli.verbose {
            eprintln!("erasing {} ({} passes)", path.display(), cli.passes);
        }
        if let Err(err) = erase::erase_path(path, &opts) {
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

fn confirm(paths: &[PathBuf], passes: u32) -> bool {
    eprintln!(
        "note: overwriting does not guarantee erasure on ssds, with snapshots, or on copy-on-write filesystems"
    );
    eprintln!("the following files will be irreversibly erased ({passes} passes):");
    for path in paths {
        eprintln!("  {}", path.display());
    }
    prompt::ask("continue?")
}
