//! windows protection checks: the directories a recursive erase must not enter, compared by identity,
//! and the verbatim-prefix handling the path checks share.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;

use crate::platform::windows::{FileId, file_id, file_id_of, is_reparse_point, open_entry};

/// `\\?\C:\dir` as `C:\dir` and `\\?\UNC\server\share\dir` as `\\server\share\dir`, so canonical paths
/// compare with the spelled-out roots. it works on components, so a name that is not valid unicode
/// keeps its protection.
pub(super) fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return path.to_path_buf();
    };
    let head = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => OsString::from(format!("{}:", char::from(letter))),
        Prefix::VerbatimUNC(server, share) => {
            let mut head = OsString::from(r"\\");
            head.push(server);
            head.push(r"\");
            head.push(share);
            head
        }
        _ => return path.to_path_buf(),
    };
    let mut stripped = PathBuf::from(head);
    stripped.extend(parts);
    stripped
}

/// the directories a recursive erase refuses to enter, by volume and file index: the protected system
/// roots and the home directories. the name that reaches them does not matter.
pub(crate) struct Protected(Vec<FileId>);

impl Protected {
    /// the identities of the directories at `roots`; one that cannot be opened is left out.
    pub(crate) fn of(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self(
            roots
                .into_iter()
                .filter_map(|root| file_id(&root, true).ok())
                .collect(),
        )
    }

    /// refuses `dir` when it is a protected directory, unless `allow`. `dir` is opened itself and not
    /// followed, so a link to a protected directory is not one. a directory that is a reparse point
    /// is refused whatever `allow` says: listing it would follow the link, and the walk never enters links.
    pub(crate) fn check_dir(&self, dir: &Path, allow: bool) -> io::Result<()> {
        let handle = open_entry(dir)?;
        if is_reparse_point(&handle)? {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "directory is a link or reparse point, it is kept",
            ));
        }
        if allow || !self.0.contains(&file_id_of(&handle)?) {
            return Ok(());
        }
        Err(io::Error::other(
            "refusing to change a protected system path (--i-know-what-i-am-doing allows it)",
        ))
    }
}

/// catches protected directories reached under another spelling or name, such as `\\localhost\C$\Windows`
/// or a junction path that does not start with the root. the entry itself is compared with both kinds
/// of roots, its ancestors only with the roots that protect everything below them. like on unix, a root
/// or an ancestor that cannot be opened is skipped, so an unreadable directory never refuses a path by itself.
pub(super) fn is_protected_by_identity(
    path: &Path,
    subtree: &[PathBuf],
    exact: &[PathBuf],
) -> bool {
    let ids = |roots: &[PathBuf]| -> Vec<FileId> {
        roots
            .iter()
            .filter_map(|root| file_id(root, true).ok())
            .collect()
    };
    let (subtree, exact) = (ids(subtree), ids(exact));
    // not followed: a link to a protected directory is not the directory and may be removed.
    if let Ok(me) = file_id(path, false)
        && (subtree.contains(&me) || exact.contains(&me))
    {
        return true;
    }
    path.ancestors()
        .skip(1)
        .filter_map(|ancestor| file_id(ancestor, true).ok())
        .any(|id| subtree.contains(&id))
}

