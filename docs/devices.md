# devices

overwrite a whole disk or partition. this mode is experimental. linux only: on macos and windows a device path is refused with exit code 1. this code has not been tested on real disks, usb drives or partitions (see [guarantees and limits](guarantees-and-limits.md#what-is-not-verified)).

contents: [targets](#targets) · [refusals](#refusals) · [confirmation](#confirmation) · [how it writes](#how-it-writes) · [flags](#flags) · [not covered](#not-covered)

```sh
sudo mudo apt opt gop stop remove --dry-run /dev/sdb   # facts and refusals, writes nothing
sudo mudo apt opt gop stop remove -n 3 /dev/sdb        # asks you to type /dev/sdb
```

## targets

- a block device: a whole disk (`/dev/sdb`) or a partition (`/dev/sdb1`). name the device path itself.
- a symbolic link to a device is refused, except links under `/dev/disk/` (by-id, by-uuid, by-label, by-path, by-partuuid) that resolve to a block device. they are erased through the resolved path. a path with `.` or `..` under `/dev/disk/` is refused.
- a character device named directly is refused as a device (exit code 1).
- the sysfs entry is found by the device number of the node (`/sys/dev/block/major:minor`), and its `dev` file must hold the same number. checks use the kernel device name from sysfs, even when the node has another name.
- devices cannot be mixed with files in one run. `-r` with a device is exit code 1.
- targets that overlap are refused: the same device twice, or a disk together with one of its partitions. two partitions of one disk are fine.

## refusals

before anything is written, with exit code 1, the tool refuses a device that:

- is mounted, or has a partition that is. a partition also checks the mounts, swap and holders of its whole disk.
- is used as swap.
- has holders (sysfs `holders`: device-mapper, md or lvm on top of it).
- is not a hard disk: ssds and devices whose type is unknown are refused (sysfs `queue/rotational`).
- has zero size (no medium).
- is virtual: device-mapper, md or loop (`/sys/devices/virtual/`).
- is checked while a btrfs or zfs filesystem is mounted anywhere on the system: these can span several devices and the mount table shows only one of them, so any mounted btrfs or zfs refuses the device.

if sysfs (`queue/rotational`, `holders`, `size`, `dev`), `/proc/swaps` or the mount table cannot be read, or a `holders` or `slaves` entry is missing, the device is refused (fail closed).

the device is opened with `O_EXCL` after these checks, right before the confirmation. when the kernel answers that it is busy (mounted, or claimed by device-mapper, md, swap or another program), the run is refused with exit code 1. `--dry-run` does not open the device, so it can pass while the real run is refused.

`--i-know-what-i-am-doing` lifts the mounted, swap, holders, not-a-hard-disk and btrfs/zfs refusals, with a warning for each, and opens the device without `O_EXCL`. it never lifts the virtual-device refusal, the zero size or the confirmation.

## confirmation

the tool prints the device summary and asks you to type the exact device path, as you named it. `-f` does not skip this question. it needs a terminal: when stdin is not one, the run is refused with exit code 3 before anything is written. a wrong answer is exit code 3.

`--dry-run` needs no terminal.

## how it writes

- writes go through `O_DIRECT`, from an aligned buffer, in 1 MiB chunks. `fdatasync` follows each pass.
- passes are the same as for files: zeros, ones, random, the last one random.
- before each pass the tool checks again that the open descriptor still points at the same device (`rdev`), and that the sysfs state and the size are unchanged. a change or check failure stops the erase.
- an interrupt leaves the device partly erased. nothing can restore it.

## flags

| flag | with a device |
| --- | --- |
| `-n` | passes, as for files |
| `-f` | no effect on the confirmation (a note says so) |
| `--dry-run` | prints the facts and refusals; `--json` is not supported (exit code 1) |
| `--verify` | refused, exit code 1, also with `--dry-run` |
| `--log <file>` | one line per device, `kind` `device` or `partition`. the log must not be on the device being erased. on a device run a log on tmpfs, overlay, nfs, zfs or any non-block source is refused |
| `-r` | refused, exit code 1 |

## not covered

macos and windows devices (not implemented; windows would need dismounted volumes and administrator rights), the drive's own commands (ata secure erase, nvme format) and `blkdiscard`, which is not counted as an erase. checking the members of a multi-device btrfs or zfs, and the backing file of a loop device, is not done.
