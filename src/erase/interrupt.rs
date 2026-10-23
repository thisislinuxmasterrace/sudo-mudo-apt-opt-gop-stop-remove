//! ctrl+c and sigterm during an erase: the flag the write loops check, and the guard that routes the
//! signals to it while an erase runs.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use crate::platform::windows;
#[cfg(unix)]
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

/// set by the interrupt handlers on the first ctrl+c or sigterm while an erase runs.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// records an interrupt and returns true when one was already recorded. it runs inside signal and
/// console handlers, so it only does an atomic swap.
pub(crate) fn raise_interrupt() -> bool {
    record_interrupt(&INTERRUPTED)
}

/// sets `flag` and returns true when it was already set, which makes the caller's signal the second one.
fn record_interrupt(flag: &AtomicBool) -> bool {
    flag.swap(true, Ordering::SeqCst)
}

pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// returned by the write loops once an interrupt was recorded. it is passed up unchanged, so the
/// walk stops and the partly written file keeps its name.
pub(crate) fn check_interrupted() -> io::Result<()> {
    if interrupted() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "interrupted by signal",
        ));
    }
    Ok(())
}

/// routes ctrl+c and sigterm to the interrupt flag until the guard is dropped, so the handlers only
/// cover the erase itself and not the questions before it.
#[cfg(unix)]
pub struct InterruptGuard {
    previous: Vec<(Signal, SigAction)>,
}

#[cfg(unix)]
impl InterruptGuard {
    pub fn install() -> io::Result<Self> {
        let action = SigAction::new(
            SigHandler::Handler(on_unix_interrupt),
            SaFlags::empty(),
            SigSet::empty(),
        );
        let mut guard = Self {
            previous: Vec::new(),
        };
        for signal in [Signal::SIGINT, Signal::SIGTERM] {
            // a signal the process was started with ignored stays ignored.
            if ignores(signal)? {
                continue;
            }
            // safety: on_unix_interrupt only does an atomic swap and calls _exit, both allowed in a signal handler.
            let old = unsafe { sigaction(signal, &action) }.map_err(io::Error::from)?;
            guard.previous.push((signal, old));
        }
        Ok(guard)
    }
}

/// true when `signal` is currently ignored. a null action only reads the disposition, so nothing changes.
#[cfg(unix)]
fn ignores(signal: Signal) -> io::Result<bool> {
    let mut current = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // safety: with a null new action, sigaction only writes the current action into `current`.
    let ret = unsafe {
        libc::sigaction(
            signal as libc::c_int,
            std::ptr::null(),
            current.as_mut_ptr(),
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    // safety: sigaction returned 0, so it has written the whole struct.
    let current = unsafe { current.assume_init() };
    Ok(current.sa_sigaction == libc::SIG_IGN)
}

#[cfg(unix)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        for (signal, action) in &self.previous {
            // safety: the action was returned by sigaction for this signal, so restoring it is valid.
            let _ = unsafe { sigaction(*signal, action) };
        }
    }
}

#[cfg(unix)]
extern "C" fn on_unix_interrupt(_signal: libc::c_int) {
    if raise_interrupt() {
        // safety: _exit is async-signal-safe and does not return.
        unsafe { libc::_exit(130) }
    }
}

/// console ctrl+c, ctrl+break and closing the console set the interrupt flag while the guard lives.
#[cfg(windows)]
pub struct InterruptGuard;

#[cfg(windows)]
impl InterruptGuard {
    pub fn install() -> io::Result<Self> {
        windows::set_interrupt_handler(true)?;
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        let _ = windows::set_interrupt_handler(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_signal_is_the_call_that_finds_the_flag_set() {
        let flag = AtomicBool::new(false);

        assert!(!record_interrupt(&flag));
        assert!(record_interrupt(&flag));
    }
}
