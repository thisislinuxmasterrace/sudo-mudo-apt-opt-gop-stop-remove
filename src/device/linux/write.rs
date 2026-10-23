//! the writes to a device: opened with `O_DIRECT`, checked against the reading made before the confirmation,
//! written from an aligned buffer, with `fdatasync` after each pass.

use std::alloc::{self, Layout};
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::ptr::NonNull;

use crate::erase::{self, Options};
use crate::progress::Progress;

use super::facts::{Facts, inspect};
use super::target::Target;
use super::{ALIGN, CHUNK};

/// opens the device for writing with `O_DIRECT`, so the writes bypass the page cache. the device
/// is not opened exclusively: a device in use is refused by the checks, not by the kernel.
pub(super) fn open_for_write(target: &Target) -> Result<File, String> {
    let shown = target.shown.display();
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_DIRECT)
        .open(&target.path)
        .map_err(|err| {
            let hint = if err.kind() == io::ErrorKind::PermissionDenied {
                " (erasing a device needs root)"
            } else {
                ""
            };
            format!("{shown}: could not open for writing: {err}{hint}")
        })?;
    let meta = file.metadata().map_err(|err| format!("{shown}: {err}"))?;
    if !meta.file_type().is_block_device() || meta.rdev() != target.rdev {
        return Err(format!("{shown}: the device changed while it was opened"));
    }
    Ok(file)
}

/// reads the device again and compares it with what was read before the confirmation. `Ok(false)`
/// means it changed. the open descriptor must still be the device it was opened as (fstat), and the size
/// is taken from it, which is what the writes use.
pub(super) fn unchanged(
    target: &Target,
    expected: &Facts,
    file: &mut File,
) -> Result<bool, String> {
    let shown = target.shown.display();
    let now = inspect(target).map_err(|err| format!("{shown}: {err}"))?;
    let opened = file
        .metadata()
        .map_err(|err| format!("{shown}: {err}"))?
        .rdev();
    let size =
        device_size(file).map_err(|err| format!("{shown}: could not read the size: {err}"))?;
    Ok(now == *expected && opened == target.rdev && size == expected.size)
}

/// the size of a block device, from the open file: seeking from the end gives the device size.
fn device_size(file: &mut File) -> io::Result<u64> {
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    Ok(size)
}

/// `fdatasync`, called again when a signal interrupts it: the kernel does not restart it, and that is not an
/// erase interrupted by the user, so it is not reported as one.
fn sync_pass(file: &File) -> io::Result<()> {
    loop {
        match file.sync_data() {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

/// the write buffer of a device with this logical block size: one chunk, aligned to the larger of a page
/// and the block size.
pub(super) fn buffer_for(logical: u64) -> io::Result<AlignedBuf> {
    let logical = usize::try_from(logical).map_err(io::Error::other)?;
    AlignedBuf::new(CHUNK, ALIGN.max(logical))
}

/// writes every pass over the whole device, with `fdatasync` after each one. the buffer is allocated by the
/// caller, so a failed allocation is known before the first write.
pub(super) fn overwrite(
    file: &mut File,
    size: u64,
    buf: &mut [u8],
    opts: &Options,
) -> io::Result<()> {
    let total = size.saturating_mul(u64::from(opts.passes));
    let mut progress = Progress::new(Some(total), opts.quiet);
    let result = overwrite_passes(file, size, buf, opts, &mut progress);
    progress.finish();
    result
}

fn overwrite_passes(
    file: &mut File,
    size: u64,
    buf: &mut [u8],
    opts: &Options,
    progress: &mut Progress,
) -> io::Result<()> {
    for pass in 0..opts.passes {
        let pattern = erase::pattern_for_pass(pass, opts.passes);
        if opts.verbose {
            progress.finish();
            eprintln!("  pass {}/{}: {}", pass + 1, opts.passes, pattern.name());
        }
        file.seek(SeekFrom::Start(0))?;
        let mut offset = 0u64;
        while offset < size {
            erase::check_interrupted()?;
            let len = (size - offset).min(buf.len() as u64) as usize;
            erase::fill(&mut buf[..len], pattern);
            file.write_all(&buf[..len]).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("write failed at offset {offset}: {err}"),
                )
            })?;
            progress.add(len as u64);
            offset += len as u64;
        }
        sync_pass(file).map_err(|err| {
            io::Error::new(err.kind(), format!("could not flush the pass: {err}"))
        })?;
    }
    Ok(())
}

/// a write buffer whose address is a multiple of `align`, which `O_DIRECT` requires.
pub(super) struct AlignedBuf {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl AlignedBuf {
    fn new(len: usize, align: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::other("the write buffer is empty"));
        }
        let layout = Layout::from_size_align(len, align).map_err(io::Error::other)?;
        // safety: alloc_zeroed needs a nonzero size: it is CHUNK, a constant of 1 mib, and the check above
        // rejects zero in any case. the alignment is a power of two, which from_size_align has checked.
        let ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| io::Error::other("could not allocate the write buffer"))?;
        Ok(Self { ptr, layout })
    }

    pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
        // safety: ptr points to layout.size() bytes from alloc_zeroed, so they are initialized, and they are
        // freed only in drop. the &mut borrow of self makes this slice the only way to reach them.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.layout.size()) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        // safety: ptr was returned by alloc::alloc_zeroed with this layout, and is freed exactly once, here.
        unsafe { alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}
