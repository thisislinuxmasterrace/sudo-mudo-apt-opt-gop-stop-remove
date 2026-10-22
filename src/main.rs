mod cli;
mod erase;
mod json;
mod log;
mod platform;
mod preview;
mod progress;
mod prompt;
mod safety;
mod storage;
mod wipe;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use cli::Cli;

const NAME: &str = env!("CARGO_PKG_NAME");

// exit codes, documented in the README. clap exits with 2 on a bad command line by itself.
const EXIT_INVALID: u8 = 1;
const EXIT_ARGS: u8 = 2;
const EXIT_DECLINED: u8 = 3;
const EXIT_INCOMPLETE: u8 = 4;

// a recursive erase of one path above either limit needs the directory name typed instead of "y".
const STRICT_OBJECTS: u64 = 1000;
const STRICT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

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

/// how the source of `--files-from` is named in messages: `-` is stdin.
fn list_label(source: &Path) -> String {
    if source.as_os_str() == "-" {
        "stdin".to_owned()
    } else {
        source.display().to_string()
    }
}

/// reads the paths of `--files-from`: one per line, or one per nul byte with `--null`. entries are
/// taken as they are, so spaces and a `\r` at the end of a line stay in the path. empty entries are skipped.
fn read_paths(source: &Path, null: bool) -> Result<Vec<PathBuf>, String> {
    let label = list_label(source);
    let bytes = if source.as_os_str() == "-" {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes).map(|_| bytes)
    } else {
        fs::read(source)
    }
    .map_err(|err| format!("{label}: could not read the list of paths: {err}"))?;
    let separator = if null { b'\0' } else { b'\n' };
    bytes
        .split(|&byte| byte == separator)
        .filter(|entry| !entry.is_empty())
        .map(|entry| path_from_bytes(entry).map_err(|err| format!("{label}: {err}")))
        .collect()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    std::str::from_utf8(bytes).map(PathBuf::from).map_err(|_| {
        format!(
            "a path is not valid utf-8: {}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// drops `.` components, so `./a` and `a` are the same path. a path made only of `.` stays `.`,
/// while the empty path stays empty: it must be refused, not turned into the current directory.
fn normalize(path: &Path) -> PathBuf {
    let normalized: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() && !path.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    normalized
}

/// a mount point given as a target is refused before anything is erased, so nothing is erased before
/// the refusal. inside a tree the erase skips it instead. unix only: `is_target_boundary` opens the parent
/// directory, which windows does not allow, so on windows it would refuse every target.
#[cfg(unix)]
fn target_is_mount_point(path: &Path) -> bool {
    match preview::is_target_boundary(path) {
        Ok(false) => false,
        Ok(true) => {
            eprintln!(
                "{NAME}: {}: is a mount point or on another filesystem, refusing to erase it",
                path.display()
            );
            true
        }
        Err(err) => {
            eprintln!("{NAME}: {}: {err}", path.display());
            true
        }
    }
}

#[cfg(not(unix))]
fn target_is_mount_point(_path: &Path) -> bool {
    false
}

fn run_erase(paths: &[PathBuf], cli: &Cli, opts: &erase::Options) -> ExitCode {
    // validate every path before touching any file: one bad path aborts the whole run.
    let mut invalid = false;
    for path in paths {
        match safety::validate_target(path, cli.recursive, cli.allow_protected) {
            Ok(protected) => {
                if protected {
                    eprintln!(
                        "{NAME}: warning: {}: protected system path, erasing it because --i-know-what-i-am-doing was given",
                        path.display()
                    );
                }
                if target_is_mount_point(path) {
                    invalid = true;
                }
            }
            Err(err) => {
                eprintln!("{NAME}: {err}");
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
        eprintln!("{NAME}: {err}");
        return ExitCode::from(EXIT_INVALID);
    }

    for warning in storage::warnings(paths) {
        eprintln!("{NAME}: warning: {warning}");
    }

    // counting walks the whole tree, so it only happens when someone is going to read it.
    let counted = if cli.dry_run || !cli.force {
        Some(preview::summarize(paths, cli.recursive, &cli.exclude))
    } else {
        None
    };
    let total = counted.as_deref().map(preview::total);
    let summary = total
        .as_ref()
        .map_or_else(Vec::new, preview::Summary::lines);

    if cli.dry_run {
        if cli.log.is_some() {
            eprintln!("note: --log is not written by --dry-run");
        }
        if let Some(total) = total.as_ref().filter(|_| cli.json) {
            println!("{}", preview::to_json(paths, cli.passes, total));
            return ExitCode::SUCCESS;
        }
        for path in paths {
            println!("would erase {} ({} passes)", path.display(), cli.passes);
        }
        for line in &summary {
            println!("  {line}");
        }
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "note: overwriting does not guarantee erasure on ssds, with snapshots, or on copy-on-write filesystems"
    );
    if erase::running_as_root()
        && !cli.allow_protected
        && total.as_ref().is_some_and(|total| total.hard_linked > 0)
    {
        eprintln!(
            "note: under root, hard-linked files are refused: use --i-know-what-i-am-doing to erase them"
        );
    }
    let strict = match &counted {
        Some(parts) if cli.recursive => strict_paths(paths, parts),
        _ => Vec::new(),
    };
    if !cli.force && !confirm(paths, &summary, cli.passes, &strict) {
        eprintln!("aborted, nothing was erased.");
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
    for (index, path) in paths.iter().enumerate() {
        if log_file.as_ref().is_some_and(|log| log.failed()) {
            eprintln!("{NAME}: the log could not be written: the remaining paths were not erased");
            break;
        }
        if erase::interrupted() {
            if index == 0 {
                eprintln!("aborted, nothing was erased.");
                return ExitCode::from(EXIT_DECLINED);
            }
            eprintln!(
                "interrupted: {}: not erased, the remaining paths were skipped",
                path.display()
            );
            return ExitCode::from(EXIT_INCOMPLETE);
        }
        if cli.verbose {
            eprintln!("erasing {} ({} passes)", path.display(), cli.passes);
        }
        if let Err(err) = erase::erase_path(path, opts, &cli.exclude, log_file.as_ref()) {
            if err.kind() == std::io::ErrorKind::Interrupted {
                if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
                    eprintln!(
                        "interrupted: {}: an entry may be partly overwritten, the rest was kept",
                        path.display()
                    );
                } else {
                    eprintln!("interrupted: {}: {err}", path.display());
                }
                return ExitCode::from(EXIT_INCOMPLETE);
            }
            eprintln!("{NAME}: {}: {err}", path.display());
            failed = true;
        }
    }

    // a line that could not be written leaves its object erased without a line: that is not a success.
    if failed || log_file.as_ref().is_some_and(|log| log.failed()) {
        ExitCode::from(EXIT_INCOMPLETE)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_wipe_free(dir: &Path, cli: &Cli, opts: &erase::Options) -> ExitCode {
    match safety::validate_wipe_dir(dir, cli.allow_protected) {
        Ok(true) => eprintln!(
            "{NAME}: warning: {}: protected system path, filling it because --i-know-what-i-am-doing was given",
            dir.display()
        ),
        Ok(false) => {}
        Err(err) => {
            eprintln!("{NAME}: {err}");
            return ExitCode::from(EXIT_INVALID);
        }
    }

    if storage::is_network(dir) {
        eprintln!(
            "{NAME}: {}: refusing to fill a network filesystem, its free space belongs to the server and other users",
            dir.display()
        );
        return ExitCode::from(EXIT_INVALID);
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
        return ExitCode::from(EXIT_DECLINED);
    }

    let _interrupts = match start_interrupt_guard() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    if erase::interrupted() {
        eprintln!("aborted, nothing was erased.");
        return ExitCode::from(EXIT_DECLINED);
    }

    match wipe::wipe_free_space(dir, opts) {
        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
            eprintln!(
                "interrupted: {}: free space was partly wiped, the temporary files are removed",
                dir.display()
            );
            ExitCode::from(EXIT_INCOMPLETE)
        }
        Ok(0) => {
            eprintln!(
                "{NAME}: {}: no free space was written, nothing was wiped (the volume is full, or there is no space for this user)",
                dir.display()
            );
            ExitCode::from(EXIT_INCOMPLETE)
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
            ExitCode::from(EXIT_INCOMPLETE)
        }
    }
}

/// routes ctrl+c and sigterm to the interrupt flag for the erase that follows the confirmation.
fn start_interrupt_guard() -> Result<erase::InterruptGuard, ExitCode> {
    erase::InterruptGuard::install().map_err(|err| {
        eprintln!("{NAME}: could not install the interrupt handler: {err}");
        ExitCode::from(EXIT_INVALID)
    })
}

/// creates the `--log` file once the erase is confirmed. the note comes first: the log holds names.
/// the location is checked again once the file exists; if the check fails, the new file is removed.
fn open_log(path: &Path, passes: u32, targets: &[PathBuf]) -> Result<log::Log, ExitCode> {
    if cfg!(unix) {
        eprintln!("note: the log contains file names; it is created with mode 0600");
    }
    let created = log::Log::create(path, passes).map_err(|err| {
        eprintln!(
            "{NAME}: {}: could not create the log: {err}",
            path.display()
        );
        ExitCode::from(EXIT_INVALID)
    })?;
    if let Err(err) = log::check_location(path, targets) {
        drop(created);
        if std::fs::remove_file(path).is_err() {
            eprintln!("{NAME}: {}: could not remove the log", path.display());
        }
        eprintln!("{NAME}: {err}");
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

/// true when a recursive erase of one path is large enough to need its name typed instead of "y".
fn needs_strict_confirmation(objects: u64, bytes: u64) -> bool {
    objects > STRICT_OBJECTS || bytes > STRICT_BYTES
}

/// the paths whose names are typed before a recursive erase. a path above the limits by itself asks
/// for its own name. when only the total is above them, the path with the most objects asks instead.
fn strict_paths<'a>(
    paths: &'a [PathBuf],
    parts: &'a [preview::Summary],
) -> Vec<(&'a PathBuf, &'a preview::Summary)> {
    let own: Vec<_> = paths
        .iter()
        .zip(parts)
        .filter(|(_, part)| needs_strict_confirmation(part.objects(), part.bytes))
        .collect();
    if !own.is_empty() {
        return own;
    }
    let total = preview::total(parts);
    if needs_strict_confirmation(total.objects(), total.bytes) {
        paths
            .iter()
            .zip(parts)
            .max_by_key(|(_, part)| part.objects())
            .into_iter()
            .collect()
    } else {
        Vec::new()
    }
}

/// with no strict path the run asks "continue?". with strict paths the names come first, and
/// "continue?" follows when some path was not named, so the whole run is confirmed.
fn confirm(
    paths: &[PathBuf],
    summary: &[String],
    passes: u32,
    strict: &[(&PathBuf, &preview::Summary)],
) -> bool {
    eprintln!("the following will be irreversibly erased ({passes} passes):");
    for path in paths {
        eprintln!("  {}", path.display());
    }
    for line in summary {
        eprintln!("  {line}");
    }
    if strict.is_empty() {
        return prompt::ask("continue?");
    }
    strict.iter().all(|(path, part)| confirm_name(path, part))
        && (strict.len() == paths.len() || prompt::ask("continue?"))
}

/// a large path is confirmed by typing its last path component, exactly as it is named.
fn confirm_name(path: &Path, part: &preview::Summary) -> bool {
    let name = path.file_name().unwrap_or(path.as_os_str());
    let kind = if part.dirs > 0 { "directory" } else { "file" };
    let question = format!(
        "this is a large erase ({} objects, {}): type the name of the {kind} ({}) to continue:",
        part.objects(),
        preview::human_bytes(part.bytes),
        name.to_string_lossy()
    );
    prompt::read_line(&format!("{question} "))
        .as_deref()
        .map(OsStr::new)
        == Some(name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_dot_components() {
        assert_eq!(
            normalize(Path::new("./a.txt")),
            normalize(Path::new("a.txt"))
        );
        assert_eq!(
            normalize(Path::new("dir/./a.txt")),
            PathBuf::from("dir/a.txt")
        );
        assert_eq!(normalize(Path::new("./")), PathBuf::from("."));
        assert_eq!(normalize(Path::new(".")), PathBuf::from("."));
    }

    #[test]
    fn normalize_keeps_empty_path_empty() {
        assert_eq!(normalize(Path::new("")), PathBuf::new());
    }

    #[test]
    fn strict_confirmation_starts_above_a_thousand_objects() {
        assert!(!needs_strict_confirmation(1000, 0));
        assert!(needs_strict_confirmation(1001, 0));
    }

    #[test]
    fn strict_confirmation_starts_above_ten_gib() {
        const GIB: u64 = 1024 * 1024 * 1024;

        assert!(!needs_strict_confirmation(0, 10 * GIB));
        assert!(needs_strict_confirmation(0, 10 * GIB + 1));
    }
}
