//! protection checks against the entries and ancestors actually opened by unix operations.

use std::fs::File;
use std::io;
use std::sync::OnceLock;

use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat};

fn same(a: &FileStat, b: &FileStat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}

/// checks the selected entry and the parent chain without resolving the user's path again.
pub(crate) fn check_entry(parent: &File, entry: &FileStat, allow: bool) -> io::Result<()> {
    let directory = SFlag::from_bits_truncate(entry.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR;
    check(parent, directory.then_some(entry), allow, true)
}

pub(crate) fn check_wipe(dir: &File, allow: bool) -> io::Result<()> {
    check(dir, Some(&fstat(dir)?), allow, false)
}

struct Protected {
    subtree: Vec<FileStat>,
    exact: Vec<FileStat>,
    homes: Vec<FileStat>,
}

fn protected() -> &'static Protected {
    static ROOTS: OnceLock<Protected> = OnceLock::new();
    ROOTS.get_or_init(|| Protected {
        subtree: super::protected_roots()
            .iter()
            .filter_map(|path| nix::sys::stat::stat(path).ok())
            .collect(),
        exact: super::exact_roots()
            .iter()
            .filter_map(|path| nix::sys::stat::stat(path).ok())
            .collect(),
        homes: [std::env::home_dir(), crate::invocation::sudo_home()]
            .into_iter()
            .flatten()
            .filter_map(|path| nix::sys::stat::stat(&path).ok())
            .collect(),
    })
}

fn check(parent: &File, entry: Option<&FileStat>, allow: bool, erase: bool) -> io::Result<()> {
    let Protected {
        subtree,
        exact,
        homes,
    } = protected();
    if !allow
        && entry.is_some_and(|entry| {
            subtree
                .iter()
                .chain(exact)
                .chain(homes.iter().filter(|_| erase))
                .any(|root| same(entry, root))
        })
    {
        return Err(io::Error::other(
            "refusing to change a protected system path (--i-know-what-i-am-doing allows it)",
        ));
    }
    let mut dir = parent.try_clone()?;
    // bound ancestry traversal too; a failed or changing chain must never grant permission.
    for _ in 0..4096 {
        let here = fstat(&dir)?;
        #[cfg(target_os = "linux")]
        let access = OFlag::O_PATH;
        #[cfg(not(target_os = "linux"))]
        let access = OFlag::O_RDONLY;
        let up = File::from(openat(
            &dir,
            c"..",
            access | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?);
        let above = fstat(&up)?;
        if same(&here, &above) {
            if entry.is_some_and(|entry| same(entry, &here)) {
                return Err(io::Error::other("refusing to change a filesystem root"));
            }
            return Ok(());
        }
        if !allow && subtree.iter().any(|root| same(&here, root)) {
            return Err(io::Error::other(
                "refusing to change a protected system path (--i-know-what-i-am-doing allows it)",
            ));
        }
        dir = up;
    }
    Err(io::Error::other(
        "could not establish the opened directory's ancestors",
    ))
}
