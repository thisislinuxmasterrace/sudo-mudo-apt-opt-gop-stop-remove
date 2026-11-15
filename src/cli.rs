//! command line definition. the doc comments on `Cli` are the `--help` text and the man page.

use std::path::PathBuf;

use clap::{Arg, ArgAction, Command, CommandFactory, FromArgMatches, Parser};

const PREFIX: [&str; 5] = ["apt", "opt", "gop", "stop", "remove"];
const USAGE: &str = "sudo mudo apt opt gop stop remove [options] [paths]...";
const HELP: &str = "sudo mudo apt opt gop stop remove --help";
const TEMPLATE: &str = "{about}\n\nusage: {usage}\n\n{all-args}{after-help}";

const EXAMPLES: &str = "\
examples:
  erase a file, after a question:        sudo mudo apt opt gop stop remove venom.exe
  look first, change nothing:            sudo mudo apt opt gop stop remove --dry-run -r build/
  look first and get advice:             sudo mudo apt opt gop stop remove --explain venom.exe
  a tree, keeping some entries:          sudo mudo apt opt gop stop remove -r --exclude .git build/
  paths from find, in a script:          find . -name '*.tmp' -print0 | sudo mudo apt opt gop stop remove -f -0 --files-from -
  with a record of what was erased:      sudo mudo apt opt gop stop remove -r --log erase.jsonl build/
  the free space of a volume:            sudo mudo apt opt gop stop remove --wipe-free /home

exit codes: 0 done, 1 refused before erasing, 2 bad arguments, 3 declined, 4 not finished, 130 second ctrl+c.";

/// when you really want to nuke the shit out of some files
#[derive(Debug, Parser)]
#[command(
    version,
    about,
    after_help = EXAMPLES,
    override_usage = USAGE,
    help_template = TEMPLATE
)]
pub struct Cli {
    /// files to erase; directories need -r. a linux block device is experimental
    #[arg(
        help_heading = "what to erase",
        required_unless_present_any = ["wipe_free", "files_from"],
        conflicts_with = "wipe_free"
    )]
    pub paths: Vec<PathBuf>,

    /// erase directories and their contents
    #[arg(short, long, help_heading = "what to erase")]
    pub recursive: bool,

    /// skip matching entries inside trees given with -r; may be repeated
    ///
    /// `*` and `?` match within one name and are always wildcards. a pattern without `/` matches a name at
    /// any depth; a pattern with `/` matches the path from the command-line argument (`src/*.o` skips
    /// `build/src/x.o` for `build`). a trailing `/` is ignored. command-line arguments are never excluded.
    /// directories that hold excluded entries are kept, and the run still succeeds.
    #[arg(
        long = "exclude",
        value_name = "GLOB",
        help_heading = "what to erase",
        value_parser = parse_exclude,
        requires = "recursive",
        conflicts_with = "wipe_free"
    )]
    pub exclude: Vec<String>,

    /// read more paths from FILE, one per line; `-` reads stdin
    ///
    /// spaces are part of the path, a `\r` before the line break is dropped and empty lines are skipped.
    #[arg(
        long,
        value_name = "FILE",
        help_heading = "what to erase",
        conflicts_with = "wipe_free"
    )]
    pub files_from: Option<PathBuf>,

    /// with --files-from: paths are separated by nul bytes, as `find -print0` writes them
    #[arg(
        short = '0',
        long,
        help_heading = "what to erase",
        requires = "files_from"
    )]
    pub null: bool,

    /// erase the free space of the filesystem that holds DIR instead of files
    #[arg(long, value_name = "DIR", help_heading = "what to erase")]
    pub wipe_free: Option<PathBuf>,

    /// show what would be erased; change nothing
    #[arg(long, help_heading = "look first")]
    pub dry_run: bool,

    /// --dry-run plus advice: what an overwrite can and cannot reach for these paths
    ///
    /// the advice comes from the storage medium, filesystem, snapshots and copies found nearby. read-only
    /// git and snapshot tools may run; suggested commands are printed, never run.
    #[arg(long, help_heading = "look first", conflicts_with_all = ["wipe_free", "json"])]
    pub explain: bool,

    /// with --dry-run: print the summary as one line of json
    #[arg(
        long,
        help_heading = "look first",
        requires = "dry_run",
        conflicts_with = "wipe_free"
    )]
    pub json: bool,

    /// number of overwrite passes; the last one is random
    #[arg(
        short = 'n',
        long,
        help_heading = "how",
        default_value_t = 1,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub passes: u32,

    /// end with one extra pass of zeros; it does not make erasure more reliable
    #[arg(long, help_heading = "how")]
    pub zero: bool,

    /// read each file back after the last pass; a file that differs is kept
    ///
    /// a match shows what the filesystem returns now, not that old copies are gone. linux drops the file's
    /// cache first. not for devices or --wipe-free.
    #[arg(long, help_heading = "how", conflicts_with = "wipe_free")]
    pub verify: bool,

    /// record every erased object in FILE as json lines; FILE must not exist
    ///
    /// one object per line: time, path, kind, size, passes, result and assessment. no file contents. a
    /// record, not proof of erasure.
    #[arg(
        long,
        value_name = "FILE",
        help_heading = "how",
        conflicts_with = "wipe_free"
    )]
    pub log: Option<PathBuf>,

    /// ask no questions (confirmation, lock holders, reboot); refusals still apply
    #[arg(short, long, help_heading = "safety")]
    pub force: bool,

    /// allow protected paths, hard links under root and some device refusals; always warns
    #[arg(long = "i-know-what-i-am-doing", help_heading = "safety")]
    pub allow_protected: bool,

    /// windows: close the programs that hold a locked file
    #[arg(long, help_heading = "safety")]
    pub kill_holders: bool,

    /// print each pass
    #[arg(short, long, help_heading = "output")]
    pub verbose: bool,

    /// print only warnings, errors and questions
    #[arg(short, long, help_heading = "output")]
    pub quiet: bool,
}

