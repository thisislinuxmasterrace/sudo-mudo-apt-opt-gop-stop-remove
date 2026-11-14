use std::path::PathBuf;
use std::sync::OnceLock;

use nix::unistd::{Uid, User, geteuid};

static SUDO_HOME: OnceLock<PathBuf> = OnceLock::new();

fn invoking_user() -> Result<User, String> {
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("missing or invalid sudo user id")?;
    let _gid = std::env::var("SUDO_GID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("missing or invalid sudo group id")?;
    let name = std::env::var("SUDO_USER").map_err(|_| "missing sudo user name")?;
    let user = User::from_uid(Uid::from_raw(uid))
        .map_err(|err| format!("cannot resolve sudo user: {err}"))?
        .ok_or("sudo user is not in the account database")?;
    if user.name != name || !user.dir.is_absolute() {
        return Err("sudo user does not match the account database".to_owned());
    }
    Ok(user)
}

pub(super) fn check() -> Result<(), String> {
    if !geteuid().is_root() {
        return Err("sudo must run the command as root".to_owned());
    }
    let user = invoking_user()?;
    SUDO_HOME
        .set(user.dir)
        .map_err(|_| "sudo invocation was already checked".to_owned())
}

/// use the account database because sudo normally replaces the invoking user's home environment.
pub(crate) fn sudo_home() -> Option<PathBuf> {
    SUDO_HOME.get().cloned()
}
