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

/// checks an entry reached inside a walk by its own identity only, without walking its ancestors.
///
/// this equals `check_entry` for every entry below the walk's start. the start went through the full
/// check: no ancestor of it is a protected subtree root. every directory the walk entered below it
/// was compared with all protected identities (subtree roots, exact roots, homes) before it was
/// entered, so none of its ancestors is a subtree root either. that is all the ancestor walk looks for.
/// the filesystem-root refusal needs no repeat: a root of a filesystem is a mount point, and the walk
/// does not cross those. the one case this does not see is a directory that is already inside the
/// walk being moved under a protected subtree afterwards, which needs write access there.
pub(crate) fn check_walk_entry(entry: &FileStat, allow: bool) -> io::Result<()> {
    let directory = SFlag::from_bits_truncate(entry.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR;
    if directory {
        refuse_protected_identity(entry, allow, true)
    } else {
        Ok(())
    }
}

struct Protected {
    subtree: Vec<FileStat>,
    exact: Vec<FileStat>,
    /// roots that only erasing protects: `--wipe-free` adds files there and removes none.
    erase_subtree: Vec<FileStat>,
    erase_exact: Vec<FileStat>,
    homes: Vec<FileStat>,
}

fn identities(roots: Vec<std::path::PathBuf>) -> Vec<FileStat> {
    roots
        .iter()
        .filter_map(|path| nix::sys::stat::stat(path).ok())
        .collect()
}

fn protected() -> &'static Protected {
    static ROOTS: OnceLock<Protected> = OnceLock::new();
    ROOTS.get_or_init(|| Protected {
        subtree: identities(super::protected_roots()),
        exact: identities(super::exact_roots()),
        erase_subtree: identities(super::erase_only_subtree_roots()),
        erase_exact: identities(super::erase_only_exact_roots()),
        homes: [std::env::home_dir(), crate::invocation::sudo_home()]
            .into_iter()
            .flatten()
            .filter_map(|path| nix::sys::stat::stat(&path).ok())
            .collect(),
    })
}

/// refuses a directory that is one of the protected roots or homes. `erase` adds the roots and
/// homes that only erasing protects.
fn refuse_protected_identity(entry: &FileStat, allow: bool, erase: bool) -> io::Result<()> {
    let Protected {
        subtree,
        exact,
        erase_subtree,
        erase_exact,
        homes,
    } = protected();
    if !allow
        && subtree
            .iter()
            .chain(exact)
            .chain(
                erase_subtree
                    .iter()
                    .chain(erase_exact)
                    .chain(homes)
                    .filter(|_| erase),
            )
            .any(|root| same(entry, root))
    {
        return Err(io::Error::other(
            "refusing to change a protected system path (--i-know-what-i-am-doing allows it)",
        ));
    }
    Ok(())
}

fn check(parent: &File, entry: Option<&FileStat>, allow: bool, erase: bool) -> io::Result<()> {
    if let Some(entry) = entry {
        refuse_protected_identity(entry, allow, erase)?;
    }
    let Protected {
        subtree,
        erase_subtree,
        ..
    } = protected();
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
        if !allow
            && subtree
                .iter()
                .chain(erase_subtree.iter().filter(|_| erase))
                .any(|root| same(&here, root))
        {
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
