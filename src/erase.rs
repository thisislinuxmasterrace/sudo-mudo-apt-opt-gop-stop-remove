use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::ffi::{CStr, CString, OsStr};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(unix)]
use nix::dir::Dir;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
#[cfg(unix)]
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
#[cfg(unix)]
use nix::sys::stat::{FchmodatFlags, FileStat, Mode, SFlag, fchmodat, fstat, fstatat};
#[cfg(unix)]
use nix::unistd::{UnlinkatFlags, geteuid, unlinkat};
use rand::RngCore;

use crate::log::{Kind, Log, Outcome};
use crate::progress::Progress;

#[cfg(windows)]
use crate::platform::windows;
#[cfg(windows)]
use crate::prompt;

/// size of the buffer used for each write, in bytes.
const BUFFER_SIZE: usize = 1024 * 1024;

/// settings for one erase run.
pub struct Options {
    pub passes: u32,
    pub verbose: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub force: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub kill_holders: bool,
    /// lifts the refusal of hard-linked files under root, the same flag that lifts the protected-path refusal.
    pub allow_protected: bool,
    /// no progress line on stderr. warnings and errors are printed anyway.
    pub quiet: bool,
}

/// where an entry sits in a walk: `rel` is its path from the command-line argument (empty for the
/// argument itself), `excludes` are the `--exclude` patterns, and `log` is the `--log` file, if any.
#[derive(Clone, Copy)]
struct Walk<'a> {
    rel: &'a Path,
    excludes: &'a [String],
    log: Option<&'a Log>,
}

/// set by the interrupt handlers on the first ctrl+c or sigterm while an erase runs.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// records an interrupt and returns true when one was already recorded. it runs inside signal and
/// console handlers, so it only does an atomic swap.
pub(crate) fn raise_interrupt() -> bool {
    record_interrupt(&INTERRUPTED)
}

/// sets `flag` and returns true when it was already set, which makes the caller's signal the second one.
fn record_interrupt(flag: &AtomicBool) -> bool {
    flag.swap(true, Ordering::SeqCst)
}

pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// returned by the write loops once an interrupt was recorded. it is passed up unchanged, so the
/// walk stops and the partly written file keeps its name.
pub(crate) fn check_interrupted() -> io::Result<()> {
    if interrupted() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "interrupted by signal",
        ));
    }
    Ok(())
}

/// routes ctrl+c and sigterm to the interrupt flag until the guard is dropped, so the handlers only
/// cover the erase itself and not the questions before it.
#[cfg(unix)]
pub struct InterruptGuard {
    previous: Vec<(Signal, SigAction)>,
}

#[cfg(unix)]
impl InterruptGuard {
    pub fn install() -> io::Result<Self> {
        let action = SigAction::new(
            SigHandler::Handler(on_unix_interrupt),
            SaFlags::empty(),
            SigSet::empty(),
        );
        let mut guard = Self {
            previous: Vec::new(),
        };
        for signal in [Signal::SIGINT, Signal::SIGTERM] {
            // a signal the process was started with ignored stays ignored.
            if ignores(signal)? {
                continue;
            }
            // safety: on_unix_interrupt only does an atomic swap and calls _exit, both allowed in a signal handler.
            let old = unsafe { sigaction(signal, &action) }.map_err(io::Error::from)?;
            guard.previous.push((signal, old));
        }
        Ok(guard)
    }
}

/// true when `signal` is currently ignored. a null action only reads the disposition, so nothing changes.
#[cfg(unix)]
fn ignores(signal: Signal) -> io::Result<bool> {
    let mut current = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // safety: with a null new action, sigaction only writes the current action into `current`.
    let ret = unsafe {
        libc::sigaction(
            signal as libc::c_int,
            std::ptr::null(),
            current.as_mut_ptr(),
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    // safety: sigaction returned 0, so it has written the whole struct.
    let current = unsafe { current.assume_init() };
    Ok(current.sa_sigaction == libc::SIG_IGN)
}

#[cfg(unix)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        for (signal, action) in &self.previous {
            // safety: the action was returned by sigaction for this signal, so restoring it is valid.
            let _ = unsafe { sigaction(*signal, action) };
        }
    }
}

#[cfg(unix)]
extern "C" fn on_unix_interrupt(_signal: libc::c_int) {
    if raise_interrupt() {
        // safety: _exit is async-signal-safe and does not return.
        unsafe { libc::_exit(130) }
    }
}

/// console ctrl+c, ctrl+break and closing the console set the interrupt flag while the guard lives.
#[cfg(windows)]
pub struct InterruptGuard;

#[cfg(windows)]
impl InterruptGuard {
    pub fn install() -> io::Result<Self> {
        windows::set_interrupt_handler(true)?;
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        let _ = windows::set_interrupt_handler(false);
    }
}

/// what erase does with a file that has more than one hard link.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HardLinkRule {
    Erase,
    EraseWithWarning,
    Refuse,
}

/// under root, other names may point to the same data outside the erased tree, so a file with
/// several links is refused unless `allow` is set. the rule covers root only: an ordinary user
/// links to files it may already change.
pub(crate) fn hard_link_rule(is_root: bool, hard_linked: bool, allow: bool) -> HardLinkRule {
    match (is_root && hard_linked, allow) {
        (false, _) => HardLinkRule::Erase,
        (true, true) => HardLinkRule::EraseWithWarning,
        (true, false) => HardLinkRule::Refuse,
    }
}

/// applies `hard_link_rule` before the file is touched. `shown` is the path reported to the user.
fn check_hard_links(shown: &Path, hard_linked: bool, opts: &Options) -> io::Result<()> {
    match hard_link_rule(running_as_root(), hard_linked, opts.allow_protected) {
        HardLinkRule::Erase => Ok(()),
        HardLinkRule::EraseWithWarning => {
            eprintln!(
                "{}: warning: hard-linked file under root, erasing it because --i-know-what-i-am-doing was given",
                shown.display()
            );
            Ok(())
        }
        HardLinkRule::Refuse => Err(io::Error::other(
            "hard-linked file under root: other names may point to the same data, use --i-know-what-i-am-doing to erase it",
        )),
    }
}

/// writes one line to the log, when there is one. `passes` is recorded for regular files only.
fn log_object(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    outcome: Outcome,
    detail: &str,
) {
    if let Some(log) = log {
        log.record(shown, kind, size, outcome, detail);
    }
}

