use std::path::PathBuf;

use clap::Parser;

/// irreversibly erase files: overwrite their contents, then delete them.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// files to erase
    #[arg(required_unless_present = "wipe_free", conflicts_with = "wipe_free")]
    pub paths: Vec<PathBuf>,

    /// erase the free space of the filesystem that holds this directory
    #[arg(long, value_name = "DIR")]
    pub wipe_free: Option<PathBuf>,

    /// erase directories and their contents
    #[arg(short, long)]
    pub recursive: bool,

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

    /// do not ask any questions (confirmation, lock holders, raw volume, reboot)
    #[arg(short, long)]
    pub force: bool,

    /// print progress for each pass
    #[arg(short, long)]
    pub verbose: bool,

    /// close processes that hold a locked file (windows only)
    #[arg(long)]
    pub kill_holders: bool,

    /// overwrite a locked file through the raw volume, needs administrator (windows only)
    #[arg(long)]
    pub raw_volume: bool,
}
