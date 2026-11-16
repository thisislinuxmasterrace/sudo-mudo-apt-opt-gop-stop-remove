//! operations that have no portable equivalent.

#[cfg(windows)]
mod console;
#[cfg(windows)]
pub mod windows;
