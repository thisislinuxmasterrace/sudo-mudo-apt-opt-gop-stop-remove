# wipe-free

`--wipe-free <dir>` overwrites the free blocks of the filesystem that holds `<dir>`, so data deleted earlier is harder to recover from unused space. it does not touch any existing file.

```sh
sudo mudo apt opt gop stop remove --dry-run --wipe-free /home    # shows the space it would fill
sudo mudo apt opt gop stop remove --wipe-free /home -n 3
```

## how it works

each pass fills temporary files until the volume is full, then removes them. after each pass the run waits, up to 60 seconds, until the filesystem has freed that space: zfs frees deleted blocks seconds later, and the next pass would find the volume still full. if the space is not freed in time, a warning is printed; the next pass then fills what is free, and after the last pass the filesystem frees the space later by itself. every ordinary pass writes random data; `--zero` adds a final zero pass. a pass that writes more than the volume size stops with exit code 4: the filesystem probably compresses or deduplicates, so free space may not be wiped. files stay open to retain their blocks. reaching the open-file limit fails the run and releases them. unix rolls over at filesystem file-size limits; windows fat32 rolls over one mib below 4 gib. other windows file-size limits are not handled separately.

- nothing in `<dir>` is changed. on linux the fill files are created without a name (`O_TMPFILE`) where the filesystem supports it, and on windows they are deleted on close, so a killed process leaves nothing behind. with the named fallback (`.fill-*`), a kill between create and unlink can leave an empty file.
- free-space wiping does not reach names and metadata: directory entries and journal records of deleted files may keep their names.
- unix keeps the selected directory open before confirmation and creates and unlinks fill files relative to it. changing the path or its symlinks cannot redirect the fill. protection and filesystem checks use the open directory again before creating each file.
- displayed space estimates and warnings are path-based observations, not an atomic snapshot of the retained directory.
- the dry run and the prompt show the free space. the run is root, so blocks reserved for root (ext4 reserves 5% by default) are filled too and the shown figure includes them. if nothing could be written, the run fails with a message (exit code 4).
- other programs writing to the volume may fail while it is full. on large disks this takes a long time. `-v` prints a line for every gigabyte written and a total for each pass; a progress line is drawn on a terminal unless `-q` is given.
- the prompt is skipped with `-f`. ctrl+c, sigterm or sighup stops the fill, removes the temporary files and ends with exit code 4.

## what it refuses

all with exit code 1 and before anything is written:

- protected locations and the filesystem root (`/` on unix). `--i-know-what-i-am-doing` lifts the first, not the root. see [safety](safety.md#protected-locations). the home directory and other mount points are not refused.
- a path that is not a directory.
- tmpfs and ramfs, which have no disk blocks to wipe (no override).
- network filesystems, any fuse mount, any filesystem type the tool does not recognize or cannot read, and remote drives. their free space may belong to a server or to other users. a plain erase only warns about these types.
- bsd systems: the filesystem type is not determined there, so `--wipe-free` is refused.

copy-on-write and log-structured filesystems (apfs, btrfs, zfs, refs, bcachefs, overlay, f2fs, nilfs2, ubifs) are allowed with a warning.

## limits

writing through the filesystem reaches only the blocks allocated to the temporary files. a full pass does not prove that every old copy is gone, even on a hard disk:

- ssd and nvme remapped blocks and over-provisioned space are not offered to the filesystem.
- copy-on-write may place the fill in new blocks, leaving old data.
- snapshots, journals, backups and caches can retain copies outside free space.

the general picture is in [guarantees and limits](guarantees-and-limits.md#by-kind-of-storage). for drives that leave your hands, use encryption or the drive's own erase.

## flags

used with `--wipe-free`: `-n`, `-f`, `-v`, `-q`, `--dry-run`, `--i-know-what-i-am-doing`. positional paths, `--files-from`, `--exclude`, `--json`, `--log` and `--verify` together with `--wipe-free` are argument errors (exit code 2).
