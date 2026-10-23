//! `--log`: one json line per object the erase handles. file contents are never written here.

use std::cell::{Cell, RefCell};

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json;

/// the kind of object a line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Link,
    /// fifos, sockets and devices, and entries that could not be inspected.
    Other,
    /// a whole disk erased as a block device (linux only for now).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Device,
    /// a partition erased as a block device (linux only for now).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Partition,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Dir => "dir",
            Kind::Link => "link",
            Kind::Other => "other",
            Kind::Device => "device",
            Kind::Partition => "partition",
        }
    }
}

/// what the erase did with the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    Skipped,
    Kept,
    Refused,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Skipped => "skipped",
            Outcome::Kept => "kept",
            Outcome::Refused => "refused",
        }
    }
}

/// where the lines go: a file in a run, memory in the tests.
pub(crate) trait Sink: Write {
    /// cuts the output back to `len` bytes.
    fn cut_to(&mut self, len: u64) -> io::Result<()>;
}

impl Sink for File {
    fn cut_to(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)?;
        self.seek(SeekFrom::Start(len)).map(|_| ())
    }
}

/// identity of a directory or file: (device, inode) on unix, (volume serial, file index) on windows.
#[derive(Debug, PartialEq, Eq)]
struct Identity(u64, u64);

/// the open log of one run. each line is written and flushed on its own.
pub struct Log {
    path: PathBuf,
    /// the overwrite passes of the run. written for regular files and devices only.
    passes: u32,
    sink: RefCell<Box<dyn Sink>>,
    /// length of the complete lines written so far. a line that failed half way is cut back to it.
    len: Cell<u64>,
    failed: Cell<bool>,
    interrupted: Cell<bool>,
}

impl Log {
    /// creates the log. the file must not exist: an existing file is never overwritten.
    /// on unix it gets mode 0600, because the names in it can be sensitive; on windows the default.
    pub fn create(path: &Path, passes: u32) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        Ok(Self::new(path, passes, Box::new(options.open(path)?)))
    }

    pub(crate) fn new(path: &Path, passes: u32, sink: Box<dyn Sink>) -> Self {
        Self {
            path: path.to_path_buf(),
            passes,
            sink: RefCell::new(sink),
            len: Cell::new(0),
            failed: Cell::new(false),
            interrupted: Cell::new(false),
        }
    }

    /// true once a line could not be written. the erase then stops, so nothing is erased without its log.
    pub fn failed(&self) -> bool {
        self.failed.get()
    }

    /// appends one line and flushes it, so the lines written so far survive the process being killed.
    /// `detail` is the error or warning text, empty when there is none. the first write error is
    /// reported, the partly written line is cut off, and the log takes no more lines.
    pub fn record(&self, path: &Path, kind: Kind, size: u64, outcome: Outcome, detail: &str) {
        if self.failed.get() {
            return;
        }
        let passes = if matches!(kind, Kind::File | Kind::Device | Kind::Partition) {
            self.passes
        } else {
            0
        };
        let line = line(path, kind, size, passes, outcome, detail);
        let mut sink = self.sink.borrow_mut();
        match sink.write_all(line.as_bytes()).and_then(|()| sink.flush()) {
            Ok(()) => {
                let line_len = u64::try_from(line.len()).unwrap_or(u64::MAX);
                self.len.set(self.len.get().saturating_add(line_len));
            }
            Err(err) => {
                self.failed.set(true);
                let cut = sink.cut_to(self.len.get());
                let note = if cut.is_ok() {
                    ""
                } else {
                    "; the last line may be partly written"
                };
                eprintln!(
                    "{}: could not write the log {}: {err}{note}",
                    crate::NAME,
                    self.path.display()
                );
            }
        }
    }

    /// writes the line of the object an interrupt stopped. only the first one is written: the
    /// directories above it are stopped by the same interrupt, and their lines would repeat it.
    pub fn record_interrupted(&self, path: &Path, kind: Kind, size: u64) {
        if !self.interrupted.replace(true) {
            self.record(path, kind, size, Outcome::Error, "interrupted");
        }
    }
}