/// the protected directories of this process, computed once: the system roots, `%USERPROFILE%` and the
/// profile of the account that started sudo when it is known.
pub(crate) fn protected() -> &'static Protected {
    static PROTECTED: OnceLock<Protected> = OnceLock::new();
    PROTECTED.get_or_init(|| {
        let mut roots = super::protected_roots();
        roots.extend(super::exact_roots());
        roots.extend(super::erase_only_exact_roots());
        roots.extend(super::erase_only_subtree_roots());
        roots.extend(std::env::home_dir());
        roots.extend(crate::invocation::invoking_home());
        Protected::of(roots)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::ffi::OsStringExt;

    fn not_unicode() -> OsString {
        let name = OsString::from_wide(&[0x61, 0xD800, 0x62]);
        assert!(name.to_str().is_none());
        name
    }

    #[test]
    fn verbatim_prefix_is_stripped_from_names_that_are_not_unicode() {
        let name = not_unicode();
        let mut verbatim = OsString::from(r"\\?\C:\Windows\");
        verbatim.push(&name);
        let mut expected = OsString::from(r"C:\Windows\");
        expected.push(&name);

        let stripped = strip_verbatim_prefix(Path::new(&verbatim));

        assert_eq!(stripped, PathBuf::from(expected));
        assert!(stripped.starts_with(r"C:\Windows"));
    }

    #[test]
    fn verbatim_unc_prefix_becomes_a_plain_unc_path() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\dir\file")),
            PathBuf::from(r"\\server\share\dir\file")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\dir")),
            PathBuf::from(r"C:\dir")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"C:\dir")),
            PathBuf::from(r"C:\dir")
        );
    }

    #[test]
    fn non_unicode_name_below_a_system_folder_is_protected() {
        let root = fs::canonicalize(std::env::var_os("SystemRoot").unwrap()).unwrap();

        assert!(crate::safety::is_protected(&root.join(not_unicode())));
    }

    #[test]
    fn protected_directory_is_refused_by_identity_unless_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let guarded = dir.path().join("guarded");
        let other = dir.path().join("other");
        fs::create_dir(&guarded).unwrap();
        fs::create_dir(&other).unwrap();
        let protected = Protected::of([guarded.clone(), dir.path().join("missing")]);

        assert!(protected.check_dir(&guarded, false).is_err());
        assert!(protected.check_dir(&other, false).is_ok());
        assert!(protected.check_dir(&guarded, true).is_ok());
        // another spelling of the same directory is still the same directory.
        let spelled = other.join("..").join("guarded");
        assert!(protected.check_dir(&spelled, false).is_err());
    }

    #[test]
    fn directory_that_is_a_reparse_point_is_refused_even_when_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::create_dir(&target).unwrap();
        // a junction needs no privilege, unlike a directory symlink.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let protected = Protected::of([]);

        assert!(protected.check_dir(&link, false).is_err());
        assert!(protected.check_dir(&link, true).is_err());
        assert!(protected.check_dir(&target, true).is_ok());
    }

    #[test]
    fn identity_covers_other_spellings_of_protected_roots() {
        let dir = tempfile::tempdir().unwrap();
        let subtree = dir.path().join("subtree");
        let exact = dir.path().join("exact");
        let other = dir.path().join("other");
        for sub in [&subtree, &exact, &other] {
            fs::create_dir(sub).unwrap();
        }
        fs::create_dir(subtree.join("below")).unwrap();
        fs::create_dir(exact.join("below")).unwrap();
        let (subtree_roots, exact_roots) = (vec![subtree.clone()], vec![exact.clone()]);
        // spellings that do not start with the roots.
        let alias = |root: &str| dir.path().join("other").join("..").join(root);

        assert!(is_protected_by_identity(
            &alias("subtree"),
            &subtree_roots,
            &exact_roots
        ));
        assert!(is_protected_by_identity(
            &alias("subtree").join("below"),
            &subtree_roots,
            &exact_roots
        ));
        assert!(is_protected_by_identity(
            &alias("exact"),
            &subtree_roots,
            &exact_roots
        ));
        // an exact root protects only itself.
        assert!(!is_protected_by_identity(
            &alias("exact").join("below"),
            &subtree_roots,
            &exact_roots
        ));
        assert!(!is_protected_by_identity(
            &alias("other"),
            &subtree_roots,
            &exact_roots
        ));
        // a root that cannot be opened protects nothing and does not break the check.
        assert!(!is_protected_by_identity(
            &other,
            &[dir.path().join("missing")],
            &[]
        ));
    }
}
