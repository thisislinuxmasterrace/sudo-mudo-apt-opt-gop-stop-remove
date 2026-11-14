# safety

what the tool refuses, and why. every check runs before the first byte is written, except where noted. a refusal before erasing exits with code 1; the codes are in the [reference](reference.md#exit-codes).

## summary

missing paths and directories without `-r` are refused with code 1. storage warnings do not block a file erase. other checks and overrides are below.

## paths and symlinks

paths are normalized first (`.` components are dropped), so `link/` is the link, not its target. a symlink is never followed: erasing one removes the link. a link to a protected directory can therefore be removed, while the directory it points to stays. an empty path argument is an argument error (code 2).

on unix, `-r` walks open directory descriptors, preventing a swapped directory from redirecting the walk. entries are renamed and checked against their earlier identity; a detected substitution is reported and restored when possible. the later unlink is separate, so another process with parent-directory write access can still replace the hidden name between checks.

on windows, reparse points that are not recognized as symbolic links or junctions are refused without changing their attributes or traversing them. the path-based walk cannot prevent substitutions by another process between checks and operations.

## protected locations

refused for erase and for `--wipe-free`:

- unix: `/bin`, `/sbin`, `/usr`, `/etc`, `/boot`, `/lib`, `/lib64`, `/dev`, `/proc`, `/sys`, `/System`, `/Library`, also under their resolved paths (`/private/etc` on macos), and everything inside them.
- windows: the system folders, read from `SystemRoot`, `ProgramFiles` and `SystemDrive`.
- the root of a filesystem.

`/var`, `/opt`, `/Volumes` and `/private` are refused as directories only: files inside them can be erased, so volumes under `/Volumes` and `/private/tmp` stay usable.

on unix a directory is compared by device and inode with the protected ones and their ancestors, not by spelling. `/USR/bin` on a case-insensitive macos volume is `/usr/bin`, and a bind mount of `/usr` at `~/x` is refused too.

the home directory (`$HOME` or the passwd entry on unix, `%USERPROFILE%` on windows) is refused for erase as a directory; files inside it can be erased. unix also protects the invoking sudo account's home, resolved and retained before argument parsing, even when sudo resets `$HOME` to root's home. `--wipe-free` does not refuse home, because it changes no existing files.

## mount points

- a mount point, or a path on another filesystem than its parent, given as a target is refused with code 1 before anything is erased (unix only). on windows this check does not exist.
- inside `-r`, a mount point or another filesystem is skipped and reported, the directory that holds it is kept, and the run ends with code 4.
- linux and macos read the mount table, including bind mounts, again for each command-line path, but not during a path's walk. non-utf-8 mount points are compared as bytes. other bsds compare only device ids, so a bind mount there can be erased through its source.
- if the mount table cannot be read, every entry that is not a link is refused and reported (fail closed).
- on windows, mount points and junctions count as links: removed as links, content untouched. a junction was tested in windows ci; volume mount points have not been verified.

## hard links

a hard link shares one copy of the data with other names, so overwriting it erases the content for all of them.

- unix invocations run as root and refuse hard-linked files: a named target fails validation (code 1, also with `--dry-run`); one found inside `-r` keeps the tree and returns code 4. `--i-know-what-i-am-doing` overrides this with a warning.
- on windows the link count is not read and root is never detected, so the rule does not apply and the summary does not list hard links.
- system trees are not scanned for hard links.

## special files

fifos, sockets and character devices are refused when named, and listed as `other` in the summary and log when found in a tree. a block device given as a target switches the run to [device erase](devices.md) (linux only; elsewhere refused with code 1). a symlink to a device is refused, except the links under `/dev/disk/` described there.

## what lifts a refusal

- `--i-know-what-i-am-doing` lifts the protected-path refusal (a warning is printed every time), the hard-link rule under root and, for devices, the refusals listed in [devices](devices.md#refusals). it never lifts the filesystem root, mount-point targets, or the confirmation of a device erase.
- `-f` skips questions only. it does not lift any refusal.