/// writes the line of an object whose erase returned `result`. `refused` marks an object the erase
/// refused before touching it; `warning` is the note for an erase that went ahead anyway.
fn log_done(
    log: Option<&Log>,
    shown: &Path,
    kind: Kind,
    size: u64,
    result: &io::Result<bool>,
    refused: bool,
    warning: &str,
) {
    match result {
        Ok(false) => log_object(log, shown, kind, size, Outcome::Ok, warning),
        Ok(true) => log_object(
            log,
            shown,
            kind,
            size,
            Outcome::Kept,
            "kept: an entry inside is excluded by --exclude",
        ),
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            if let Some(log) = log {
                log.record_interrupted(shown, kind, size);
            }
        }
        Err(err) => {
            let outcome = if refused {
                Outcome::Refused
            } else {
                Outcome::Error
            };
            log_object(log, shown, kind, size, outcome, &err.to_string());
        }
    }
}

/// writes the line of an entry that could not be inspected, so its kind is not known.
fn unseen(log: Option<&Log>, shown: &Path, err: io::Error) -> io::Error {
    log_object(log, shown, Kind::Other, 0, Outcome::Error, &err.to_string());
    err
}

/// refuses to erase anything once a log line could not be written: nothing is erased without its log.
fn log_stopped(log: Option<&Log>) -> io::Result<()> {
    match log {
        Some(log) if log.failed() => Err(io::Error::other(
            "the log could not be written, erasing stopped",
        )),
        _ => Ok(()),
    }
}

#[cfg(unix)]
pub(crate) fn running_as_root() -> bool {
    geteuid().is_root()
}

#[cfg(not(unix))]
pub(crate) fn running_as_root() -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pattern {
    Zeros,
    Ones,
    Random,
}

impl Pattern {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Pattern::Zeros => "zeros",
            Pattern::Ones => "ones",
            Pattern::Random => "random",
        }
    }
}

/// zeros, then ones, then random data, repeated. the last pass is always random,
/// so the final content is not a recognizable fixed pattern.
pub(crate) fn pattern_for_pass(pass: u32, passes: u32) -> Pattern {
    if pass + 1 == passes {
        return Pattern::Random;
    }
    match pass % 3 {
        0 => Pattern::Zeros,
        1 => Pattern::Ones,
        _ => Pattern::Random,
    }
}

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
///
/// entries matching `excludes` are kept on purpose: their directories are kept too, and the result is
/// `Ok(true)` (something was kept because of `--exclude`), which is not a failure.
#[cfg(unix)]
pub fn erase_path(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
) -> io::Result<bool> {
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to erase '.', '..' or a filesystem root",
        ));
    };
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    // the parent is named by the user, so links in it are followed. the entry is looked up
    // only relative to open directory descriptors, so no later lookup can follow a link.
    let dir = File::open(parent).map_err(|err| unseen(log, path, err))?;
    let entry =
        CString::new(name.as_bytes()).map_err(|err| unseen(log, path, io::Error::other(err)))?;
    let st = fstatat(&dir, entry.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|err| unseen(log, path, io::Error::from(err)))?;
    let real = fs::canonicalize(parent)
        .map_err(|err| unseen(log, path, err))?
        .join(name);
    // a mount point cannot be renamed away, so it is refused before anything inside it is erased.
    let boundary = fstat(&dir)
        .map_err(io::Error::from)
        .and_then(|parent_st| is_boundary(&st, &parent_st, &real))
        .map_err(|err| unseen(log, path, err))?;
    if boundary {
        let err =
            io::Error::other("is a mount point or on another filesystem, refusing to erase it");
        let (kind, size) = stat_kind(&st);
        log_object(log, path, kind, size, Outcome::Refused, &err.to_string());
        return Err(err);
    }
    let walk = Walk {
        rel: Path::new(""),
        excludes,
        log,
    };
    erase_object(&dir, entry.as_c_str(), &st, path, &real, walk, opts)
}

/// erases one entry and writes its line to the log. `st` is its stat, taken without following links.
#[cfg(unix)]
fn erase_object(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    log_stopped(walk.log)?;
    let (kind, size) = stat_kind(st);
    let rule = hard_link_rule(running_as_root(), st.st_nlink > 1, opts.allow_protected);
    let refused = kind == Kind::Other || (kind == Kind::File && rule == HardLinkRule::Refuse);
    let warning = if kind == Kind::File && rule == HardLinkRule::EraseWithWarning {
        "hard-linked file under root, erased because --i-know-what-i-am-doing was given"
    } else {
        ""
    };
    // checked before each object, so an object the signal reaches is not touched.
    let result =
        check_interrupted().and_then(|()| erase_entry(parent, name, st, shown, real, walk, opts));
    log_done(walk.log, shown, kind, size, &result, refused, warning);
    result
}

/// kind of an entry from its stat, and its size: only regular files have one.
#[cfg(unix)]
fn stat_kind(st: &FileStat) -> (Kind, u64) {
    let kind = file_kind(st);
    if kind == SFlag::S_IFREG {
        (Kind::File, u64::try_from(st.st_size).unwrap_or(0))
    } else if kind == SFlag::S_IFDIR {
        (Kind::Dir, 0)
    } else if kind == SFlag::S_IFLNK {
        (Kind::Link, 0)
    } else {
        (Kind::Other, 0)
    }
}

/// writes a skipped line for an entry that is not erased. the entry is only stat'ed for the log.
#[cfg(unix)]
fn log_skipped_at(log: Option<&Log>, dir: &File, name: &CStr, shown: &Path, detail: &str) {
    if log.is_none() {
        return;
    }
    let (kind, size) = fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_or((Kind::Other, 0), |st| stat_kind(&st));
    log_object(log, shown, kind, size, Outcome::Skipped, detail);
}

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
#[cfg(windows)]
pub fn erase_path(
    path: &Path,
    opts: &Options,
    excludes: &[String],
    log: Option<&Log>,
) -> io::Result<bool> {
    erase_within(
        path,
        Walk {
            rel: Path::new(""),
            excludes,
            log,
        },
        opts,
    )
}

#[cfg(windows)]
fn erase_within(path: &Path, walk: Walk<'_>, opts: &Options) -> io::Result<bool> {
    log_stopped(walk.log)?;
    let meta = fs::symlink_metadata(path).map_err(|err| unseen(walk.log, path, err))?;
    let (kind, size) = meta_kind(&meta);
    let result = check_interrupted().and_then(|()| erase_kind(path, &meta, walk, opts));
    log_done(walk.log, path, kind, size, &result, kind == Kind::Other, "");
    result
}

#[cfg(windows)]
fn erase_kind(
    path: &Path,
    meta: &fs::Metadata,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let file_type = meta.file_type();

    if file_type.is_symlink() {
        return fs::remove_file(path)
            .or_else(|file_err| fs::remove_dir(path).map_err(|_| file_err))
            .map(|()| false);
    }
    if file_type.is_dir() {
        return erase_dir(path, walk, opts);
    }
    if file_type.is_file() {
        return erase_file(path, opts).map(|()| false);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not a regular file, directory or symbolic link",
    ))
}

