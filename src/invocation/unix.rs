use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use nix::unistd::{Uid, User, geteuid};

static SUDO_HOME: OnceLock<PathBuf> = OnceLock::new();

/// the invoking user's ids and groups, checked together with the account database.
struct SudoIds {
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
}

static SUDO_IDS: OnceLock<SudoIds> = OnceLock::new();

fn invoking_user() -> Result<(User, u32), String> {
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("missing or invalid sudo user id")?;
    let gid = std::env::var("SUDO_GID")
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
    Ok((user, gid))
}

pub(super) fn check() -> Result<(), String> {
    if !geteuid().is_root() {
        return Err("sudo must run the command as root".to_owned());
    }
    let (user, gid) = invoking_user()?;
    // fewer groups mean fewer rights, so a failed lookup leaves the primary group only.
    let groups = group_list(&user.name, gid).unwrap_or_else(|| vec![gid]);
    let _ = SUDO_IDS.set(SudoIds {
        uid: user.uid.as_raw(),
        gid,
        groups,
    });
    SUDO_HOME
        .set(user.dir)
        .map_err(|_| "sudo invocation was already checked".to_owned())
}

#[cfg(not(target_vendor = "apple"))]
fn group_list(name: &str, gid: u32) -> Option<Vec<u32>> {
    let name = CString::new(name).ok()?;
    let groups = nix::unistd::getgrouplist(&name, nix::unistd::Gid::from_raw(gid)).ok()?;
    Some(groups.into_iter().map(nix::unistd::Gid::as_raw).collect())
}

/// nix has no `getgrouplist` on apple targets, where the group ids are `c_int`. macos `setgroups`
/// rejects more than 16 groups; that makes the spawn fail, so the fact stays unknown, which is safe.
#[cfg(target_vendor = "apple")]
fn group_list(name: &str, gid: u32) -> Option<Vec<u32>> {
    let name = CString::new(name).ok()?;
    let mut count: libc::c_int = 32;
    // the list can grow between calls, so the size is retried a bounded number of times.
    for _ in 0..4 {
        let mut buffer = vec![0 as libc::c_int; usize::try_from(count).ok()?];
        // safety: name is NUL-terminated, buffer holds `count` ints and count points to a live
        // local; the call writes at most `count` entries and updates count.
        let rc = unsafe {
            libc::getgrouplist(
                name.as_ptr(),
                gid as libc::c_int,
                buffer.as_mut_ptr(),
                &mut count,
            )
        };
        if rc >= 0 {
            buffer.truncate(usize::try_from(count).ok()?);
            return Some(buffer.into_iter().map(|g| g as u32).collect());
        }
        count = count.checked_mul(2)?.max(count.checked_add(1)?);
    }
    None
}

/// makes `command` run as the invoking sudo user: the child sets its groups, then its gid, then
/// its uid before exec. returns false when that user is unknown, and the command must not be run
/// then: it would run as root.
pub(crate) fn run_as_sudo_user(command: &mut Command) -> bool {
    let Some(ids) = SUDO_IDS.get() else {
        return false;
    };
    // safety: the closure runs in the forked child before exec and only calls the async-signal-safe
    // setgroups, setgid and setuid on data that already exists; it allocates nothing. the order
    // matters: the groups and the gid can no longer be changed after the uid is dropped. if any call
    // fails, spawn fails and the command never runs with root rights.
    unsafe {
        command.pre_exec(|| {
            let ok = libc::setgroups(ids.groups.len() as _, ids.groups.as_ptr()) == 0
                && libc::setgid(ids.gid) == 0
                && libc::setuid(ids.uid) == 0;
            if ok {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    true
}

/// use the account database because sudo normally replaces the invoking user's home environment.
pub(crate) fn sudo_home() -> Option<PathBuf> {
    SUDO_HOME.get().cloned()
}
