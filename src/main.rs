mod cli;
mod confirm;
mod device;
mod erase;
mod json;
mod log;
mod paths;
mod platform;
mod preview;
mod progress;
mod prompt;
mod run;
mod safety;
mod storage;
mod verify;
mod wipe;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use paths::{list_label, normalize, read_paths};
use run::{run_erase, run_wipe_free};

const NAME: &str = env!("CARGO_PKG_NAME");

// exit codes, documented in the README. clap exits with 2 on a bad command line by itself.
const EXIT_INVALID: u8 = 1;
const EXIT_ARGS: u8 = 2;
const EXIT_DECLINED: u8 = 3;
const EXIT_INCOMPLETE: u8 = 4;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if !cfg!(windows) && cli.kill_holders {
        eprintln!("{NAME}: --kill-holders is only supported on windows");
        return ExitCode::from(EXIT_ARGS);
    }

    // the question after the list would read stdin, which the list has already used up.
    if cli.files_from.as_deref() == Some(Path::new("-")) && !cli.force && !cli.dry_run {
        eprintln!(
            "{NAME}: reading the list from stdin needs -f: the answer to the question cannot come from stdin"
        );
        return ExitCode::from(EXIT_ARGS);
    }

    let opts = erase::Options {
        passes: cli.passes,
        verbose: cli.verbose,
        force: cli.force,
        kill_holders: cli.kill_holders,
        allow_protected: cli.allow_protected,
        quiet: cli.quiet,
        verify: cli.verify,
    };

    let mut given = cli.paths.clone();
    if let Some(source) = &cli.files_from {
        match read_paths(source, cli.null) {
            Ok(read) => {
                eprintln!(
                    "note: {} paths read from {}",
                    read.len(),
                    list_label(source)
                );
                given.extend(read);
            }
            Err(err) => {
                eprintln!("{NAME}: {err}");
                return ExitCode::from(EXIT_INVALID);
            }
        }
    }

    // trailing slashes and `.` make `lstat` follow a symlink (`link/` is the target directory),
    // so every path is normalized before it is checked or used.
    // a repeated path is kept once: erasing it twice would fail with NotFound on the second run.
    // `a/../a.txt` is not recognized as a repeat: its second erase fails with NotFound (exit 4).
    // the data is not lost, the first erase already removed it.
    let mut seen = HashSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in &given {
        let path = normalize(path);
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }

    match &cli.wipe_free {
        Some(dir) => run_wipe_free(&normalize(dir), &cli, &opts),
        None if paths.is_empty() => {
            eprintln!("{NAME}: no paths were given");
            ExitCode::from(EXIT_INVALID)
        }
        None => run_erase(&paths, &cli, &opts),
    }
}
