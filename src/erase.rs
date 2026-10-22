use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
use nix::sys::stat::{FchmodatFlags, FileStat, Mode, SFlag, fchmodat, fstat, fstatat};
#[cfg(unix)]
use nix::unistd::{UnlinkatFlags, unlinkat};
use rand::RngCore;

#[cfg(windows)]
use crate::platform::windows;
#[cfg(windows)]
use crate::prompt;

/// size of the buffer used for each write, in bytes.
const BUFFER_SIZE: usize = 1024 * 1024;

/// shown before a raw volume write; this path bypasses the filesystem entirely.
#[cfg(windows)]
const RAW_VOLUME_WARNING: &str = "raw volume write bypasses the filesystem: it can corrupt the volume \
    if the file changes during the operation, and data cached by other programs may survive";

/// settings for one erase run.
pub struct Options {
    pub passes: u32,
    pub verbose: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub force: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub kill_holders: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub raw_volume: bool,
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
#[cfg(unix)]
pub fn erase_path(path: &Path, opts: &Options) -> io::Result<()> {
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
    let dir = File::open(parent)?;
    let entry = CString::new(name.as_bytes()).map_err(io::Error::other)?;
    let st = fstatat(&dir, entry.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
    let real = fs::canonicalize(parent)?.join(name);
    // a mount point cannot be renamed away, so it is refused before anything inside it is erased.
    if is_boundary(&st, &fstat(&dir)?, &real)? {
        return Err(io::Error::other(
            "is a mount point or on another filesystem, refusing to erase it",
        ));
    }
    erase_entry(&dir, entry.as_c_str(), &st, path, &real, opts)
}

/// erases a file, a directory tree, or a symbolic link.
///
/// symbolic links are removed without being followed, so their targets are never touched.
/// directories are erased depth-first. a failing entry is reported and skipped; its directory
/// is kept, so the error for the whole tree is returned.
#[cfg(windows)]
pub fn erase_path(path: &Path, opts: &Options) -> io::Result<()> {
    let file_type = fs::symlink_metadata(path)?.file_type();

    if file_type.is_symlink() {
        return fs::remove_file(path)
            .or_else(|file_err| fs::remove_dir(path).map_err(|_| file_err));
    }
    if file_type.is_dir() {
        return erase_dir(path, opts);
    }
    if file_type.is_file() {
        return erase_file(path, opts);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not a regular file, directory or symbolic link",
    ))
}

#[cfg(windows)]
fn erase_dir(path: &Path, opts: &Options) -> io::Result<()> {
    make_writable(path, true)?;

    // collect entries first so the directory is not modified while it is being iterated.
    let entries: Vec<PathBuf> = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()?;
    // never erase what is mounted inside the tree: that is another disk or the source of a bind mount.
    let parent_dev = device(&fs::symlink_metadata(path)?);
    let mut failed = 0usize;
    for entry in &entries {
        let result = match is_mount_boundary(entry, parent_dev) {
            Ok(true) => Err(io::Error::other(
                "is a mount point or on another filesystem, skipped",
            )),
            Ok(false) => erase_path(entry, opts),
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            eprintln!("{}: {err}", entry.display());
            failed += 1;
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            path.display()
        )));
    }

    if opts.verbose {
        eprintln!("  removing directory {}", path.display());
    }
    unlink_with_random_name(path, true)
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
fn unescape_mountinfo(path: &str) -> String {
    path.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// erases `name` inside the open directory `parent`. `st` is its stat, taken without following links.
#[cfg(unix)]
fn erase_entry(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    shown: &Path,
    real: &Path,
    opts: &Options,
) -> io::Result<()> {
    let kind = file_kind(st);
    if kind == SFlag::S_IFLNK {
        unlinkat(parent, name, UnlinkatFlags::NoRemoveDir).map_err(io::Error::from)
    } else if kind == SFlag::S_IFDIR {
        erase_dir_at(parent, name, st, shown, real, opts)
    } else if kind == SFlag::S_IFREG {
        erase_file_at(parent, name, st, shown, opts)
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
    opts: &Options,
) -> io::Result<()> {
    make_owner_accessible(parent, name, st, Mode::S_IRWXU)?;
    let dir = open_checked(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        st,
    )?;
    erase_contents(&dir, st, shown, real, opts)?;
    if opts.verbose {
        eprintln!("  removing directory {}", shown.display());
    }
    remove_at(parent, name, true, shown)
}

/// erases every entry of the open directory `dir`, whose stat is `dir_st`. a failing entry is
/// reported and skipped, and the directory is kept.
#[cfg(unix)]
fn erase_contents(
    dir: &File,
    dir_st: &FileStat,
    shown: &Path,
    real: &Path,
    opts: &Options,
) -> io::Result<()> {
    let names = list_names(dir)?;
    let mut failed = 0usize;
    for name in &names {
        let entry = OsStr::from_bytes(name.to_bytes());
        let entry_shown = shown.join(entry);
        let entry_real = real.join(entry);
        let result = match fstatat(dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(st) => match is_boundary(&st, dir_st, &entry_real) {
                Ok(true) => Err(io::Error::other(
                    "is a mount point or on another filesystem, skipped",
                )),
                Ok(false) => erase_entry(dir, name, &st, &entry_shown, &entry_real, opts),
                Err(err) => Err(err),
            },
            Err(err) => Err(io::Error::from(err)),
        };
        if let Err(err) = result {
            eprintln!("{}: {err}", entry_shown.display());
            failed += 1;
        }
    }
    if failed > 0 {
        return Err(io::Error::other(format!(
            "{failed} entries could not be erased, {} was kept",
            shown.display()
        )));
    }
    Ok(())
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
    make_owner_accessible(parent, name, st, Mode::S_IWUSR)?;
    let hidden = hide_entry(parent, name, st, shown)?;
    overwrite_hidden(parent, &hidden, st, opts)
        .map_err(|err| restore_name(parent, name, &hidden, shown, err))?;
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
    // a name taken in the meantime is not replaced by the rename back.
    let name_free = matches!(
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW),
        Err(Errno::ENOENT)
    );
    if name_free && renameat(parent, hidden, parent, name).is_ok() {
        return err;
    }
    io::Error::other(format!(
        "{err}; the entry is left as {}",
        shown
            .with_file_name(OsStr::from_bytes(hidden.to_bytes()))
            .display()
    ))
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
            let whole = 0..file.metadata()?.len();
            overwrite(&mut file, std::slice::from_ref(&whole), opts)?;
            drop(file);
            remove_entry(path, opts)
        }
        Err(err) if is_locked(&err) => erase_locked(path, opts, err),
        Err(err) => Err(err),
    }
}

