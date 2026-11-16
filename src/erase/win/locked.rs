//! windows erase of a file that another process holds open: list the holders, optionally close them,
//! and try again.

use std::io;
use std::path::Path;

use crate::erase::Options;
use crate::erase::interrupt::check_interrupted;
use crate::platform::windows;
use crate::prompt;

use super::file::{ReadOnly, erase_open, open_for_overwrite};

/// why a locked file has no listed holder. code 5 is also an acl or delete-pending state, so it
/// does not claim a lock; 32 and 33 are sharing and lock violations, held by something unlisted.
fn no_holder_reason(err: &io::Error) -> &'static str {
    match err.raw_os_error() {
        Some(32 | 33) => {
            "the file is in use, but no process is listed (a service, driver or antivirus may hold it)"
        }
        _ => {
            "no lock holder was listed, so the file may be pending deletion or its permissions may deny access"
        }
    }
}

/// handles a file that another process holds open. the restart manager session that lists the
/// holders stays open until they are closed, so only the processes that were listed are closed.
pub(super) fn erase_locked(
    path: &Path,
    opts: &Options,
    err: io::Error,
    keep: &mut ReadOnly,
) -> io::Result<()> {
    let (listing, session) = match windows::holder_session(path) {
        Ok((holders, session)) => (Ok(holders), Some(session)),
        Err(list_err) => (Err(list_err), None),
    };
    erase_locked_with(
        path,
        opts,
        err,
        listing,
        |shown| match &session {
            Some(session) => session.shut_down(shown),
            None => Err(io::Error::other("no lock holder session")),
        },
        keep,
    )
}

/// `close_holders` gets the holders that were shown to the user and closes exactly those.
pub(super) fn erase_locked_with(
    path: &Path,
    opts: &Options,
    err: io::Error,
    listing: io::Result<Vec<windows::Holder>>,
    close_holders: impl FnOnce(&[windows::Holder]) -> io::Result<()>,
    keep: &mut ReadOnly,
) -> io::Result<()> {
    let holders = listing.as_deref().unwrap_or_default();
    for holder in holders {
        crate::ui::message!(
            "{}: locked by {} (pid {})",
            crate::ui::shown(path),
            holder.name,
            holder.pid
        );
    }

    if opts.kill_holders && !holders.is_empty() {
        if !prompt::confirmed_action(
            || {
                if opts.force {
                    Ok(true)
                } else {
                    prompt::ask_interruptible(
                        "close these processes to unlock the file? unsaved data in them is lost",
                    )
                }
            },
            check_interrupted,
            || {
                crate::ui::message!(
                    "warning: closing {} process(es) may lose unsaved data and stop services",
                    holders.len()
                );
                close_holders(holders)
            },
        )? {
            return Err(io::Error::other("skipped, file is still locked"));
        }

        match open_for_overwrite(path) {
            Ok(file) => return erase_open(path, file, opts, keep),
            Err(retry_err) => {
                // the forced shutdown can leave holders or let new ones open the file: list them again.
                let remaining = windows::holders(path);
                for holder in remaining.as_deref().unwrap_or_default() {
                    crate::ui::message!(
                        "{}: still locked by {} (pid {})",
                        crate::ui::shown(path),
                        holder.name,
                        holder.pid
                    );
                }
                return Err(match &remaining {
                    Ok(list) if list.is_empty() => {
                        io::Error::other(format!("{retry_err}; {}", no_holder_reason(&retry_err)))
                    }
                    Ok(_) => {
                        io::Error::other(format!("{retry_err}; still locked after closing holders"))
                    }
                    Err(list_err) => io::Error::other(format!(
                        "{retry_err}; could not list lock holders: {list_err}"
                    )),
                });
            }
        }
    }

    Err(match &listing {
        Ok(list) if list.is_empty() => {
            io::Error::other(format!("{err}; {}", no_holder_reason(&err)))
        }
        Ok(_) => io::Error::other(format!("{err}; use --kill-holders to erase it anyway")),
        Err(list_err) => {
            io::Error::other(format!("{err}; could not list lock holders: {list_err}"))
        }
    })
}
