//! progress line on stderr for the long parts of an erase and of `--wipe-free`.
//!
//! the line is drawn in place with `\r`, at most twice a second. it is drawn only when stderr is a
//! terminal and `-q` was not given. nothing here changes what is erased or the exit code.

use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::preview::human_bytes;

/// the shortest time between two drawings of the line.
const INTERVAL: Duration = Duration::from_millis(500);

/// progress of one operation. `total` is the number of bytes when it is known in advance.
pub struct Progress {
    out: Option<Box<dyn Write>>,
    total: Option<u64>,
    done: u64,
    started: Instant,
    last_draw: Option<Instant>,
    /// length of the last line drawn, so a shorter line can cover it.
    width: usize,
    /// a line is on screen without its newline.
    drawn: bool,
}

/// true when progress is drawn: not quiet, and stderr is a terminal.
fn wanted(quiet: bool, terminal: bool) -> bool {
    !quiet && terminal
}

impl Progress {
    pub fn new(total: Option<u64>, quiet: bool) -> Self {
        let out: Option<Box<dyn Write>> = wanted(quiet, io::stderr().is_terminal())
            .then(|| Box::new(io::stderr()) as Box<dyn Write>);
        Self::with_output(total, out, Instant::now())
    }

    fn with_output(total: Option<u64>, out: Option<Box<dyn Write>>, started: Instant) -> Self {
        Self {
            out,
            total,
            done: 0,
            started,
            last_draw: None,
            width: 0,
            drawn: false,
        }
    }

    /// counts `bytes` more, and draws the line when the last drawing is at least `INTERVAL` ago.
    pub fn add(&mut self, bytes: u64) {
        self.add_at(bytes, Instant::now());
    }

    fn add_at(&mut self, bytes: u64, now: Instant) {
        self.done = self.done.saturating_add(bytes);
        if self.out.is_none() {
            return;
        }
        let since = self.last_draw.unwrap_or(self.started);
        if now.saturating_duration_since(since) >= INTERVAL {
            self.draw(now, false);
        }
    }

    /// ends the line with the current state and a newline, if a line is on screen. it is called
    /// before other output that must not share the line, and when the operation ends. a second call
    /// prints nothing, because the line has already ended.
    pub fn finish(&mut self) {
        if self.drawn {
            self.draw(Instant::now(), true);
        }
    }

    fn draw(&mut self, now: Instant, end: bool) {
        let text = self.line(now);
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let pad = " ".repeat(self.width.saturating_sub(text.len()));
        // a failed write to stderr is ignored: progress must not change the outcome of the erase.
        let _ = write!(out, "\r{text}{pad}");
        if end {
            let _ = writeln!(out);
        }
        let _ = out.flush();
        self.last_draw = Some(now);
        self.drawn = !end;
        self.width = if end { 0 } else { text.len() };
    }

    fn line(&self, now: Instant) -> String {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f64();
        let speed = if elapsed > 0.0 {
            format!(", {}/s", human_bytes((self.done as f64 / elapsed) as u64))
        } else {
            String::new()
        };
        match self.total {
            Some(total) if total > 0 => {
                let percent = (u128::from(self.done) * 100 / u128::from(total)).min(100);
                format!(
                    "progress: {percent}% {} of {}{speed}",
                    human_bytes(self.done),
                    human_bytes(total)
                )
            }
            _ => format!("progress: {} written{speed}", human_bytes(self.done)),
        }
    }
}

