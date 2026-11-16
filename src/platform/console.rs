//! the console handler that turns ctrl+c, ctrl+break and closing the console into the erase interrupt
//! flag, and the wait that lets the erase stop before windows ends the process.

use std::io;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows_sys::core::BOOL;

use crate::erase;

/// how long the console handler waits for the erase to stop; windows ends the process after 5 seconds.
const STOP_WAIT: Duration = Duration::from_secs(4);

/// registers or removes the console handler that turns ctrl+c, ctrl+break and closing the console
/// into the erase interrupt flag. removing it also tells a waiting handler that the erase has stopped.
pub fn set_interrupt_handler(on: bool) -> io::Result<()> {
    STOPPED.set(!on);
    // safety: console_handler is a plain function that lives for the whole process. removing it
    // passes the same routine with add set to false.
    let ok = unsafe { SetConsoleCtrlHandler(Some(console_handler), BOOL::from(on)) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// set once the erase has stopped and its last object is restored and logged.
static STOPPED: StopSignal = StopSignal::new();

/// a flag one thread waits on, with a time limit, until another sets it.
struct StopSignal {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl StopSignal {
    const fn new() -> Self {
        Self {
            stopped: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn set(&self, value: bool) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = value;
        self.changed.notify_all();
    }

    /// waits until the flag is set or `limit` passes, and returns the flag.
    fn wait(&self, limit: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, limit, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }
}

/// runs on a thread that windows creates. the first event sets the flag; a second one exits at once,
/// because a write may be half-way through a file. windows ends the process as soon as the handler
/// returns for closing the console, logging off or shutting down, so those events wait (below the
/// five-second limit) until the erase has stopped.
extern "system" fn console_handler(ctrl_type: u32) -> BOOL {
    let ends_process = matches!(
        ctrl_type,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    );
    if !ends_process && !matches!(ctrl_type, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        return 0;
    }
    if erase::raise_interrupt() {
        std::process::exit(130);
    }
    if ends_process {
        STOPPED.wait(STOP_WAIT);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn stop_signal_wakes_the_waiter_and_gives_up_after_the_limit() {
        let signal = StopSignal::new();
        assert!(!signal.wait(Duration::from_millis(20)));

        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                signal.set(true);
            });
            assert!(signal.wait(Duration::from_secs(30)));
        });
        signal.set(false);
        assert!(!signal.wait(Duration::from_millis(20)));
    }
}