/// kind of an entry from its metadata, and its size: only regular files have one.
#[cfg(windows)]
fn meta_kind(meta: &fs::Metadata) -> (Kind, u64) {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        (Kind::Link, 0)
    } else if file_type.is_dir() {
        (Kind::Dir, 0)
    } else if file_type.is_file() {
        (Kind::File, meta.len())
    } else {
        (Kind::Other, 0)
    }
}

/// writes a skipped line for an entry that is not erased. the entry is only stat'ed for the log.
#[cfg(windows)]
fn log_skipped_path(log: Option<&Log>, path: &Path, detail: &str) {
    if log.is_none() {
        return;
    }
    let (kind, size) = fs::symlink_metadata(path).map_or((Kind::Other, 0), |meta| meta_kind(&meta));
    log_object(log, path, kind, size, Outcome::Skipped, detail);
}

#[cfg(windows)]
fn erase_dir(path: &Path, walk: Walk<'_>, opts: &Options) -> io::Result<bool> {
    make_writable(path, true)?;

    // collect entries first so the directory is not modified while it is being iterated.
    let entries: Vec<fs::DirEntry> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
    // never erase what is mounted inside the tree: that is another disk or the source of a bind mount.
    let parent_dev = device(&fs::symlink_metadata(path)?);
    let mut failed = 0usize;
    let mut excluded = 0usize;
    let mut kept_inside = false;
    for entry in &entries {
        let entry_path = entry.path();
        let entry_rel = walk.rel.join(entry.file_name());
        // excluded entries are not walked, so an excluded directory is not walked either.
        if is_excluded(walk.excludes, &entry_rel) {
            excluded += 1;
            log_skipped_path(walk.log, &entry_path, "excluded by --exclude");
            continue;
        }
        let child = Walk {
            rel: &entry_rel,
            excludes: walk.excludes,
            log: walk.log,
        };
        let result = match is_mount_boundary(&entry_path, parent_dev) {
            Ok(true) => {
                log_skipped_path(
                    walk.log,
                    &entry_path,
                    "mount point or other filesystem, skipped",
                );
                Err(io::Error::other(
                    "is a mount point or on another filesystem, skipped",
                ))
            }
            Ok(false) => erase_within(&entry_path, child, opts),
            Err(err) => Err(unseen(walk.log, &entry_path, err)),
        };
        match result {
            Ok(inside_kept) => kept_inside |= inside_kept,
            Err(err) => {
                if err.kind() == io::ErrorKind::Interrupted {
                    if !entry_path.is_dir() {
                        eprintln!("{}: {err}", entry_path.display());
                    }
                    return Err(err);
                }
                eprintln!("{}: {err}", entry_path.display());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            path.display()
        )));
    }
    if excluded > 0 {
        eprintln!(
            "kept {}: {excluded} entries excluded by --exclude",
            path.display()
        );
    }
    if excluded > 0 || kept_inside {
        return Ok(true);
    }

    if opts.verbose {
        eprintln!("  removing directory {}", path.display());
    }
    unlink_with_random_name(path, true).map(|()| false)
}

/// filesystem id of an entry.
#[cfg(unix)]
pub(crate) fn device(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    meta.dev()
}

// windows mount points and junctions are reparse points, which std reports as symlinks:
// they are removed as links and never descended into.
#[cfg(not(unix))]
pub(crate) fn device(_meta: &fs::Metadata) -> u64 {
    0
}

/// number of hard links to the entry's data. std does not expose it on windows yet, so 1 there.
#[cfg(unix)]
pub(crate) fn link_count(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    meta.nlink()
}

#[cfg(not(unix))]
pub(crate) fn link_count(_meta: &fs::Metadata) -> u64 {
    1
}

/// true when `entry` lives on another filesystem than its parent, or is a mount point itself.
/// symlinks are never followed here, so they are never reported.
pub(crate) fn is_mount_boundary(entry: &Path, parent_dev: u64) -> io::Result<bool> {
    let meta = fs::symlink_metadata(entry)?;
    if meta.file_type().is_symlink() {
        return Ok(false);
    }
    if device(&meta) != parent_dev {
        return Ok(true);
    }
    // bind mounts share the device id of their parent, so the mount table is needed as well.
    Ok(mount_points()?.contains(&fs::canonicalize(entry)?))
}

/// mount points of the running system, read once. on linux they come from
/// `/proc/self/mountinfo`, on macos from `getmntinfo`; elsewhere only the device check applies.
#[cfg(target_os = "linux")]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static MOUNTS: OnceLock<Result<HashSet<PathBuf>, String>> = OnceLock::new();
    MOUNTS
        .get_or_init(|| {
            fs::read_to_string("/proc/self/mountinfo")
                .map(|table| {
                    table
                        .lines()
                        // field 5 is the mount point
                        .filter_map(|line| line.split(' ').nth(4))
                        .map(|escaped| PathBuf::from(unescape_mountinfo(escaped)))
                        .collect()
                })
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| io::Error::other(format!("could not read the mount table: {err}")))
}

#[cfg(target_os = "macos")]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static MOUNTS: OnceLock<Result<HashSet<PathBuf>, String>> = OnceLock::new();
    MOUNTS
        .get_or_init(|| {
            macos_mount_points()
                .map(|points| points.into_iter().collect())
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| io::Error::other(format!("could not read the mount table: {err}")))
}

/// mount points from `getmntinfo(3)`. the array lives in libc's memory and is only read here.
#[cfg(target_os = "macos")]
fn macos_mount_points() -> io::Result<Vec<PathBuf>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    let mut buf: *mut libc::statfs = ptr::null_mut();
    // safety: buf is a valid out pointer. mnt_nowait keeps the call from contacting network
    // filesystems.
    let count = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if count <= 0 || buf.is_null() {
        return Err(io::Error::other("getmntinfo failed"));
    }
    // safety: getmntinfo returned `count` entries at `buf`, valid until the next call; nothing
    // else in this process calls it while this slice is alive.
    let entries = unsafe { std::slice::from_raw_parts(buf, count as usize) };
    Ok(entries
        .iter()
        .map(|entry| {
            // safety: f_mntonname is a NUL-terminated array inside the entry.
            let name = unsafe { CStr::from_ptr(entry.f_mntonname.as_ptr()) };
            PathBuf::from(OsStr::from_bytes(name.to_bytes()))
        })
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn mount_points() -> io::Result<&'static HashSet<PathBuf>> {
    static NONE: OnceLock<HashSet<PathBuf>> = OnceLock::new();
    Ok(NONE.get_or_init(HashSet::new))
}

