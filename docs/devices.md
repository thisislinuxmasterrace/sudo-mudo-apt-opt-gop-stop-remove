# devices

overwrite a whole disk or partition. this mode is experimental. linux only: on macos and windows a device path is refused with exit code 1. this code has not been tested on real disks, usb drives or partitions (see [guarantees and limits](guarantees-and-limits.md#what-is-not-verified)).

```sh
sudo mudo apt opt gop stop remove --dry-run /dev/sdb   # facts and refusals, writes nothing
sudo mudo apt opt gop stop remove --explain /dev/sdb   # advice too; existing refusals still apply
sudo mudo apt opt gop stop remove -n 3 /dev/sdb        # asks you to type /dev/sdb
```

## targets

name a block device such as `/dev/sdb` or `/dev/sdb1`. symlinks are refused except `/dev/disk/` links resolving to a block device; `.` and `..` under `/dev/disk/` are refused. character devices, mixing devices with files, `-r`, and overlapping targets are refused with code 1. two partitions of one disk are allowed. checks use the device number and kernel name from sysfs.

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

after checks, the device is opened with `O_EXCL`; a busy device is refused with code 1. `--dry-run` does not open it and can pass when a real run would be refused.

`--i-know-what-i-am-doing` lifts the mounted, swap, holders, not-a-hard-disk and btrfs/zfs refusals, with a warning for each, and opens the device without `O_EXCL`. it never lifts the virtual-device refusal, the zero size or the confirmation.

## confirmation

type the exact device path as named. `-f` cannot skip this question; non-terminal stdin or a wrong answer exits with code 3 before writing. `--dry-run` needs no terminal.

## how it writes

- writes go through `O_DIRECT`, from an aligned buffer, in 1 MiB chunks. `fdatasync` follows each pass.
- passes are the same as for files: zeros, ones, random, the last one random; `--zero` adds one final zero pass.
- before each pass the tool checks again that the open descriptor still points at the same device (`rdev`), and that the sysfs state and the size are unchanged. a change or check failure stops the erase.
- an interrupt leaves the device partly erased. nothing can restore it.

## flags

| flag | with a device |
| --- | --- |
| `-n` | passes, as for files |
| `--zero` | one additional final zero pass; no stronger erasure guarantee |
| `--explain` | dry run with overwrite, firmware erase and luks advice. commands are printed, never executed; support is not assumed |
| `-f` | no effect on the confirmation (a note says so) |
| `--dry-run` | prints the facts and refusals; `--json` is not supported (exit code 1) |
| `--verify` | refused, exit code 1, also with `--dry-run` |
| `--log <file>` | one line per device, `kind` `device` or `partition`. the log must not be on the device being erased. on a device run a log on tmpfs, overlay, nfs, zfs or any non-block source is refused |
| `-r` | refused, exit code 1 |

## not covered

macos and windows devices (not implemented; windows would need dismounted volumes and administrator rights), the drive's own commands (ata secure erase, nvme format) and `blkdiscard`, which is not counted as an erase. checking the members of a multi-device btrfs or zfs, and the backing file of a loop device, is not done.
