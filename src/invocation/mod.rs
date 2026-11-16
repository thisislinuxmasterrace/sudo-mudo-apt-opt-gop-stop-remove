//! cooperative invocation guard against accidental use; not an authentication boundary.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{run_as_sudo_user, sudo_home};
#[cfg(windows)]
pub(crate) use windows::invoking_home;

pub(crate) fn check() -> Result<(), String> {
    #[cfg(unix)]
    let result = unix::check();
    #[cfg(windows)]
    let result = windows::check();
    result.map_err(|reason| {
        format!("run with sudo: sudo mudo apt opt gop stop remove <paths>; {reason}")
    })
}