/// mountinfo escapes space, tab, newline and backslash as octal sequences.
#[cfg(target_os = "linux")]
pub(crate) fn unescape_mountinfo(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// true when `rel`, the path of an entry from its command-line argument, matches an `--exclude`
/// pattern. a pattern without `/` is compared with the name of the entry, a pattern with `/` with the
/// whole path. the argument itself has an empty `rel`, so it never matches.
pub(crate) fn is_excluded(patterns: &[String], rel: &Path) -> bool {
    let Some(name) = rel.file_name() else {
        return false;
    };
    let parts: Vec<&[u8]> = rel
        .components()
        .map(|part| part.as_os_str().as_encoded_bytes())
        .collect();
    let whole = parts.join(&b'/');
    patterns.iter().any(|pattern| {
        let text = if pattern.contains('/') {
            whole.as_slice()
        } else {
            name.as_encoded_bytes()
        };
        glob_match(pattern.as_bytes(), text)
    })
}

/// `*` matches any run of characters except `/`, `?` matches one UTF-8 character except `/`, and every
/// other byte matches itself. there are no character classes. the text is walked bytewise, but a step
/// never stops inside a UTF-8 character.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // the last `*`: where its pattern resumes, and the text byte where it started matching.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    star = Some((p + 1, t));
                    p += 1;
                    continue;
                }
                b'?' if text[t] != b'/' => {
                    p += 1;
                    t = char_end(text, t);
                    continue;
                }
                literal if literal == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }
        // no direct match: the last `*` takes one more byte, unless that byte is a `/`.
        match star {
            Some((resume, taken)) if text[taken] != b'/' => {
                let next = char_end(text, taken);
                star = Some((resume, next));
                p = resume;
                t = next;
            }
            _ => return false,
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

/// index just past the UTF-8 character that starts at `at`: the continuation bytes are `0b10xxxxxx`.
fn char_end(text: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    while end < text.len() && (text[end] & 0xC0) == 0x80 {
        end += 1;
    }
    end
}

/// erases `name` inside the open directory `parent`. `st` is its stat, taken without following links.
#[cfg(unix)]
fn erase_entry(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let kind = file_kind(st);
    if kind == SFlag::S_IFLNK {
        unlinkat(parent, name, UnlinkatFlags::NoRemoveDir)
            .map(|()| false)
            .map_err(io::Error::from)
    } else if kind == SFlag::S_IFDIR {
        erase_dir_at(parent, name, st, shown, real, walk, opts)
    } else if kind == SFlag::S_IFREG {
        erase_file_at(parent, name, st, shown, opts).map(|()| false)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not a regular file, directory or symbolic link",
        ))
    }
}

#[cfg(unix)]
fn erase_dir_at(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    make_owner_accessible(parent, name, st, Mode::S_IRWXU)?;
    let dir = open_checked(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    if erase_contents(&dir, st, shown, real, walk, opts)? {
        return Ok(true);
    }
    if opts.verbose {
        eprintln!("  removing directory {}", shown.display());
    }
    remove_at(parent, name, true, shown).map(|()| false)
}

/// erases every entry of the open directory `dir`, whose stat is `dir_st`. a failing entry is
/// reported and skipped, and the directory is kept. returns true when an entry was kept because of
/// `--exclude`, here or in a subdirectory; the caller then keeps this directory too.
#[cfg(unix)]
fn erase_contents(
    dir: &File,
    dir_st: &FileStat,
    shown: &Path,
    real: &Path,
    walk: Walk<'_>,
    opts: &Options,
) -> io::Result<bool> {
    let names = list_names(dir)?;
    let mut failed = 0usize;
    let mut excluded = 0usize;
    let mut kept_inside = false;
    for name in &names {
        let entry = OsStr::from_bytes(name.to_bytes());
        let entry_shown = shown.join(entry);
        let entry_real = real.join(entry);
        let entry_rel = walk.rel.join(entry);
        // excluded entries are not walked, so an excluded directory is not walked either.
        if is_excluded(walk.excludes, &entry_rel) {
            excluded += 1;
            log_skipped_at(walk.log, dir, name, &entry_shown, "excluded by --exclude");
            continue;
        }
        let child = Walk {
            rel: &entry_rel,
            excludes: walk.excludes,
            log: walk.log,
        };
        let result = match fstatat(dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(st) => match is_boundary(&st, dir_st, &entry_real) {
                Ok(true) => {
                    let (kind, size) = stat_kind(&st);
                    log_object(
                        walk.log,
                        &entry_shown,
                        kind,
                        size,
                        Outcome::Skipped,
                        "mount point or other filesystem, skipped",
                    );
                    Err(io::Error::other(
                        "is a mount point or on another filesystem, skipped",
                    ))
                }
                Ok(false) => erase_object(dir, name, &st, &entry_shown, &entry_real, child, opts),
                Err(err) => Err(unseen(walk.log, &entry_shown, err)),
            },
            Err(err) => Err(unseen(walk.log, &entry_shown, io::Error::from(err))),
        };
        match result {
            Ok(inside_kept) => kept_inside |= inside_kept,
            Err(err) => {
                // an interrupt ends the walk: the entries after this one are left as they are. only the
                // innermost entry is named, the directories above it are not repeated.
                if err.kind() == io::ErrorKind::Interrupted {
                    if !is_directory_at(dir, name) {
                        eprintln!("{}: {err}", entry_shown.display());
                    }
                    return Err(err);
                }
                eprintln!("{}: {err}", entry_shown.display());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            shown.display()
        )));
    }
    if excluded > 0 {
        eprintln!(
            "kept {}: {excluded} entries excluded by --exclude",
            shown.display()
        );
    }
    Ok(excluded > 0 || kept_inside)
}

