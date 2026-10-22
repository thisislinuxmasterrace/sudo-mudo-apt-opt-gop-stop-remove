//! writes the man page or shell completions for the cli defined in `src/cli.rs`.
//!
//! cargo run --example gen_docs -- man <dir>
//! cargo run --example gen_docs -- completions <bash|zsh|fish|powershell> <dir>

#[allow(dead_code)]
#[path = "../src/cli.rs"]
mod cli;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::Shell;

const BIN: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    let outcome = match args.as_slice() {
        [cmd, dir] if cmd.to_str() == Some("man") => write_man(Path::new(dir)),
        [cmd, shell, dir] if cmd.to_str() == Some("completions") => match parse_shell(shell) {
            Some(shell) => write_completions(shell, Path::new(dir)),
            None => return usage(&format!("unknown shell `{}`", shell.to_string_lossy())),
        },
        _ => return usage("expected a subcommand: man or completions"),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("gen_docs: {err}");
            ExitCode::FAILURE
        }
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("gen_docs: {problem}");
    eprintln!("usage: cargo run --example gen_docs -- man <dir>");
    eprintln!(
        "       cargo run --example gen_docs -- completions <bash|zsh|fish|powershell> <dir>"
    );
    ExitCode::from(2)
}

fn parse_shell(name: &OsStr) -> Option<Shell> {
    match name.to_str()? {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "powershell" => Some(Shell::PowerShell),
        _ => None,
    }
}

fn create_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
        .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", dir.display())))
}

fn write_man(dir: &Path) -> io::Result<()> {
    create_dir(dir)?;
    let mut page = Vec::new();
    clap_mangen::Man::new(cli::Cli::command()).render(&mut page)?;
    let path = dir.join(format!("{BIN}.1"));
    fs::write(&path, page)?;
    println!("{}", path.display());
    Ok(())
}

fn write_completions(shell: Shell, dir: &Path) -> io::Result<()> {
    create_dir(dir)?;
    let mut cmd = cli::Cli::command();
    let path = clap_complete::generate_to(shell, &mut cmd, BIN, dir)?;
    println!("{}", path.display());
    Ok(())
}
