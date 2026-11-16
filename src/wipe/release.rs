//! waits until the filesystem gives back the space of a finished pass. zfs frees the blocks of
//! deleted files in a later transaction group, seconds after the files are closed; a pass started
//! before that finds the volume still full.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

pub(super) const POLL: Duration = Duration::from_millis(250);
/// zfs commits a transaction group every 5 seconds by default; a large free can take several.
pub(super) const TIMEOUT: Duration = Duration::from_secs(60);

const MIB: u64 = 1024 * 1024;

/// the free space that counts as given back: metadata and other programs change the free space a
/// little between passes.
fn target(before: u64) -> u64 {
    before.saturating_sub(before / 100 + MIB)
}

/// returns once `free` reports about as much as `before`, the free space before the first pass.
/// `waiting` runs once if the space is not back at the first look. false when `timeout` passed
/// first; an unknown free space does not wait.
pub(super) fn wait_for_space(
    before: u64,
    mut free: impl FnMut() -> Option<u64>,
    mut waiting: impl FnMut(),
    poll: Duration,
    timeout: Duration,
) -> io::Result<bool> {
    let target = target(before);
    let start = Instant::now();
    let mut announced = false;
    loop {
        if free().is_none_or(|now| now >= target) {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        if !announced {
            waiting();
            announced = true;
        }
        crate::erase::check_interrupted()?;
        thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const GIB: u64 = 1024 * MIB;
    const QUICK: Duration = Duration::from_millis(1);

    #[test]
    fn space_that_is_back_does_not_wait() {
        let waited = Cell::new(false);

        let back = wait_for_space(GIB, || Some(GIB - MIB), || waited.set(true), QUICK, TIMEOUT);

        assert!(back.unwrap());
        assert!(!waited.get());
    }

    #[test]
    fn waits_until_the_space_comes_back() {
        let reads = Cell::new(0);
        let waited = Cell::new(0);
        let free = || {
            reads.set(reads.get() + 1);
            Some(if reads.get() < 4 { 0 } else { GIB })
        };

        let back = wait_for_space(GIB, free, || waited.set(waited.get() + 1), QUICK, TIMEOUT);

        assert!(back.unwrap());
        assert_eq!(reads.get(), 4);
        // the message is shown once, not for every look.
        assert_eq!(waited.get(), 1);
    }

    #[test]
    fn gives_up_after_the_timeout() {
        let back = wait_for_space(GIB, || Some(0), || {}, QUICK, Duration::from_millis(20));

        assert!(!back.unwrap());
    }

    #[test]
    fn unknown_free_space_does_not_wait() {
        assert!(wait_for_space(GIB, || None, || {}, QUICK, TIMEOUT).unwrap());
    }
}