/// names in `dir` except `.` and `..`, read before anything is erased.
#[cfg(unix)]
fn list_names(dir: &File) -> io::Result<Vec<CString>> {
    // fdopendir takes ownership of its descriptor, so the listing reads a duplicate.
    let mut listing = Dir::from_fd(OwnedFd::from(dir.try_clone()?))?;
    let mut names = Vec::new();
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name();
        if name != c"." && name != c".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// same test as `is_mount_boundary`, for an entry already stat'ed through its directory.
#[cfg(unix)]
fn is_boundary(st: &FileStat, dir_st: &FileStat, real: &Path) -> io::Result<bool> {
    if file_kind(st) == SFlag::S_IFLNK {
        return Ok(false);
    }
    Ok(st.st_dev != dir_st.st_dev || mount_points()?.contains(real))
}

/// overwrites the file's contents, then deletes it. the name is moved away first, so a file that
/// cannot be deleted is refused before anything is overwritten.
#[cfg(unix)]
fn erase_file_at(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    opts: &Options,
) -> io::Result<()> {
    check_hard_links(shown, st.st_nlink > 1, opts)?;
    make_owner_accessible(parent, name, st, Mode::S_IWUSR)?;
    let hidden = hide_entry(parent, name, st, shown)?;
    overwrite_hidden(parent, &hidden, st, opts)
        .map_err(|err| keep_partly_overwritten(parent, name, &hidden, shown, err))?;
    remove_hidden(parent, name, &hidden, false, shown)
}

/// overwrites the contents of the renamed entry `hidden`, through a descriptor checked against `st`.
#[cfg(unix)]
fn overwrite_hidden(parent: &File, hidden: &CStr, st: &FileStat, opts: &Options) -> io::Result<()> {
    // nonblock: a fifo swapped in for the file must not block the open. noctty: it must not
    // become the controlling terminal of this process.
    let mut file = open_checked(
        parent,
        hidden,
        OFlag::O_WRONLY
            | OFlag::O_NOFOLLOW
            | OFlag::O_NONBLOCK
            | OFlag::O_NOCTTY
            | OFlag::O_CLOEXEC,
        st,
    )?;
    // the length comes from the handle being overwritten, not from a second lookup by name.
    let whole = 0..file.metadata()?.len();
    overwrite(&mut file, std::slice::from_ref(&whole), opts).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("{err}; the contents may be partly overwritten"),
        )
    })
}

/// renames the entry to a random hidden name in `parent`. the rename is undone when the entry
/// under the hidden name is not the one `st` describes.
#[cfg(unix)]
fn hide_entry(parent: &File, name: &CStr, st: &FileStat, shown: &Path) -> io::Result<CString> {
    let hidden = hidden_name(parent)?;
    renameat(parent, name, parent, hidden.as_c_str())?;
    let checked = fstatat(parent, hidden.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)
        .and_then(|now| {
            if same_entry(&now, st) {
                Ok(())
            } else {
                Err(io::Error::other("entry changed while it was being erased"))
            }
        });
    match checked {
        Ok(()) => Ok(hidden),
        Err(err) => Err(restore_name(parent, name, &hidden, shown, err)),
    }
}

/// opens `name` in `parent` without following links, then checks the descriptor is still the
/// entry that was stat'ed, so a name swapped after the stat is refused.
#[cfg(unix)]
fn open_checked(parent: &File, name: &CStr, flags: OFlag, st: &FileStat) -> io::Result<File> {
    let file = File::from(openat(parent, name, flags, Mode::empty())?);
    if !same_entry(&fstat(&file)?, st) {
        return Err(io::Error::other("entry changed while it was being erased"));
    }
    Ok(file)
}

/// true when `now` is the entry `st` describes: same device, inode and file type.
#[cfg(unix)]
fn same_entry(now: &FileStat, st: &FileStat) -> bool {
    now.st_dev == st.st_dev && now.st_ino == st.st_ino && file_kind(now) == file_kind(st)
}

/// file type bits of a stat result: S_IFDIR, S_IFREG, S_IFLNK and the rest.
#[cfg(unix)]
fn file_kind(st: &FileStat) -> SFlag {
    SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT
}

/// adds `owner_bits` to the owner's permissions when they are missing; other bits are kept.
/// works on mode-000 entries too, because it needs no read or write access to the entry.
#[cfg(unix)]
fn make_owner_accessible(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    owner_bits: Mode,
) -> io::Result<()> {
    let mode = Mode::from_bits_truncate(st.st_mode);
    if mode.contains(owner_bits) {
        return Ok(());
    }
    set_mode_at(parent, name, st, mode | owner_bits)
}

/// changes the mode of the inode that `st` describes. linux goes through a path handle and
/// /proc, so the change lands on that inode even if the name is swapped meanwhile.
#[cfg(target_os = "linux")]
fn set_mode_at(parent: &File, name: &CStr, st: &FileStat, mode: Mode) -> io::Result<()> {
    // O_PATH needs no access to the entry itself, only search permission on `parent`.
    let handle = open_checked(
        parent,
        name,
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    let proc_path = format!("/proc/self/fd/{}", handle.as_raw_fd());
    // the path is absolute, so `parent` is not used for the lookup.
    fchmodat(
        parent,
        proc_path.as_str(),
        mode,
        FchmodatFlags::FollowSymlink,
    )
    .map_err(io::Error::from)
}

/// other unix systems have no path handle without access to the entry, so the change goes by
/// name, with the identity checked right before it.
#[cfg(all(unix, not(target_os = "linux")))]
fn set_mode_at(parent: &File, name: &CStr, st: &FileStat, mode: Mode) -> io::Result<()> {
    let now = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if now.st_dev != st.st_dev || now.st_ino != st.st_ino {
        return Err(io::Error::other("entry changed while it was being erased"));
    }
    fchmodat(parent, name, mode, FchmodatFlags::NoFollowSymlink).map_err(io::Error::from)
}

/// renames the entry to a random hidden name in `parent`, then deletes it, so the original name
/// does not remain in the directory.
#[cfg(unix)]
fn remove_at(parent: &File, name: &CStr, is_dir: bool, shown: &Path) -> io::Result<()> {
    let hidden = hidden_name(parent)?;
    renameat(parent, name, parent, hidden.as_c_str())?;
    remove_hidden(parent, name, &hidden, is_dir, shown)
}

/// deletes the entry that was renamed to `hidden`. if that fails, the name is put back.
#[cfg(unix)]
fn remove_hidden(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    is_dir: bool,
    shown: &Path,
) -> io::Result<()> {
    let flags = if is_dir {
        UnlinkatFlags::RemoveDir
    } else {
        UnlinkatFlags::NoRemoveDir
    };
    unlinkat(parent, hidden, flags)
        .map_err(|err| restore_name(parent, name, hidden, shown, io::Error::from(err)))
}

/// renames the hidden entry back to `name`. on failure returns where the entry is now.
#[cfg(unix)]
fn put_back(parent: &File, name: &CStr, hidden: &CStr, shown: &Path) -> Result<(), PathBuf> {
    // a name taken in the meantime is not replaced by the rename back.
    let name_free = matches!(
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW),
        Err(Errno::ENOENT)
    );
    if name_free && renameat(parent, hidden, parent, name).is_ok() {
        return Ok(());
    }
    Err(shown.with_file_name(OsStr::from_bytes(hidden.to_bytes())))
}

