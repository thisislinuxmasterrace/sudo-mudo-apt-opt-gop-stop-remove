# safety

what the tool refuses, and why. every check runs before the first byte is written, except where noted. a refusal before erasing exits with code 1; the codes are in the [reference](reference.md#exit-codes).

contents: [summary](#summary) · [paths and symlinks](#paths-and-symlinks) · [protected locations](#protected-locations) · [mount points](#mount-points) · [hard links](#hard-links) · [special files](#special-files) · [what lifts a refusal](#what-lifts-a-refusal)

## summary

| target | result | lifted by `--i-know-what-i-am-doing` |
| --- | --- | --- |
| a missing path, a directory without `-r` | refused, code 1 | no |
| fifo, socket, character device | refused, code 1 | no |
| protected system path, the home directory | refused, code 1 | yes (with a warning) |
| filesystem root | refused, code 1 | no |
| mount point given as a target (unix) | refused, code 1 | no |
| mount point or other filesystem inside `-r` | skipped, directory kept, code 4 | no |
| hard-linked file under root, given as a path (unix) | refused, code 1 | yes (with a warning) |
| hard-linked file under root, found inside `-r` (unix) | refused, tree kept, code 4 | yes (with a warning) |
| symlink | removed as a link, target untouched | n/a |

warnings (copy-on-write, network, fuse, unknown filesystems, flash media, hard links as a normal user) never block an erase.

## paths and symlinks

paths are normalized first (`.` components are dropped), so `link/` is the link, not its target. a symlink is never followed: erasing one removes the link. a link to a protected directory can therefore be removed, while the directory it points to stays. an empty path argument is an argument error (code 2).

on unix, `-r` walks through open directory descriptors (`openat`, `fstatat`, `renameat`, `unlinkat`). a directory swapped for a symlink during the walk cannot redirect the erase. every entry, links and directories included, is renamed to a random hidden name and checked against its earlier device, inode and type. a substitution detected by that check is reported and its name is restored when possible. the check and the later unlink are separate operations: another process with write access to the parent can still replace the hidden name between them.

on windows, reparse points that are not recognized as symbolic links or junctions are refused without changing their attributes or traversing them. the path-based walk cannot prevent substitutions by another process between checks and operations.

## protected locations

refused for erase and for `--wipe-free`:

- unix: `/bin`, `/sbin`, `/usr`, `/etc`, `/boot`, `/lib`, `/lib64`, `/dev`, `/proc`, `/sys`, `/System`, `/Library`, also under their resolved paths (`/private/etc` on macos), and everything inside them.
- windows: the system folders, read from `SystemRoot`, `ProgramFiles` and `SystemDrive`.
- the root of a filesystem.

`/var`, `/opt`, `/Volumes` and `/private` are refused as directories only: files inside them can be erased, so volumes under `/Volumes` and `/private/tmp` stay usable.

on unix a directory is compared by device and inode with the protected ones and their ancestors, not by spelling. `/USR/bin` on a case-insensitive macos volume is `/usr/bin`, and a bind mount of `/usr` at `~/x` is refused too.

the home directory (`$HOME` or the passwd entry on unix, `%USERPROFILE%` on windows) is refused for erase as a directory; files inside it can be erased. `--wipe-free` does not refuse it, because it changes no existing files.

## mount points

- a mount point, or a path on another filesystem than its parent, given as a target is refused with code 1 before anything is erased (unix only). on windows this check does not exist.
- inside `-r`, a mount point or another filesystem is skipped and reported, the directory that holds it is kept, and the run ends with code 4.
- the mount table is read from `/proc/self/mountinfo` on linux and `getmntinfo` on macos, so bind mounts are found. it is read again for each path given on the command line, so a mount made while an earlier path was erased is seen; a mount made during the walk of one path is not. mount points that are not utf-8 are compared as bytes. on linux the path of the opened parent directory is taken from its descriptor (`/proc/self/fd`) and checked to be the same directory before it is used for the lookup. on other bsds only the device id is compared, so a bind mount there can be erased through its source.
- if the mount table cannot be read, every entry that is not a link is refused and reported (fail closed).
- on windows, mount points and junctions count as links: removed as links, content untouched. a junction was tested in windows ci; volume mount points have not been verified.

## hard links

a hard link shares one copy of the data with other names, so overwriting it erases the content for all of them.

- as a normal user the file is erased like any other, and the summary lists hard-linked files and warns that other names lose their content too.
- as root on unix it is refused, because other names may be files the user could not otherwise change. a file given as a path is refused during validation (code 1, also with `--dry-run`). a file found inside `-r` is refused during the walk and the tree is kept (code 4). `--i-know-what-i-am-doing` erases it with a warning.
- on windows the link count is not read and root is never detected, so the rule does not apply and the summary does not list hard links.
- system trees are not scanned for hard links.

## special files

fifos, sockets and character devices are refused when named, and listed as `other` in the summary and log when found in a tree. a block device given as a target switches the run to [device erase](devices.md) (linux only; elsewhere refused with code 1). a symlink to a device is refused, except the links under `/dev/disk/` described there.

## what lifts a refusal

- `--i-know-what-i-am-doing` lifts the protected-path refusal (a warning is printed every time), the hard-link rule under root and, for devices, the refusals listed in [devices](devices.md#refusals). it never lifts the filesystem root, mount-point targets, or the confirmation of a device erase.
- `-f` skips questions only. it does not lift any refusal.
