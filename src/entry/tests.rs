use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::{assessment::Assessment, erase, preview};

const SET_REPARSE: u32 = 0x0009_00a4;
const DELETE_REPARSE: u32 = 0x0009_00ac;

#[repr(C)]
struct ReparseBuffer {
    tag: u32,
    length: u16,
    reserved: u16,
    guid: [u8; 16],
}

impl ReparseBuffer {
    fn new() -> Self {
        Self {
            tag: 0x0000_0042,
            length: 0,
            reserved: 0,
            guid: [0x51; 16],
        }
    }
}

struct Fixture {
    file: File,
    path: PathBuf,
    permissions: fs::Permissions,
    attached: bool,
}

fn control(file: &File, code: u32) -> io::Result<()> {
    let buffer = ReparseBuffer::new();
    let mut returned = 0;
    // safety: the live handle and buffer are valid for this synchronous call.
    let result = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            (&raw const buffer).cast(),
            size_of::<ReparseBuffer>() as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Fixture {
    fn new(path: &Path, directory: bool) -> io::Result<Option<Self>> {
        if directory {
            fs::create_dir(path)?;
        } else {
            fs::write(path, b"preserve contents")?;
        }
        let file = OpenOptions::new()
            .access_mode(0x4000_0000 | 0x80)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let permissions = file.metadata()?.permissions();
        if let Err(error) = control(&file, SET_REPARSE) {
            if error.raw_os_error() == Some(1314) && std::env::var_os("GITHUB_ACTIONS").is_none() {
                eprintln!("skipping reparse fixture: {error}");
                return Ok(None);
            }
            return Err(error);
        }
        let fixture = Self {
            file,
            path: path.to_owned(),
            permissions,
            attached: true,
        };
        let mut readonly = fixture.permissions.clone();
        readonly.set_readonly(true);
        fixture.file.set_permissions(readonly)?;
        Ok(Some(fixture))
    }

    fn cleanup(&mut self) -> io::Result<()> {
        let permissions = self.file.set_permissions(self.permissions.clone());
        let detached = if self.attached {
            let result = control(&self.file, DELETE_REPARSE);
            if result.is_ok() {
                self.attached = false;
            }
            result
        } else {
            Ok(())
        };
        permissions.and(detached)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!(
                "could not clean reparse fixture {}: {error}",
                self.path.display()
            );
        }
    }
}

fn options() -> erase::Options {
    erase::Options {
        passes: 1,
        zero: false,
        verbose: false,
        force: true,
        kill_holders: false,
        allow_protected: false,
        quiet: true,
        verify: false,
    }
}

#[test]
fn unsupported_reparse_points_are_refused_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let file = tree.join("file");
    let directory = tree.join("directory");
    let Some(mut file_fixture) = Fixture::new(&file, false).unwrap() else {
        return;
    };
    let Some(mut dir_fixture) = Fixture::new(&directory, true).unwrap() else {
        return;
    };
    for path in [&file, &directory] {
        let meta = fs::symlink_metadata(path).unwrap();
        assert_ne!(meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT, 0);
        assert!(!meta.file_type().is_symlink());
        assert_eq!(meta.is_file(), path == &file);
        assert_eq!(meta.is_dir(), path == &directory);
        assert!(super::is_unsupported_reparse(&meta));
        assert!(meta.permissions().readonly());
        let (_, assessment) = Assessment::capture(path).unwrap();
        assert_eq!(
            assessment,
            Assessment::from_facts(
                None,
                crate::storage::Medium::Unknown,
                crate::storage::Compression::Unknown,
                crate::storage::Encryption::NotChecked,
                false,
                false,
                None,
            )
        );
    }
    let summary = preview::summarize(std::slice::from_ref(&tree), true, &[], false).remove(0);
    assert_eq!((summary.dirs, summary.files, summary.other), (1, 0, 2));
    assert_eq!((summary.unreadable, summary.skipped), (0, 0));
    assert!(erase::erase_path(&tree, &options(), &[], None).is_err());
    for path in [&file, &directory] {
        let meta = fs::symlink_metadata(path).unwrap();
        assert!(super::is_unsupported_reparse(&meta));
        assert!(meta.permissions().readonly());
    }
    dir_fixture.cleanup().unwrap();
    file_fixture.cleanup().unwrap();
    assert_eq!(fs::read(file).unwrap(), b"preserve contents");
}

#[test]
fn junction_is_counted_and_removed_as_link() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let junction = temp.path().join("junction");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("child"), b"untouched").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success());
    let meta = fs::symlink_metadata(&junction).unwrap();
    assert!(meta.file_type().is_symlink());
    assert!(!super::is_unsupported_reparse(&meta));
    let summary = preview::summarize(std::slice::from_ref(&junction), true, &[], false).remove(0);
    assert_eq!((summary.links, summary.files, summary.dirs), (1, 0, 0));
    erase::erase_path(&junction, &options(), &[], None).unwrap();
    assert_eq!(fs::read(target.join("child")).unwrap(), b"untouched");
}