/// puts a hidden entry back under `name` after a failed step. if that fails too, the error says
/// where the entry is.
#[cfg(unix)]
fn restore_name(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    shown: &Path,
    err: io::Error,
) -> io::Error {
    match put_back(parent, name, hidden, shown) {
        Ok(()) => err,
        Err(left) => io::Error::new(
            err.kind(),
            format!("{err}; the entry is left as {}", left.display()),
        ),
    }
}

/// puts back a file whose overwrite failed. the error says the file is kept under its name, or where
/// it is left.
#[cfg(unix)]
fn keep_partly_overwritten(
    parent: &File,
    name: &CStr,
    hidden: &CStr,
    shown: &Path,
    err: io::Error,
) -> io::Error {
    match put_back(parent, name, hidden, shown) {
        Ok(()) => io::Error::new(err.kind(), format!("{err}, the file is kept")),
        Err(left) => io::Error::new(
            err.kind(),
            format!("{err}; the entry is left as {}", left.display()),
        ),
    }
}

/// true when `name` in `dir` is a directory, without following links.
#[cfg(unix)]
fn is_directory_at(dir: &File, name: &CStr) -> bool {
    matches!(
        fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW),
        Ok(st) if file_kind(&st) == SFlag::S_IFDIR
    )
}

/// a random name that is free in `parent`. only ENOENT from fstatat on the directory counts as free.
#[cfg(unix)]
fn hidden_name(parent: &File) -> io::Result<CString> {
    loop {
        let name =
            CString::new(format!(".{:016x}", rand::random::<u64>())).map_err(io::Error::other)?;
        match fstatat(parent, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Err(Errno::ENOENT) => return Ok(name),
            Ok(_) => {}
            Err(err) => return Err(io::Error::from(err)),
        }
    }
}

/// overwrites the file's contents, then deletes it.
///
/// if deletion fails after the overwrite, the contents are already destroyed
/// but the file itself remains.
#[cfg(windows)]
fn erase_file(path: &Path, opts: &Options) -> io::Result<()> {
    make_writable(path, false)?;

    match open_for_overwrite(path) {
        Ok(mut file) => {
            // the length comes from the handle being overwritten, not from a second lookup by name.
            let meta = file.metadata()?;
            check_hard_links(path, link_count(&meta) > 1, opts)?;
            overwrite_in_place(&mut file, meta.len(), opts)?;
            drop(file);
            remove_entry(path, opts)
        }
        Err(err) if is_locked(&err) => erase_locked(path, opts, err),
        Err(err) => Err(err),
    }
}

/// why a locked file has no listed holder. code 5 is also an acl or delete-pending state, so it
/// does not claim a lock; 32 and 33 are sharing and lock violations, held by something unlisted.
#[cfg(windows)]
fn no_holder_reason(err: &io::Error) -> &'static str {
    match err.raw_os_error() {
        Some(32 | 33) => {
            "the file is in use, but no process is listed (a service, driver or antivirus may hold it)"
        }
        _ => {
            "no lock holder was listed, so the file may be pending deletion or its permissions may deny access"
        }
    }
}

/// handles a file that another process holds open.
#[cfg(windows)]
fn erase_locked(path: &Path, opts: &Options, err: io::Error) -> io::Result<()> {
    let len = fs::metadata(path)?.len();
    let listing = windows::holders(path);
    let holders = listing.as_deref().unwrap_or_default();
    for holder in holders {
        eprintln!(
            "{}: locked by {} (pid {})",
            path.display(),
            holder.name,
            holder.pid
        );
    }

    if opts.kill_holders && !holders.is_empty() {
        if !opts.force
            && !prompt::ask(
                "close these processes to unlock the file? unsaved data in them is lost",
            )
        {
            return Err(io::Error::other("skipped, file is still locked"));
        }
        eprintln!(
            "warning: closing {} process(es) may lose unsaved data and stop services",
            holders.len()
        );
        windows::close_holders(path)?;

        match open_for_overwrite(path) {
            Ok(mut file) => {
                overwrite_in_place(&mut file, len, opts)?;
                drop(file);
                return remove_entry(path, opts);
            }
            Err(retry_err) => {
                // the forced shutdown can leave holders or let new ones open the file: list them again.
                let remaining = windows::holders(path);
                for holder in remaining.as_deref().unwrap_or_default() {
                    eprintln!(
                        "{}: still locked by {} (pid {})",
                        path.display(),
                        holder.name,
                        holder.pid
                    );
                }
                return Err(match &remaining {
                    Ok(list) if list.is_empty() => {
                        io::Error::other(format!("{retry_err}; {}", no_holder_reason(&retry_err)))
                    }
                    Ok(_) => {
                        io::Error::other(format!("{retry_err}; still locked after closing holders"))
                    }
                    Err(list_err) => io::Error::other(format!(
                        "{retry_err}; could not list lock holders: {list_err}"
                    )),
                });
            }
        }
    }

    Err(match &listing {
        Ok(list) if list.is_empty() => {
            io::Error::other(format!("{err}; {}", no_holder_reason(&err)))
        }
        Ok(_) => io::Error::other(format!("{err}; use --kill-holders to erase it anyway")),
        Err(list_err) => {
            io::Error::other(format!("{err}; could not list lock holders: {list_err}"))
        }
    })
}

/// overwrites a file in place. windows does not rename it first, so an interrupted overwrite leaves
/// the file under its name.
#[cfg(windows)]
fn overwrite_in_place(file: &mut File, len: u64, opts: &Options) -> io::Result<()> {
    let whole = 0..len;
    overwrite(file, std::slice::from_ref(&whole), opts).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("{err}; the contents may be partly overwritten, the file is kept"),
        )
    })
}

#[cfg(windows)]
fn open_for_overwrite(path: &Path) -> io::Result<File> {
    windows::open_for_erase(path)
}