/// checks the log path before the confirmation, so a refusal costs no question. the file itself is
/// created later by `Log::create`, which fails if it exists by then, and `check_location` runs again.
pub fn check_path(log: &Path, targets: &[PathBuf]) -> Result<(), String> {
    if fs::symlink_metadata(log).is_ok() {
        return Err(format!(
            "{}: the log file already exists, it is never overwritten",
            log.display()
        ));
    }
    if log.file_name().is_none() {
        return Err(format!("{}: the log needs a file name", log.display()));
    }
    check_location(log, targets)
}

/// refuses a log that lies inside one of the targets, or whose directory is one of them. the
/// directories are compared by identity, so `./`, extra `/` and letter case do not get around it.
/// a target that is a link is erased as a link, nothing behind it is touched, so it is not compared.
pub fn check_location(log: &Path, targets: &[PathBuf]) -> Result<(), String> {
    let shown = log.display();
    let chain = log_dir_chain(log).map_err(|err| format!("{shown}: {err}"))?;
    for target in targets {
        let meta =
            fs::symlink_metadata(target).map_err(|err| format!("{}: {err}", target.display()))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let id = identity(target).map_err(|err| format!("{}: {err}", target.display()))?;
        if chain.contains(&id) {
            return Err(format!(
                "{shown}: the log would be erased with {}, choose another file",
                target.display()
            ));
        }
    }
    Ok(())
}

/// identities of the directory that holds the log and of all its ancestors. the path is canonical,
/// so the chain has no links in it.
fn log_dir_chain(log: &Path) -> io::Result<Vec<Identity>> {
    let parent = match log.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut chain = Vec::new();
    let mut current = Some(fs::canonicalize(parent)?);
    while let Some(dir) = current {
        chain.push(identity(&dir)?);
        current = dir.parent().map(Path::to_path_buf);
    }
    Ok(chain)
}

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;

    let meta = fs::metadata(path)?;
    Ok(Identity(meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn identity(path: &Path) -> io::Result<Identity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
    };

    // directories can only be opened with backup semantics.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // safety: the handle stays open while `file` lives, and `info` is a valid out pointer.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // safety: GetFileInformationByHandle returned nonzero, so it filled `info`.
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(Identity(u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(not(any(unix, windows)))]
fn identity(_path: &Path) -> io::Result<Identity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cannot compare file identities on this system",
    ))
}

fn line(path: &Path, kind: Kind, size: u64, passes: u32, outcome: Outcome, detail: &str) -> String {
    let mut out = String::from("{\"time\":");
    json::push_str(&mut out, &rfc3339_utc(now_secs()));
    out.push_str(",\"path\":");
    json::push_path(&mut out, path);
    out.push_str(",\"kind\":");
    json::push_str(&mut out, kind.name());
    out.push_str(",\"size\":");
    json::push_u64(&mut out, size);
    out.push_str(",\"passes\":");
    json::push_u64(&mut out, u64::from(passes));
    out.push_str(",\"result\":");
    json::push_str(&mut out, outcome.name());
    out.push_str(",\"detail\":");
    json::push_str(&mut out, detail);
    out.push_str("}\n");
    out
}

