//! inventory for hard links whose names are all selected inside one unix tree.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::fcntl::{AtFlags, OFlag};
use nix::sys::stat::{FileStat, SFlag, fstatat};

use super::at::{file_kind, open_checked};
use super::mount::{MountTable, is_boundary};
use super::walk::is_excluded;

type Identity = (u64, u64);

#[derive(Default)]
pub(super) struct TreeLinks(RefCell<HashMap<Identity, u64>>);

#[allow(clippy::unnecessary_cast)]
fn identity(st: &FileStat) -> Identity {
    (st.st_dev as u64, st.st_ino as u64)
}

impl TreeLinks {
    pub(super) fn collect(
        parent: &File,
        name: &CStr,
        st: &FileStat,
        real: &Path,
        excludes: &[String],
        mounts: &MountTable,
    ) -> io::Result<Self> {
        let mut counts = HashMap::new();
        let mut seen = HashSet::new();
        // stop before erasing when the inventory cannot prove which entries were selected.
        collect(
            parent,
            name,
            st,
            real,
            Path::new(""),
            excludes,
            mounts,
            &mut seen,
            &mut counts,
        )?;
        Ok(Self(RefCell::new(counts)))
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn contains_all(&self, st: &FileStat) -> bool {
        self.0.borrow().get(&identity(st)).copied() == Some(st.st_nlink as u64)
    }

    pub(super) fn removed(&self, st: &FileStat) {
        if let Some(count) = self.0.borrow_mut().get_mut(&identity(st)) {
            *count = count.saturating_sub(1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect(
    parent: &File,
    name: &CStr,
    st: &FileStat,
    real: &Path,
    rel: &Path,
    excludes: &[String],
    mounts: &MountTable,
    seen: &mut HashSet<Identity>,
    counts: &mut HashMap<Identity, u64>,
) -> io::Result<()> {
    super::interrupt::check_interrupted()?;
    super::check_depth(rel)
        .map_err(|err| io::Error::other(format!("{}: {err}", crate::ui::shown(real))))?;
    if file_kind(st) == SFlag::S_IFREG {
        if st.st_nlink > 1 {
            *counts.entry(identity(st)).or_default() += 1;
        }
    } else if file_kind(st) == SFlag::S_IFDIR && seen.insert(identity(st)) {
        let dir = open_checked(
            parent,
            name,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            st,
        )
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("{}: hard-link inventory: {err}", crate::ui::shown(real)),
            )
        })?;
        for name in super::unix::list_names(&dir)? {
            let entry = std::ffi::OsStr::from_bytes(name.to_bytes());
            let rel = rel.join(entry);
            if is_excluded(excludes, &rel) {
                continue;
            }
            let real = real.join(entry);
            let child = fstatat(&dir, name.as_c_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
            if !is_boundary(&child, st, &real, mounts)? {
                collect(
                    &dir, &name, &child, &real, &rel, excludes, mounts, seen, counts,
                )?;
            }
        }
    }
    Ok(())
}