/// sharing violation, lock violation, or access denied (which windows also returns
/// for delete-pending files).
#[cfg(windows)]
fn is_locked(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

/// windows: posix delete first (removes the name even while other handles are open),
/// then rename and delete, then deletion at the next reboot (only if not forced).
#[cfg(windows)]
fn remove_entry(path: &Path, opts: &Options) -> io::Result<()> {
    if let Ok(handle) = windows::open_for_delete(path)
        && windows::posix_delete(&handle).is_ok()
    {
        return Ok(());
    }
    let delete_err = match unlink_with_random_name(path, false) {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };

    if opts.force
        || !prompt::ask(
            "file is still in use and cannot be deleted now (contents are already erased). schedule deletion at next reboot?",
        )
    {
        return Err(io::Error::other(format!(
            "contents are erased, the file remains ({delete_err}) and is not scheduled for deletion at reboot"
        )));
    }
    windows::delete_on_reboot(path)
}

fn overwrite(file: &mut File, ranges: &[Range<u64>], opts: &Options) -> io::Result<()> {
    let size: u64 = ranges.iter().map(|range| range.end - range.start).sum();
    let total = size.saturating_mul(u64::from(opts.passes));
    let mut progress = Progress::new(Some(total), opts.quiet);
    let result = overwrite_passes(file, ranges, opts, &mut progress);
    progress.finish();
    result
}

fn overwrite_passes(
    file: &mut File,
    ranges: &[Range<u64>],
    opts: &Options,
    progress: &mut Progress,
) -> io::Result<()> {
    let mut buf = vec![0u8; BUFFER_SIZE];
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            progress.finish();
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        for range in ranges {
            overwrite_range(file, range, pattern, &mut buf, progress)?;
        }
        file.sync_all()?;
    }
    Ok(())
}

fn overwrite_range(
    file: &mut File,
    range: &Range<u64>,
    pattern: Pattern,
    buf: &mut [u8],
    progress: &mut Progress,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(range.start))?;

    let mut remaining = range.end - range.start;
    while remaining > 0 {
        check_interrupted()?;
        let chunk = remaining.min(buf.len() as u64) as usize;
        fill(&mut buf[..chunk], pattern);
        file.write_all(&buf[..chunk])?;
        progress.add(chunk as u64);
        remaining -= chunk as u64;
    }
    Ok(())
}

pub(crate) fn fill(buf: &mut [u8], pattern: Pattern) {
    match pattern {
        Pattern::Zeros => buf.fill(0x00),
        Pattern::Ones => buf.fill(0xFF),
        Pattern::Random => rand::rng().fill_bytes(buf),
    }
}

/// clears the read-only flag so the entry can be overwritten and renamed.
#[cfg(windows)]
fn make_writable(path: &Path, is_dir: bool) -> io::Result<()> {
    // permissions are changed through the path, so refuse a symlink that replaced it.
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other(
            "refusing to change permissions of a symbolic link",
        ));
    }
    // cloud and dedup reparse points get their readonly flag cleared before open_for_erase refuses them.
    let mut perms = fs::metadata(path)?.permissions();
    if !perms.readonly() {
        return Ok(());
    }
    set_writable(&mut perms, is_dir);
    fs::set_permissions(path, perms)
}

#[cfg(windows)]
fn set_writable(perms: &mut fs::Permissions, _is_dir: bool) {
    perms.set_readonly(false);
}

