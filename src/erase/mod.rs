//! erases files, directories and symbolic links: the contents are overwritten, the entry is renamed to
//! a random hidden name and deleted. unix walks a tree through open directory descriptors (`unix`),
//! windows works by path (`win`).

#[cfg(unix)]
mod at;
mod hard_link;
mod interrupt;
mod mount;
mod overwrite;
mod pattern;
mod report;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix;
mod walk;
#[cfg(windows)]
mod win;

pub(crate) use hard_link::{running_as_root, validate_hard_link};
pub use interrupt::InterruptGuard;
#[cfg(windows)]
pub(crate) use interrupt::raise_interrupt;
pub(crate) use interrupt::{check_interrupted, interrupted};
#[cfg(target_os = "linux")]
pub(crate) use mount::unescape_mountinfo;
pub(crate) use mount::{device, is_mount_boundary, link_count};
pub(crate) use pattern::{Pattern, fill, pattern_for_pass};
#[cfg(unix)]
pub use unix::erase_path;
pub(crate) use walk::is_excluded;
#[cfg(windows)]
pub use win::erase_path;

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
    /// reads the file back after the last pass and compares it with what was written; a mismatch keeps the file.
    pub verify: bool,
}