/// the line always ends, even when the operation returns early or panics.
impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// a writer that keeps what is written, so a test can read it after the progress is done.
    #[derive(Clone, Default)]
    struct Capture(Rc<RefCell<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture(total: Option<u64>, started: Instant) -> (Progress, Capture) {
        let out = Capture::default();
        let progress = Progress::with_output(total, Some(Box::new(out.clone())), started);
        (progress, out)
    }

    #[test]
    fn known_total_shows_percent_bytes_and_speed() {
        let start = Instant::now();
        let (mut progress, _) = capture(Some(2 * MIB), start);
        progress.done = MIB;

        assert_eq!(
            progress.line(start + Duration::from_secs(2)),
            "progress: 50% 1.0 mib of 2.0 mib, 512.0 kib/s"
        );
    }

    #[test]
    fn unknown_total_shows_bytes_and_speed() {
        let start = Instant::now();
        let (mut progress, _) = capture(None, start);
        progress.done = 3 * MIB;

        assert_eq!(
            progress.line(start + Duration::from_secs(2)),
            "progress: 3.0 mib written, 1.5 mib/s"
        );
    }

    #[test]
    fn percent_is_capped_at_one_hundred() {
        let start = Instant::now();
        let (mut progress, _) = capture(Some(MIB), start);
        progress.done = 2 * MIB;

        assert!(progress.line(start).starts_with("progress: 100% "));
    }

    #[test]
    fn no_speed_is_shown_before_any_time_has_passed() {
        let start = Instant::now();
        let (progress, _) = capture(Some(MIB), start);

        assert_eq!(progress.line(start), "progress: 0% 0 b of 1.0 mib");
    }

    #[test]
    fn drawing_is_throttled_to_the_interval() {
        let start = Instant::now();
        let (mut progress, out) = capture(Some(10 * MIB), start);

        progress.add_at(MIB, start + Duration::from_millis(100));
        assert!(out.text().is_empty());

        progress.add_at(MIB, start + Duration::from_millis(600));
        assert_eq!(out.text().matches('\r').count(), 1);

        progress.add_at(MIB, start + Duration::from_millis(700));
        assert_eq!(out.text().matches('\r').count(), 1);

        progress.add_at(MIB, start + Duration::from_millis(1100));
        assert_eq!(out.text().matches('\r').count(), 2);
    }

    #[test]
    fn finish_ends_the_line_only_when_one_was_drawn() {
        let start = Instant::now();
        let (mut progress, out) = capture(Some(MIB), start);

        progress.finish();
        assert!(out.text().is_empty());

        progress.add_at(MIB, start + Duration::from_secs(1));
        progress.finish();

        let text = out.text();
        assert!(text.contains("progress: 100%"));
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn a_shorter_line_is_padded_to_cover_the_longer_one() {
        let start = Instant::now();
        let (mut progress, out) = capture(None, start);

        progress.add_at(1024 * MIB, start + Duration::from_secs(1));
        progress.add_at(0, start + Duration::from_secs(2_000_000));

        let text = out.text();
        let parts: Vec<&str> = text.split('\r').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[2].trim_end().len() < parts[1].len());
        assert_eq!(parts[2].len(), parts[1].len());
    }

    #[test]
    fn nothing_is_drawn_without_a_terminal() {
        assert!(!wanted(false, false));
        assert_eq!(
            Progress::new(Some(MIB), false).out.is_some(),
            io::stderr().is_terminal()
        );
    }

    #[test]
    fn nothing_is_drawn_when_quiet() {
        assert!(wanted(false, true));
        assert!(!wanted(true, true));
        assert!(!wanted(true, false));
        assert!(Progress::new(Some(MIB), true).out.is_none());
    }

    #[test]
    fn finish_is_idempotent_and_drop_ends_the_line() {
        let start = Instant::now();
        let (mut progress, out) = capture(Some(MIB), start);
        progress.add_at(MIB, start + Duration::from_secs(1));
        progress.finish();
        progress.finish();
        assert_eq!(out.text().matches('\n').count(), 1);
        drop(progress);
        assert_eq!(out.text().matches('\n').count(), 1);

        let (mut dropped, out) = capture(Some(MIB), start);
        dropped.add_at(MIB, start + Duration::from_secs(1));
        drop(dropped);
        assert!(out.text().ends_with('\n'));
    }

    #[test]
    fn progress_without_output_still_counts() {
        let start = Instant::now();
        let mut progress = Progress::with_output(Some(MIB), None, start);

        progress.add_at(MIB, start + Duration::from_secs(5));
        progress.finish();

        assert_eq!(progress.done, MIB);
        assert!(progress.out.is_none());
    }
}