/// renames the entry to a random hidden name in the same directory, then deletes it,
/// so the original name does not remain in the directory.
#[cfg(windows)]
fn unlink_with_random_name(path: &Path, is_dir: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let hidden = loop {
        let candidate = dir.join(format!(".{:016x}", rand::random::<u64>()));
        if !candidate.exists() {
            break candidate;
        }
    };

    fs::rename(path, &hidden)?;
    let removed = if is_dir {
        fs::remove_dir(&hidden)
    } else {
        fs::remove_file(&hidden)
    };
    match removed {
        Ok(()) => Ok(()),
        Err(remove_err) => {
            // put the name back so the entry is not left hidden; if that fails, report where it is.
            match fs::rename(&hidden, path) {
                Ok(()) => Err(remove_err),
                Err(_) => Err(io::Error::other(format!(
                    "{remove_err}; the entry is left as {}",
                    hidden.display()
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::test_support::{Memory, Torn};
    use std::fs::OpenOptions;

    fn erase_path(path: &Path, opts: &Options) -> io::Result<()> {
        super::erase_path(path, opts, &[], None).map(|_| ())
    }

    fn options(passes: u32) -> Options {
        Options {
            passes,
            verbose: false,
            force: true,
            kill_holders: false,
            allow_protected: false,
            quiet: true,
        }
    }

    fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn second_signal_is_the_call_that_finds_the_flag_set() {
        let flag = AtomicBool::new(false);

        assert!(!record_interrupt(&flag));
        assert!(record_interrupt(&flag));
    }

    #[test]
    fn interrupted_object_is_logged_once_however_many_parents_stop() {
        let memory = Memory::default();
        let log = Log::new(Path::new("erase.log"), 1, Box::new(memory.clone()));
        let interrupted = || -> io::Result<bool> {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "interrupted by signal",
            ))
        };

        log_done(
            Some(&log),
            Path::new("dir/a.txt"),
            Kind::File,
            5,
            &interrupted(),
            false,
            "",
        );
        log_done(
            Some(&log),
            Path::new("dir"),
            Kind::Dir,
            0,
            &interrupted(),
            false,
            "",
        );

        let text = memory.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"path\":\"dir/a.txt\""));
        assert!(text.contains("\"result\":\"error\",\"detail\":\"interrupted\""));
    }

    #[test]
    fn erase_stops_after_a_log_line_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        for name in ["a", "b", "c"] {
            write_file(&tree, name, b"data");
        }
        let log = Log::new(
            Path::new("erase.log"),
            1,
            Box::new(Torn {
                memory: Memory::default(),
                room: 0,
            }),
        );

        let result = super::erase_path(&tree, &options(1), &[], Some(&log));

        assert!(result.is_err());
        assert!(log.failed());
        // the first object was erased before its line failed; the other two were not touched.
        assert_eq!(fs::read_dir(&tree).unwrap().count(), 2);
    }

    #[test]
    fn pattern_sequence_ends_with_random() {
        assert_eq!(pattern_for_pass(0, 1), Pattern::Random);
        assert_eq!(pattern_for_pass(0, 3), Pattern::Zeros);
        assert_eq!(pattern_for_pass(1, 3), Pattern::Ones);
        assert_eq!(pattern_for_pass(2, 3), Pattern::Random);
    }

    #[test]
    fn zeros_pass_replaces_contents_and_keeps_length() {
        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 2 * BUFFER_SIZE + 17];
        let path = write_file(dir.path(), "data.bin", &original);

        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        let len = original.len() as u64;
        overwrite_range(
            &mut file,
            &(0..len),
            Pattern::Zeros,
            &mut vec![0u8; BUFFER_SIZE],
            &mut Progress::new(None, true),
        )
        .unwrap();
        drop(file);

        let contents = fs::read(&path).unwrap();
        assert_eq!(contents.len(), original.len());
        assert!(contents.iter().all(|&b| b == 0));
    }

    #[test]
    fn random_pass_changes_contents() {
        let dir = tempfile::tempdir().unwrap();
        let original = vec![0xAB; 4096];
        let path = write_file(dir.path(), "data.bin", &original);

        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        let len = original.len() as u64;
        overwrite_range(
            &mut file,
            &(0..len),
            Pattern::Random,
            &mut vec![0u8; BUFFER_SIZE],
            &mut Progress::new(None, true),
        )
        .unwrap();
        drop(file);

        let contents = fs::read(&path).unwrap();
        assert_eq!(contents.len(), original.len());
        assert_ne!(contents, original);
    }

    #[test]
    fn erase_removes_file_without_leaving_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "secret.txt", b"secret");

        erase_path(&path, &options(2)).unwrap();

        assert!(!path.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn erase_handles_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "empty.txt", b"");

        erase_path(&path, &options(1)).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn erase_removes_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("nested/deeper")).unwrap();
        write_file(&tree, "a.txt", b"a");
        write_file(&tree.join("nested/deeper"), "b.txt", b"b");

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn erase_removes_read_only_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        let file = write_file(&tree, "locked.txt", b"data");

        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();
        let mut perms = fs::metadata(&tree).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&tree, perms).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
    }

    #[cfg(unix)]
    #[test]
    fn erase_removes_nested_read_only_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let nested = tree.join("nested");
        fs::create_dir_all(&nested).unwrap();
        write_file(&nested, "data.txt", b"data");
        let mut perms = fs::metadata(&nested).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&nested, perms).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
    }

    #[cfg(unix)]
    #[test]
    fn erase_removes_mode_000_file_and_directory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let nested = tree.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let file = write_file(&nested, "data.txt", b"data");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(&tree, fs::Permissions::from_mode(0o000)).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_table_lists_the_root_filesystem() {
        assert!(mount_points().unwrap().contains(Path::new("/")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(unescape_mountinfo(r"/mnt/a\040b\134c"), "/mnt/a b\\c");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_removed_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target.txt", b"keep me");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        erase_path(&link, &options(1)).unwrap();

        assert!(!link.exists());
        assert_eq!(fs::read(&target).unwrap(), b"keep me");
    }

    #[test]
    fn hard_link_rule_refuses_only_hard_links_under_root() {
        assert_eq!(hard_link_rule(true, true, false), HardLinkRule::Refuse);
        assert_eq!(
            hard_link_rule(true, true, true),
            HardLinkRule::EraseWithWarning
        );
        assert_eq!(hard_link_rule(true, false, false), HardLinkRule::Erase);
        assert_eq!(hard_link_rule(true, false, true), HardLinkRule::Erase);
        assert_eq!(hard_link_rule(false, false, false), HardLinkRule::Erase);
        assert_eq!(hard_link_rule(false, false, true), HardLinkRule::Erase);
        assert_eq!(hard_link_rule(false, true, false), HardLinkRule::Erase);
        assert_eq!(hard_link_rule(false, true, true), HardLinkRule::Erase);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_tree_does_not_erase_outside_target() {
        let outside = tempfile::tempdir().unwrap();
        let outside_file = write_file(outside.path(), "outside.txt", b"keep me");

        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(outside.path(), tree.join("link")).unwrap();

        erase_path(&tree, &options(1)).unwrap();

        assert!(!tree.exists());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep me");
    }

    #[test]
    fn glob_star_matches_any_run_but_not_a_slash() {
        assert!(glob_match(b"*.o", b"main.o"));
        assert!(glob_match(b"*", b"anything"));
        assert!(glob_match(b"a*c", b"ac"));
        assert!(glob_match(b"a*c", b"abbbc"));
        assert!(!glob_match(b"a*c", b"abd"));
        assert!(!glob_match(b"*.o", b"src/main.o"));
        assert!(glob_match(b"*a*b", b"xaab"));
        assert!(!glob_match(b"*a*b", b"xaba"));
    }

    #[test]
    fn glob_question_matches_one_byte_but_not_a_slash() {
        assert!(glob_match(b"?.txt", b"a.txt"));
        assert!(!glob_match(b"?.txt", b"ab.txt"));
        assert!(!glob_match(b"?.txt", b"/.txt"));
        assert!(!glob_match(b"a?b", b"a/b"));
    }

    #[test]
    fn glob_question_matches_one_utf8_character() {
        assert!(glob_match("?.txt".as_bytes(), "ä.txt".as_bytes()));
        assert!(!glob_match("??.txt".as_bytes(), "ä.txt".as_bytes()));
        assert!(glob_match("?".as_bytes(), "ö".as_bytes()));
        assert!(!glob_match("?".as_bytes(), "/".as_bytes()));
        assert!(glob_match("a*b".as_bytes(), "aäöb".as_bytes()));
        assert!(glob_match("*ö*".as_bytes(), "xöy".as_bytes()));
    }

    #[test]
    fn glob_other_bytes_match_literally() {
        assert!(glob_match(b"file.txt", b"file.txt"));
        assert!(!glob_match(b"file.txt", b"fileXtxt"));
        assert!(glob_match(b"a[b]", b"a[b]"));
        assert!(!glob_match(b"a[b]", b"ab"));
    }

    #[test]
    fn pattern_without_slash_matches_the_name_at_any_depth() {
        let patterns = ["*.log".to_owned()];

        assert!(is_excluded(&patterns, Path::new("a.log")));
        assert!(is_excluded(&patterns, Path::new("sub/deeper/a.log")));
        assert!(!is_excluded(&patterns, Path::new("sub/a.logx")));
    }

    #[test]
    fn pattern_with_slash_matches_the_path_from_the_argument() {
        let patterns = ["src/*.o".to_owned()];

        assert!(is_excluded(&patterns, Path::new("src/main.o")));
        assert!(!is_excluded(&patterns, Path::new("main.o")));
        assert!(!is_excluded(&patterns, Path::new("other/src/main.o")));
        assert!(!is_excluded(&patterns, Path::new("src/deeper/main.o")));
    }

    #[test]
    fn command_line_argument_is_never_excluded() {
        let patterns = ["*".to_owned(), "build".to_owned()];

        assert!(!is_excluded(&patterns, Path::new("")));
    }

    #[test]
    fn excluded_directory_is_skipped_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let skipped = tree.join("skip");
        fs::create_dir_all(&skipped).unwrap();
        let inside = write_file(&skipped, "inside.txt", b"keep me");
        let erased = write_file(&tree, "gone.txt", b"gone");
        let excludes = ["skip".to_owned()];

        let kept = super::erase_path(&tree, &options(1), &excludes, None).unwrap();

        assert!(kept);
        assert_eq!(fs::read(&inside).unwrap(), b"keep me");
        assert!(!erased.exists());
        assert!(tree.exists());
    }
}
