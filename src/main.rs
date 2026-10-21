mod cli;
mod erase;
mod safety;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use cli::Cli;

const NAME: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let cli = Cli::parse();

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

    let mut failed = false;
    for path in &cli.paths {
        if cli.verbose {
            eprintln!("erasing {} ({} passes)", path.display(), cli.passes);
        }
        if let Err(err) = erase::erase_path(path, cli.passes, cli.verbose) {
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

/// asks the user to confirm. anything other than "y" or "yes" (including EOF) means no.
fn confirm(paths: &[PathBuf], passes: u32) -> bool {
    eprintln!("the following files will be irreversibly erased ({passes} passes):");
    for path in paths {
        eprintln!("  {}", path.display());
    }
    eprint!("continue? [y/N] ");

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
