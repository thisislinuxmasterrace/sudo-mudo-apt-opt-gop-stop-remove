#![cfg(target_os = "linux")]

mod common;

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, MetadataExt};

use common::{assert_exit, path_str, run};
use nix::errno::Errno;
use nix::unistd::{Whence, lseek};

const MIB: u64 = 1024 * 1024;
const LEN: u64 = 5 * MIB;
const DATA: [u8; 4096] = [0x55; 4096];

fn hole_at(file: &File, offset: u64) -> bool {
    match lseek(file, offset as i64, Whence::SeekData) {
        Ok(next) => next as u64 >= offset + DATA.len() as u64,
        Err(Errno::ENXIO) => true,
        Err(Errno::EINVAL | Errno::ENOSYS | Errno::EOPNOTSUPP) => false,
        Err(err) => panic!("seek data failed: {err}"),
    }
}

fn ext4(file: &File) -> bool {
    nix::sys::statfs::fstatfs(file).unwrap().filesystem_type().0 == libc::EXT4_SUPER_MAGIC as _
}

fn read_sample(file: &File, offset: u64) -> [u8; 4096] {
    let mut sample = [0; 4096];
    assert_eq!(file.read_at(&mut sample, offset).unwrap(), sample.len());
    sample
}

#[test]
fn sparse_files_keep_holes_through_cli_verification() {
    let dir = tempfile::tempdir().unwrap();
    for populated in [true, false] {
        let path = dir.path().join(if populated { "sparse" } else { "empty" });
        let writer = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        writer.set_len(LEN).unwrap();
        if populated {
            for offset in [MIB, 3 * MIB] {
                assert_eq!(writer.write_at(&DATA, offset).unwrap(), DATA.len());
            }
        }
        writer.sync_all().unwrap();
        drop(writer);
        let observer = File::open(&path).unwrap();
        let holes: Vec<_> = [0, 2 * MIB, 4 * MIB]
            .into_iter()
            .filter(|&offset| hole_at(&observer, offset))
            .collect();
        let seek_hole_supported =
            !populated || lseek(&observer, MIB as i64, Whence::SeekHole).is_ok();
        let no_data = matches!(lseek(&observer, 0, Whence::SeekData), Err(Errno::ENXIO));
        let blocks = observer.metadata().unwrap().blocks();

        let out = run(&["-f", "--verify", "-n", "3", path_str(&path)], "");

        assert_exit(&out, 0);
        assert!(!path.exists());
        assert_eq!(observer.metadata().unwrap().len(), LEN);
        if populated {
            for offset in [MIB, 3 * MIB] {
                assert_ne!(read_sample(&observer, offset), DATA);
            }
        }
        if seek_hole_supported {
            for offset in &holes {
                assert_eq!(read_sample(&observer, *offset), [0; 4096]);
            }
            if ext4(&observer) && !holes.is_empty() {
                assert_eq!(observer.metadata().unwrap().blocks(), blocks);
            }
        }
        if !populated && no_data {
            for offset in (0..LEN).step_by(DATA.len()) {
                assert_eq!(read_sample(&observer, offset), [0; 4096]);
            }
        }
    }
}
