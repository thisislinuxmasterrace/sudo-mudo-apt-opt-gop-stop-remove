use std::path::PathBuf;

use clap::Parser;

/// irreversibly erase files: overwrite their contents, then delete them.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// files to erase, in addition to the ones read with --files-from
    #[arg(
        required_unless_present_any = ["wipe_free", "files_from"],
        conflicts_with = "wipe_free"
    )]
    pub paths: Vec<PathBuf>,

    /// read more paths to erase from FILE, one per line; `-` reads stdin
    ///
    /// a line is taken as it is: spaces are part of the path, and a `\r` at the end of a line is kept.
    /// empty lines are skipped. the paths are added to the ones given on the command line.
    #[arg(long, value_name = "FILE", conflicts_with = "wipe_free")]
    pub files_from: Option<PathBuf>,

    /// with --files-from: paths are separated by nul bytes, as `find -print0` writes them
    #[arg(short = '0', long, requires = "files_from")]
    pub null: bool,

    /// erase the free space of the filesystem that holds this directory
    #[arg(long, value_name = "DIR")]
    pub wipe_free: Option<PathBuf>,

    /// erase directories and their contents
    #[arg(short, long)]
    pub recursive: bool,

    /// skip matching entries inside trees given with -r; may be repeated
    ///
    /// a pattern without `/` is matched with the name of an entry at any depth, files and directories
    /// alike. a pattern with `/` is matched with the path of the entry relative to its command-line
    /// argument: `src/*.o` skips `build/src/x.o` when `build` is the argument. a leading `./` and a
    /// trailing `/` are removed, so `build/` means `build`: an entry with that name at any depth.
    /// a pattern must not be empty or absolute. `*` matches any characters except `/`, `?` matches one
    /// character except `/`. they are always wildcards, so a name that contains them cannot be matched
    /// literally. command-line arguments are never excluded, even when they match. an excluded entry is
    /// not erased, and the directories that hold it are kept; the run still succeeds and reports them.
    #[arg(
        long = "exclude",
        value_name = "GLOB",
        value_parser = parse_exclude,
        requires = "recursive",
        conflicts_with = "wipe_free"
    )]
    pub exclude: Vec<String>,

    /// number of overwrite passes
    #[arg(
        short = 'n',
        long,
        default_value_t = 1,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub passes: u32,

    /// only show what would be erased, change nothing
    #[arg(long)]
    pub dry_run: bool,

    /// with --dry-run: print the summary as one line of json on stdout
    #[arg(long, requires = "dry_run", conflicts_with = "wipe_free")]
    pub json: bool,

    /// write a json lines log of every object erased; the file must not exist and is never overwritten
    ///
    /// one object per line, with its time (utc), path, kind, size, passes, result and detail. file contents
    /// are not logged. not written by --dry-run, and not allowed with --wipe-free.
    #[arg(long, value_name = "FILE", conflicts_with = "wipe_free")]
    pub log: Option<PathBuf>,

    /// do not ask any questions (confirmation, lock holders, reboot)
    #[arg(short, long)]
    pub force: bool,

    /// print progress for each pass
    #[arg(short, long)]
    pub verbose: bool,

    /// do not print the progress line (it is shown on a terminal only); warnings and errors are still printed
    #[arg(short, long)]
    pub quiet: bool,

    /// close processes that hold a locked file (windows only)
    #[arg(long)]
    pub kill_holders: bool,

    /// allow erasing protected system paths; a warning is printed every time. -f does not imply it
    #[arg(long = "i-know-what-i-am-doing")]
    pub allow_protected: bool,
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
