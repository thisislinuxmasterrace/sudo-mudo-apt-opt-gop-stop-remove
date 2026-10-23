//! the hard link rule: under root, a file with more than one name is refused unless the user allows it.

use std::fs;
use std::io;
use std::path::Path;

#[cfg(unix)]
use nix::unistd::geteuid;

use super::Options;
use super::mount::link_count;

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

const HARD_LINK_REFUSAL: &str = "hard-linked file under root: other names may point to the same data, use --i-know-what-i-am-doing to erase it";

/// applies `hard_link_rule` before the file is touched. `shown` is the path reported to the user.
pub(super) fn check_hard_links(shown: &Path, hard_linked: bool, opts: &Options) -> io::Result<()> {
    match hard_link_rule(running_as_root(), hard_linked, opts.allow_protected) {
        HardLinkRule::Erase => Ok(()),
        HardLinkRule::EraseWithWarning => {
            eprintln!(
                "{}: warning: hard-linked file under root, erasing it because --i-know-what-i-am-doing was given",
                shown.display()
            );
            Ok(())
        }
        HardLinkRule::Refuse => Err(io::Error::other(HARD_LINK_REFUSAL)),
    }
}

/// the refusal of `check_hard_links` for a file named on the command line, decided before the
/// confirmation. a directory is not checked here: its files are checked when the walk reaches them.
pub(crate) fn validate_hard_link(path: &Path, allow: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if !meta.file_type().is_file() {
        return Ok(());
    }
    match hard_link_rule(running_as_root(), link_count(&meta) > 1, allow) {
        HardLinkRule::Refuse => Err(format!("{}: {HARD_LINK_REFUSAL}", path.display())),
        HardLinkRule::Erase | HardLinkRule::EraseWithWarning => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