/// handles a file that another process holds open.
#[cfg(windows)]
fn erase_locked(path: &Path, opts: &Options, err: io::Error) -> io::Result<()> {
    let len = fs::metadata(path)?.len();
    let holders = match windows::holders(path) {
        Ok(holders) => holders,
        Err(holders_err) => {
            eprintln!(
                "{}: could not list lock holders: {holders_err}",
                path.display()
            );
            Vec::new()
        }
    };
    for holder in &holders {
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
                let whole = 0..len;
                overwrite(&mut file, std::slice::from_ref(&whole), opts)?;
                drop(file);
                return remove_entry(path, opts);
            }
            Err(retry_err) => {
                eprintln!(
                    "{}: still locked after closing holders: {retry_err}",
                    path.display()
                );
            }
        }
    }

    if opts.raw_volume {
        // resolve the layout and open the volume (needs admin rights) before asking,
        // so a missing privilege or a resident file fails before the user confirms
        let layout = windows::physical_layout(path)?;
        let mut volume = windows::open_volume_for_write(&layout.volume)?;
        eprintln!("warning: {RAW_VOLUME_WARNING}");
        if !holders.is_empty() {
            eprintln!(
                "warning: {} process(es) still hold the file and may write cached data back over the erased clusters",
                holders.len()
            );
        }
        if !opts.force && !prompt::ask("continue with raw volume write?") {
            return Err(io::Error::other("aborted, file is still locked"));
        }
        // the file may have changed while the question was open: write only what was confirmed.
        let fresh = windows::physical_layout(path)?;
        if fresh.ranges != layout.ranges {
            return Err(io::Error::other(
                "the file changed while waiting for confirmation, nothing was written",
            ));
        }
        let ranges: Vec<Range<u64>> = layout
            .ranges
            .iter()
            .map(|range| range.offset..range.offset + range.len)
            .collect();
        overwrite(&mut volume, &ranges, opts)?;
        drop(volume);
        // a holder may have extended the file during the write; clusters outside the confirmed
        // layout were not erased, so the name is not removed.
        if windows::physical_layout(path)?.ranges != layout.ranges {
            return Err(io::Error::other(
                "the file changed during the raw volume write, its new clusters may still hold data",
            ));
        }
        return remove_entry(path, opts);
    }

    if holders.is_empty() {
        return Err(io::Error::other(format!(
            "{err}; no process holds the file, check its permissions"
        )));
    }
    Err(io::Error::other(format!(
        "{err}; use --kill-holders or --raw-volume to erase it anyway"
    )))
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
    let mut buf = vec![0u8; BUFFER_SIZE];
    for pass in 0..opts.passes {
        let pattern = pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        for range in ranges {
            overwrite_range(file, range, pattern, &mut buf)?;
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
) -> io::Result<()> {
    file.seek(SeekFrom::Start(range.start))?;

    let mut remaining = range.end - range.start;
    while remaining > 0 {
        let chunk = remaining.min(buf.len() as u64) as usize;
        fill(&mut buf[..chunk], pattern);
        file.write_all(&buf[..chunk])?;
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
    use std::fs::OpenOptions;

    fn options(passes: u32) -> Options {
        Options {
            passes,
            verbose: false,
            force: true,
            kill_holders: false,
            raw_volume: false,
        }
    }

    fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        path
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
}
