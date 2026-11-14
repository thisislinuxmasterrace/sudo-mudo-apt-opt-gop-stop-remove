# wipe-free

`--wipe-free <dir>` overwrites the free blocks of the filesystem that holds `<dir>`, so data deleted earlier is harder to recover from unused space. it does not touch any existing file.

contents: [how it works](#how-it-works) · [what it refuses](#what-it-refuses) · [limits](#limits) · [flags](#flags)

```sh
sudo mudo apt opt gop stop remove --dry-run --wipe-free /home    # shows the space it would fill
sudo mudo apt opt gop stop remove --wipe-free /home -n 3
```

## how it works

each pass creates temporary files in `<dir>`, writes one pattern (zeros, ones, random; the last pass is random) into new blocks until the volume is full, then removes them. on unix, a filesystem with a file size limit, such as fat32, gets several files per pass. on windows fat32, each file stops one mib below 4 gib and the pass continues with another file: windows otherwise reports disk full at the per-file limit even when space remains. other windows file-size limits are not handled separately.
the files stay open until the pass ends, keeping their blocks allocated while the next file is filled.
each retained file uses an open handle. if the process reaches its open-file limit before the volume is full, the run fails and releases the temporary files; it does not report a completed wipe.

- nothing in `<dir>` is changed. the temporary files are unlinked on unix and deleted on close on windows, so a killed process leaves nothing behind.
- the dry run and the prompt show the reported available space. unix invocations require root and can also fill filesystem-reserved blocks (ext4 reserves 5% by default), so actual writes may exceed that estimate. if nothing could be written, the run fails with a message (exit code 4).
- other programs writing to the volume may fail while it is full. on large disks this takes a long time. `-v` prints a line for every gigabyte written and a total for each pass; a progress line is drawn on a terminal unless `-q` is given.
- the prompt is skipped with `-f`. ctrl+c, sigterm or sighup stops the fill, removes the temporary files and ends with exit code 4.

## what it refuses

all with exit code 1 and before anything is written:

- protected locations and the filesystem root. `--i-know-what-i-am-doing` lifts the first, not the root. see [safety](safety.md#protected-locations). the home directory is not refused.
- a path that is not a directory.
- network filesystems, any fuse mount, any filesystem type the tool does not recognize or cannot read, and remote drives. their free space may belong to a server or to other users. a plain erase only warns about these types.
- bsd systems: the filesystem type is not determined there, so `--wipe-free` is refused.

copy-on-write filesystems (apfs, btrfs, zfs, refs, bcachefs) are allowed with a warning.

## limits

writing through the filesystem reaches only the blocks allocated to the temporary files. a full pass does not prove that every old copy is gone, even on a hard disk:

- ssd and nvme: remapped blocks and over-provisioned space are never offered to the filesystem, so they are not written. a full volume does not mean every physical cell was overwritten.
- copy-on-write filesystems may place the fill in new blocks while older ones keep the data.
- snapshots and copies the filesystem keeps are not free space and are not reached.
- journals, backups and caches can keep copies too.

the general picture is in [guarantees and limits](guarantees-and-limits.md#by-kind-of-storage). for drives that leave your hands, use encryption or the drive's own erase.

## flags

used with `--wipe-free`: `-n`, `-f`, `-v`, `-q`, `--dry-run`, `--i-know-what-i-am-doing`. positional paths, `--files-from`, `--exclude`, `--json`, `--log` and `--verify` together with `--wipe-free` are argument errors (exit code 2).