/// seconds since the epoch. a clock before 1970 counts as 0, so it is written as 1970-01-01T00:00:00Z.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// RFC 3339 in UTC with whole seconds, such as `2023-11-14T22:13:20Z`.
fn rfc3339_utc(secs: u64) -> String {
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// year, month and day (proleptic gregorian) of a day count since 1970-01-01. this is the
/// days-from-civil algorithm run backwards: the era is 400 years, so leap days are exact.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    // march is month 0, so the year changes after february.
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;
    use std::io::{self, Write};
    use std::rc::Rc;

    use super::Sink;

    /// lines kept in memory; the clones share the same text.
    #[derive(Clone, Default)]
    pub(crate) struct Memory(Rc<RefCell<Vec<u8>>>);

    impl Memory {
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for Memory {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink for Memory {
        fn cut_to(&mut self, len: u64) -> io::Result<()> {
            self.0
                .borrow_mut()
                .truncate(usize::try_from(len).unwrap_or(usize::MAX));
            Ok(())
        }
    }

    /// takes `room` bytes and then fails, the way a full disk does. with `room` 0 every write fails.
    pub(crate) struct Torn {
        pub(crate) memory: Memory,
        pub(crate) room: usize,
    }

    impl Write for Torn {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.room == 0 {
                return Err(io::Error::other("no space left on device"));
            }
            let taken = buf.len().min(self.room);
            self.room -= taken;
            self.memory.write(&buf[..taken])
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink for Torn {
        fn cut_to(&mut self, len: u64) -> io::Result<()> {
            self.memory.cut_to(len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::{Memory, Torn};

    #[test]
    fn utc_time_matches_known_values() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(rfc3339_utc(1_709_251_200), "2024-03-01T00:00:00Z");
        assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn line_has_the_fields_in_order_and_escapes_them() {
        let line = line(Path::new("a\"b"), Kind::File, 7, 3, Outcome::Error, "x\\y");

        let tail =
            r#","path":"a\"b","kind":"file","size":7,"passes":3,"result":"error","detail":"x\\y"}"#;
        assert!(line.starts_with("{\"time\":\""));
        assert!(line.ends_with(&format!("{tail}\n")), "{line}");
    }

    #[test]
    fn interrupted_is_written_once_per_run() {
        let memory = Memory::default();
        let log = Log::new(Path::new("erase.log"), 1, Box::new(memory.clone()));

        log.record_interrupted(Path::new("dir/a.txt"), Kind::File, 5);
        log.record_interrupted(Path::new("dir"), Kind::Dir, 0);

        let text = memory.text();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("\"path\":\"dir/a.txt\""));
        assert!(text.contains("\"result\":\"error\",\"detail\":\"interrupted\""));
    }

    #[test]
    fn a_line_that_fails_half_way_is_cut_off() {
        let memory = Memory::default();
        // room for the first line and part of the second one.
        let log = Log::new(
            Path::new("erase.log"),
            1,
            Box::new(Torn {
                memory: memory.clone(),
                room: 150,
            }),
        );

        log.record(Path::new("a"), Kind::File, 5, Outcome::Ok, "");
        log.record(Path::new("b"), Kind::File, 5, Outcome::Ok, "");

        let text = memory.text();
        assert!(log.failed());
        assert_eq!(text.lines().count(), 1, "{text:?}");
        assert!(text.ends_with("}\n"), "{text:?}");
    }

    #[test]
    fn log_inside_a_target_is_refused_however_the_path_is_spelled() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        let targets = std::slice::from_ref(&tree);
        let spellings = [
            tree.join("erase.log"),
            tree.join("sub").join("erase.log"),
            PathBuf::from(format!("{}//./erase.log", tree.display())),
            tree.join("sub").join("..").join("erase.log"),
        ];

        for spelling in &spellings {
            assert!(
                check_location(spelling, targets).is_err(),
                "{}",
                spelling.display()
            );
        }
        assert!(check_location(&dir.path().join("erase.log"), targets).is_ok());
    }

    #[test]
    fn a_directory_with_the_same_name_prefix_is_not_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let other = dir.path().join("tree2");
        fs::create_dir_all(&tree).unwrap();
        fs::create_dir_all(&other).unwrap();

        assert!(check_location(&other.join("erase.log"), std::slice::from_ref(&tree)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_the_log_directory_is_not_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let link = dir.path().join("link");
        fs::create_dir(&tree).unwrap();
        std::os::unix::fs::symlink(&tree, &link).unwrap();

        assert!(check_location(&tree.join("erase.log"), std::slice::from_ref(&link)).is_ok());
    }

    #[test]
    fn existing_log_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("erase.log");
        fs::write(&log, b"").unwrap();

        assert!(check_path(&log, &[]).is_err());
    }
}
