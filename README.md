# sudo-mudo-apt-opt-gop-stop-remove

when you really want to delete something

## what is this?

a command-line tool that irreversibly erases files: it overwrites their
contents (one pass by default, `-n` for more), then deletes them. works on windows, macos and linux.

> **status:** early development.

## install

download a binary for your platform from the
[releases page](https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/releases)
and check it against `SHA256SUMS`. that file is published next to the binaries, so it only
detects a corrupted download, not a replaced one. or build from source:

```sh
cargo build --release
```

the binary is `target/release/sudo-mudo-apt-opt-gop-stop-remove`.

## usage

```sh
sudo-mudo-apt-opt-gop-stop-remove venom.exe            # erase a file, asks for confirmation
sudo-mudo-apt-opt-gop-stop-remove -n 3 disk-dump.img   # three overwrite passes (default is one)
sudo-mudo-apt-opt-gop-stop-remove -f notes.txt         # ask no questions, warnings are still shown
sudo-mudo-apt-opt-gop-stop-remove -r build/            # erase a directory tree
sudo-mudo-apt-opt-gop-stop-remove --dry-run notes.txt  # only show what would be erased
```

symbolic links are removed themselves and never followed, so their targets are
left untouched. read-only files and directories, and entries the owner cannot access at all, are
made accessible to their owner before erasing.

before anything is erased, the tool prints what it will touch: files, directories,
links, total size, hard-linked files, skipped mount points, special files (fifos, sockets,
devices, which are refused) and entries it could not read. `--dry-run` prints the same
summary without changing anything. with `-f` the summary is not shown and the walk is not
done twice.

with `-r`, a mount point or a directory on another filesystem inside the tree is
skipped and reported, and the directory that holds it is kept. on linux and macos the
mount table is read (`/proc/self/mountinfo` and `getmntinfo`), so bind mounts are found
too. on the other bsds only the filesystem boundary is checked, so a bind mount there can
be erased through its source. on windows, mount points and junctions are links: they are
removed as links and their content is not touched (not verified on windows yet). if the
mount table cannot be read, every entry that is not a link is refused and reported, so nothing
is erased through a bind mount that could not be seen.

protected system locations are refused for erasing and for `--wipe-free`: `/bin`, `/sbin`,
`/usr`, `/etc`, `/boot`, `/lib`, `/lib64`, `/dev`, `/proc`, `/sys`, `/System` and `/Library` on
unix (also under their resolved paths, such as `/private/etc` on macos), the windows system
folders and the root of a filesystem. a link to one of them can still be removed as a link.
a bind mount of one of them is refused too.

## limitations

overwriting does not guarantee that data is unrecoverable on every device or
filesystem. ssd wear leveling, copy-on-write filesystems (apfs, btrfs, zfs),
journaling, snapshots, backups and caches can keep old copies of the data.
for ssds, full-disk encryption (filevault, bitlocker, luks) is the more
reliable approach.

hard links share one copy of the data, so erasing one name erases the content
for all of them. a hard link inside a tree to a protected file is erased like any other file,
and the summary warns about it. on windows hard links are not detected yet, so the summary does
not list them.

a directory whose parent cannot be modified is not removed. its contents are erased first,
and the tool reports the error for the directory.

files on btrfs, zfs, apfs and refs trigger a warning, because those filesystems write
to new blocks and may keep the old data.

files on network filesystems (nfs, smb, smb2, cifs, afpfs, smbfs, webdav) and on remote drives also
trigger a warning: the overwrite happens on the server, which may keep copies that this
tool cannot see.

the warnings do not block the erase.

on ntfs only the default data stream is overwritten. alternate streams, such as the zone
identifier that windows adds to downloads, keep their clusters after the erase. compressed
files may be rewritten to new clusters, so the old data can survive (not verified on windows).

an erase that is interrupted (ctrl+c, kill, power loss) can leave a file half overwritten
under a random hidden name in its directory (`.` followed by 16 hex digits). a directory can be
left partly erased under its original name. the tool prints nothing about that state.

### locked files (windows only)

a file held open by another program is not erased by default.

- `--kill-holders` closes the programs that hold the file. it asks first, unless
  `-f` is given. services may be stopped too.
- `--raw-volume` overwrites the file's clusters directly on the volume. it needs
  administrator rights and bypasses the filesystem. it prints a warning and asks
  for confirmation unless `-f` is given.
  files whose data is stored inside the mft (very small files), compressed or sparse
  files, refs files and reparse points (links, dedup and cloud files) are refused. the
  layout is checked again after the question, and nothing is written if it changed.
  a process that still holds the file can write its cached data back over the erased
  clusters; the tool lists such processes before the question.
  on a mounted volume that is in use, windows refuses direct writes (access denied). this tool
  does not lock or dismount volumes, so `--raw-volume` currently fails there and erases nothing:
  it is not a working option yet.

if the file still cannot be deleted after its contents are erased, you are asked
whether to delete it at the next reboot. with `-f` nothing is scheduled and the
file stays in place.

## wiping free space

`--wipe-free <dir>` erases the free space of the filesystem that holds `<dir>`. it fills
the free blocks with temporary files of pattern data, one pass per pattern, and removes
them. data deleted earlier then stops being recoverable from unused blocks.

```sh
sudo-mudo-apt-opt-gop-stop-remove --wipe-free /home -n 3
```

- nothing in `<dir>` is touched. the temporary files vanish even if the process is killed.
- the dry run and the prompt show the space available to you. blocks reserved for root
  (ext4 reserves 5% by default) are not available to ordinary users, so they are only
  wiped when run as root. if nothing could be written, the run fails with a message.
- network filesystems and remote drives are refused: their free space belongs to the
  server and to other users.
- snapshots and copies the filesystem keeps are not free space and are not reached.
- other programs writing to the volume may fail while it is full.
- on large disks this takes a long time. with `-v` the tool prints a line for every
  gigabyte written and a total for each pass.

## license

copyright (c) 2026 kirill belolipetsky. mit license, see [LICENSE](LICENSE).

erased files cannot be recovered with this tool. use it with care.

## development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```