/// the options command with help and version described like the other flags; clap's own are capitalized
/// and sit apart from every heading.
pub fn leaf() -> Command {
    Cli::command()
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help("print help (--help shows more)")
                .help_heading("output"),
        )
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help("print the version")
                .help_heading("output"),
        )
}

/// builds the complete command tree for parsing and completion generation.
pub fn command() -> Command {
    let mut child = leaf().name("remove").display_name("mudo");
    for name in ["stop", "gop", "opt", "apt", "mudo"] {
        child = Command::new(name)
            .disable_help_flag(true)
            .disable_help_subcommand(true)
            .subcommand_required(true)
            .subcommand(child);
    }
    child
}

pub fn parse() -> Cli {
    // the five words are checked here, so a wrong one gets a plain answer instead of clap's errors
    // about the nested subcommands they are built from.
    let given: Vec<_> = std::env::args_os().skip(1).take(PREFIX.len()).collect();
    if given.len() < PREFIX.len() || given.iter().zip(PREFIX).any(|(word, want)| word != want) {
        eprintln!("mudo: the command is `{USAGE}`, all five words in this order. help: `{HELP}`");
        std::process::exit(2);
    }
    let mut matches = command().try_get_matches().unwrap_or_else(|err| exit(err));
    for name in PREFIX {
        matches = matches
            .remove_subcommand()
            .filter(|(found, _)| found == name)
            .map(|(_, matches)| matches)
            .unwrap_or_else(|| unreachable!("required command was validated by clap"));
    }
    Cli::from_arg_matches(&matches).unwrap_or_else(|err| exit(err))
}

/// prints a clap error in the lowercase style of the other messages. help and version go to stdout as usual.
fn exit(err: clap::Error) -> ! {
    if !err.use_stderr() {
        err.exit();
    }
    let text = err
        .render()
        .to_string()
        .replace("Usage:", "usage:")
        .replace(
            "For more information, try '--help'.",
            &format!("help: `{HELP}`"),
        );
    eprint!("{text}");
    std::process::exit(err.exit_code());
}

/// removes a leading `./` and trailing `/` from an `--exclude` pattern, and refuses what cannot match.
fn parse_exclude(raw: &str) -> Result<String, String> {
    if raw.starts_with('/') {
        return Err(format!("{raw}: the pattern must be relative"));
    }
    let mut pattern = raw;
    while let Some(rest) = pattern.strip_prefix("./") {
        pattern = rest;
    }
    let pattern = pattern.trim_end_matches('/');
    if pattern.is_empty() || pattern == "." {
        return Err(format!("{raw}: the pattern is empty"));
    }
    Ok(pattern.to_owned())
}

#[cfg(test)]
mod tests {
    use super::parse_exclude;

    #[test]
    fn leading_dot_slash_and_trailing_slashes_are_removed() {
        assert_eq!(parse_exclude("build/"), Ok("build".to_owned()));
        assert_eq!(parse_exclude("build//"), Ok("build".to_owned()));
        assert_eq!(parse_exclude("./src/*.o"), Ok("src/*.o".to_owned()));
        assert_eq!(parse_exclude("././build/"), Ok("build".to_owned()));
    }

    #[test]
    fn empty_dot_and_absolute_patterns_are_refused() {
        assert!(parse_exclude("").is_err());
        assert!(parse_exclude("./").is_err());
        assert!(parse_exclude(".").is_err());
        assert!(parse_exclude("/").is_err());
        assert!(parse_exclude("/tmp/x").is_err());
    }
}
