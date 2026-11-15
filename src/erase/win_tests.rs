use super::*;
use std::io::Read;

fn options() -> Options {
    Options {
        passes: 1,
        zero: false,
        verbose: false,
        force: true,
        kill_holders: false,
        allow_protected: false,
        quiet: true,
        verify: true,
    }
}

#[test]
fn retry_uses_size_after_holder_changes_file_before_closing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let mut opts = options();
    opts.kill_holders = true;
    for (before, after) in [(8, 4096), (4096, 8)] {
        fs::write(&path, vec![42; before]).unwrap();
        let mut observer = File::open(&path).unwrap();
        let listing = Ok(vec![windows::Holder {
            pid: 0,
            name: "test holder".to_owned(),
        }]);
        erase_locked_with(
            &path,
            &opts,
            io::Error::from_raw_os_error(32),
            listing,
            |path| {
                fs::write(path, vec![42; after])?;
                Ok(Vec::new())
            },
        )
        .unwrap();
        assert!(!path.exists());
        let mut contents = Vec::new();
        observer.read_to_end(&mut contents).unwrap();
        assert_eq!(contents.len(), after);
        let changed = if after > before {
            &contents[before..]
        } else {
            &contents[..]
        };
        assert!(changed.iter().any(|&byte| byte != 42));
    }
}

#[test]
fn named_stream_contents_are_overwritten_before_unlink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"main").unwrap();
    let stream = dir.path().join("data:secret");
    fs::write(&stream, vec![42; 2_500_017]).unwrap();
    let mut observer = File::open(&stream).unwrap();
    let mut opts = options();
    opts.passes = 2;
    opts.zero = true;
    erase_file(&path, &opts).unwrap();
    assert!(!path.exists());
    let mut bytes = Vec::new();
    observer.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len(), 2_500_017);
    assert!(bytes.iter().all(|&byte| byte == 0));
}

#[test]
fn locked_stream_keeps_main_contents_and_directory_streams_are_refused() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    fs::write(&path, b"main").unwrap();
    let stream = dir.path().join("data:secret");
    fs::write(&stream, b"secret").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&stream)
        .unwrap();
    assert!(erase_file(&path, &options()).is_err());
    drop(locked);
    assert_eq!(fs::read(&path).unwrap(), b"main");
    assert_eq!(fs::read(&stream).unwrap(), b"secret");

    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let child = tree.join("child");
    fs::write(&child, b"child").unwrap();
    fs::write(dir.path().join("tree:secret"), b"directory secret").unwrap();
    assert!(erase_path(&tree, &options(), &[], None).is_err());
    assert_eq!(fs::read(child).unwrap(), b"child");
}
