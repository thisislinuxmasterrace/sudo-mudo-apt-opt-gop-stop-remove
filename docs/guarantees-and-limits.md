# guarantees and limits

what an erase can and cannot do. the short version: the tool overwrites through the filesystem, which is a real erase mostly on a hard disk with a filesystem that rewrites blocks in place and no snapshots. everywhere else it is best effort, and encryption or the drive's own erase is the real answer.

contents: [by kind of storage](#by-kind-of-storage) · [what `--verify` proves](#what---verify-proves) · [other limits](#other-limits) · [before selling or giving away a device](#before-selling-or-giving-away-a-device) · [what is not verified](#what-is-not-verified)

## by kind of storage

| storage | what an overwrite does |
| --- | --- |
| hard disk, filesystem that writes in place (ext4, xfs, ntfs, fat), no snapshots | rewrites the blocks that held the data. the one case where the tool is a real erase of file contents |
| ssd, nvme, other flash (usb sticks, sd cards) | wear leveling and remapped blocks send a write to a new physical block. the old block can stay readable until the drive reuses it, and the tool cannot reach it |
| copy-on-write filesystem (apfs, btrfs, zfs, refs, bcachefs) | writes go to new blocks, so the old ones may keep the data. refs block cloning can also make files share clusters |
| journaling | a journal can keep copies of the data |
| snapshots and backups | keep older versions of the files. the tool does not reach them |
| network, fuse, remote drives, unrecognized filesystems | the overwrite may happen on a server that keeps its own copies |
| caches | the os or the drive may hold older data or defer writes |

on these filesystems the tool prints a warning per path and carries on. warnings never block an erase:

- copy-on-write: apfs, btrfs, zfs, refs, bcachefs.
- network: nfs, smb, smb2, cifs, afpfs, smbfs, webdav, ceph, glusterfs, 9p, afs, coda, lustre.
- fuse (`fuse`, `fuse.*`, `fuseblk`, `macfuse`, `osxfuse`), types the tool does not recognize, types it cannot read, and remote drives.
- flash: on linux the tool reads sysfs `queue/rotational` and warns for non-rotational devices. partitions take their disk's answer; device-mapper and md take their slaves'. macos and windows are not detected, and an unknown answer adds nothing.

each overwrite pass is synced with `fsync` before the next one starts. that pushes data to the filesystem's storage, not to the physical blocks the drive may have remapped.

for flash, copy-on-write and shared storage, use full-disk encryption (filevault, bitlocker, luks) enabled before the data was written, or the drive's own secure erase, and treat an overwrite as a courtesy.

## assessments

ordinary file runs report `verdict`, `reasons` and `facts` separately from the operation result. `result: ok` means the operation succeeded, not that every copy disappeared.

- `weak`: overwriting is best effort. reasons include flash, copy-on-write, network storage, an unknown medium or filesystem, and unchecked compression or snapshots.
- `cosmetic`: removing a symbolic link does not overwrite its target. target storage is not inspected.
- `effective`: reserved for proven rotational storage, a suitable filesystem and no interfering compression or snapshots. those conditions are not fully detected, so this version never issues it automatically.

each visited child gets its own storage facts; a directory's assessment concerns its metadata only and does not certify its descendants. skipped, excluded, refused and unreadable entries limit coverage. dry-run json reports `assessment_coverage: incomplete` when any such entries exist; `visited_entries` otherwise describes only the entries inspected, never backups or other copies.

filesystem and medium facts are advisory observations collected before rename or deletion. they do not replace identity checks or authorize an erase. compression and snapshots remain `unknown`; encryption and other copies are `not_checked`. there is no snapshot scan, encryption detection or search for copies in this version. device and free-space runs keep their existing reports.

linux binds storage observations to an open descriptor with checked identity. macos reads the filesystem type through a checked descriptor when opening succeeds; its medium stays unknown. windows and other platforms conservatively leave assessment storage facts unknown. the existing storage warnings remain separate.

## what `--verify` proves

`--verify` reads each file back after its last overwrite pass and compares it with what that pass wrote. the last pass writes pseudo-random bytes made from a 32-byte seed taken from the system generator for each file. the seed lives in memory for one erase and is never written to disk. the generator is not cryptographic: erasing only needs bytes that differ from the old contents.

what a match proves: the filesystem returned the new bytes for that file at that moment.

what it does not prove:

- that the old data is gone from the device. ssd remapped blocks, copy-on-write filesystems, caches, snapshots and server-side copies can keep an older copy that the read cannot see.
- that earlier passes were written. only the last pass is read back, once, after it is synced.
- on linux the kernel's cache for the file is dropped first (`posix_fadvise(DONTNEED)`, a hint). on macos the cache is not dropped, so the check is weaker there.

a `--log` record, with or without `--verify`, says what the tool did, not that the data is unrecoverable. do not hand it over as a certificate of erasure for ssd, nvme or copy-on-write storage.

a mismatch keeps the file under its name, prints `verify failed: contents differ after the last pass, the file is kept` and ends the run with exit code 4; the other paths still go on. the file is opened for reading too, so the owner's read bit is added when it is missing, and a kept file keeps that bit. `--verify` is refused for devices (code 1) and with `--wipe-free` (code 2). with `--dry-run` it is ignored.

## other limits

- **ntfs streams and compression**: only the default data stream is overwritten. alternate streams, such as the zone identifier windows adds to downloads, keep their clusters. compressed files may be rewritten to new clusters, so old data can survive (not tested on windows).
- **interrupted erase**: a file can be left half overwritten. see [usage](usage.md#interrupts).
- **hard links**: other names lose their content too, and root refuses them by default. see [safety](safety.md#hard-links).
- **directory with a read-only parent**: a directory whose parent cannot be modified is not removed. its contents are erased first and the error is reported.
- **bsd** (not linux, macos or windows): the filesystem type is not determined, so erase only warns and `--wipe-free` refuses.
- **the log holds names**: the `--log` file lists the names that were erased (mode 0600 on unix) and stays after the run. see [reference](reference.md#log-format).

## before selling or giving away a device

erasing files does not guarantee that their data is gone from the device. snapshots, backups, remapped blocks and journals are outside the tool's reach, and `--wipe-free` writes only through the filesystem, so it has the same limits.

on linux the tool can overwrite a whole hard disk or partition ([devices](devices.md)). it refuses ssds, does not use the drive's own erase commands, and does not touch space outside the device it is given. for a disk that leaves your hands, consider an external tool:

- `nwipe` (linux) or ShredOS (a bootable image) wipe whole disks; check their documentation for what they do on your drive.
- the drive's own secure erase, through the vendor's tool.
- on macos, filevault enabled before the data was written, instead of overwriting.

## what is not verified

- all windows code (locks, posix delete, reboot deletion, console handler, file identity for `--log`, mount points and junctions, hard links, ntfs streams) compiles for the windows target and has been reviewed, but has not been tested on real windows hardware. ci runs `tests/windows/locks.ps1` on a windows runner.
- erasing devices on linux has not been tested on real disks, usb or partitions. ci runs unit tests of the device functions and the refusal tests; it never erases a real device. macos and windows devices are not implemented.
- macos bind mounts (bindfs, nullfs) are found through `getmntinfo` but have not been tried on real hardware; the script `tests/manual/macos_bind_mount.sh` has not been run. fail-closed behaviour when `getmntinfo` fails is unchecked on hardware.
- refs detection has not been tried on a real refs volume. `fuseblk` (ntfs-3g) is refused by `--wipe-free` by type and has not been tried on a real mount.
- the progress line on a real terminal has been checked only without a tty.
- `--verify` speed is not measured.
- bsd systems have not been tried.
