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

the binary is `target/release/sudo-mudo-apt-opt-gop-stop-remove`. building needs rust 1.88 or newer.

## usage

```sh
sudo-mudo-apt-opt-gop-stop-remove venom.exe            # erase a file, asks for confirmation
sudo-mudo-apt-opt-gop-stop-remove -n 3 disk-dump.img   # three overwrite passes (default is one)
sudo-mudo-apt-opt-gop-stop-remove -f notes.txt         # ask no questions, warnings are still shown
sudo-mudo-apt-opt-gop-stop-remove -r build/            # erase a directory tree
sudo-mudo-apt-opt-gop-stop-remove --dry-run notes.txt  # only show what would be erased
sudo-mudo-apt-opt-gop-stop-remove --verify -f notes.txt  # read the last pass back and compare it
sudo-mudo-apt-opt-gop-stop-remove -r --exclude '*.o' --exclude .git project/  # skip matching entries
sudo-mudo-apt-opt-gop-stop-remove -0 --files-from list.bin                    # paths from a nul-separated list
sudo-mudo-apt-opt-gop-stop-remove --log erase.jsonl -r build/                 # json lines log of every object
sudo-mudo-apt-opt-gop-stop-remove --dry-run --json -r build/                  # the summary as one json line
sudo-mudo-apt-opt-gop-stop-remove -q -f -r build/                             # no progress line, no questions
```

symbolic links are removed themselves and never followed, so their targets are
left untouched. read-only files and directories, and entries the owner cannot access at all, are
made accessible to their owner before erasing.

before anything is erased, the tool prints what it will touch: files, directories,
links, total size, hard-linked files, skipped mount points, excluded entries, special files (fifos,
sockets, devices, which are refused) and entries it could not read. `--dry-run` prints the same
summary without changing anything. with `-f` the summary is not shown and the walk is not
done twice.

with `-r`, a mount point or a directory on another filesystem inside the tree is
skipped and reported, the directory that holds it is kept, and the run ends with exit code 4. a mount
point given as a target is refused before anything is erased, with exit code 1, and
`--i-know-what-i-am-doing` does not lift that refusal. on linux and macos the
mount table is read (`/proc/self/mountinfo` and `getmntinfo`), so bind mounts are found
too. on the other bsds only the filesystem boundary is checked, so a bind mount there can
be erased through its source. on windows, mount points and junctions are treated as links: they are
removed as links and their content is not touched. the windows behaviour has not been tested on real
hardware. if the mount table cannot be read, every entry that is not a link is refused and reported,
so nothing is erased through a bind mount that could not be seen.

protected system locations are refused for erasing and for `--wipe-free`: `/bin`, `/sbin`,
`/usr`, `/etc`, `/boot`, `/lib`, `/lib64`, `/dev`, `/proc`, `/sys`, `/System` and `/Library` on
unix (also under their resolved paths, such as `/private/etc` on macos), the windows system
folders and the root of a filesystem. the directories `/var`, `/opt`, `/Volumes` and `/private`
are protected as directories only: the files inside them can be erased.

your home directory (`$HOME` or the passwd entry on unix, `%USERPROFILE%` on windows) is refused
for erasing as a directory, but the files inside it can be erased. `--wipe-free` does not refuse it,
because it changes no existing files.

a link to a protected directory can still be removed as a link. a bind mount of one of them is
refused too. paths are compared by device and inode on unix, so a different spelling (`/USR/bin`
on a case-insensitive macos volume) does not get around the check.

`--i-know-what-i-am-doing` allows erasing and filling protected locations. a warning is printed
each time it is used, and `-f` does not allow them. the filesystem root itself stays refused: it has
no name that could be removed.

## limitations

overwriting does not guarantee that data is unrecoverable on every device or
filesystem. ssd wear leveling, copy-on-write filesystems (apfs, btrfs, zfs),
journaling, snapshots, backups and caches can keep old copies of the data.
for ssds, full-disk encryption (filevault, bitlocker, luks) is the more
reliable approach. on linux the tool reads sysfs and warns when the device is flash; on macos and
windows it does not check.

hard links share one copy of the data, so erasing one name erases the content
for all of them. running as root, a hard-linked file is not erased by default, whether it is
given directly or found inside a tree, because other names may point to the same data. a file given
directly is refused with exit code 1 before anything is erased; a file found inside a tree is refused
with exit code 4 and the tree is kept. `--i-know-what-i-am-doing` erases it anyway, with a warning. the rule covers root only, because an ordinary user can already change those files.
on unix the summary lists hard-linked files. on windows the link count is not read, so the rule
does not apply there and hard links are not detected: the summary does not list them.

a directory whose parent cannot be modified is not removed. its contents are erased first,
and the tool reports the error for the directory.

files on btrfs, zfs, apfs, refs and bcachefs trigger a warning, because those filesystems write
to new blocks and may keep the old data.

files on network filesystems (nfs, smb, smb2, cifs, afpfs, smbfs, webdav, ceph, glusterfs, 9p, afs,
coda, lustre), on fuse mounts (`fuse`, `fuse.*`, `fuseblk`, `macfuse`, `osxfuse`), on filesystem types
the tool does not recognize, and on remote drives also trigger a warning: the overwrite may happen on
a server, which may keep copies that this tool cannot see.

the warnings do not block the erase.

on ntfs only the default data stream is overwritten. alternate streams, such as the zone
identifier that windows adds to downloads, keep their clusters after the erase. compressed
files may be rewritten to new clusters, so the old data can survive (not tested on windows).

an erase that is interrupted can leave a file half overwritten. on ctrl+c or sigterm the tool
stops at the next write: the file it is working on keeps its name, but its contents may already be
partly overwritten. `interrupted: ...` is printed, the paths that were not reached are not touched,
and the run ends with exit code 4. a second signal exits at once with code 130 without restoring the
name, so the file can stay under a random hidden name in its directory (`.` followed by 16 hex digits).
a kill or a power loss has the same result. a directory can be left partly erased under its original
name. ctrl+c at a question is not caught: the tool stops at once and nothing is erased.

### locked files (windows only)

a file held open by another program is not erased by default.

- `--kill-holders` closes the programs that hold the file. it asks first, unless
  `-f` is given. services may be stopped too. if the question is declined, the file is
  not erased and the run ends with exit code 4.

if the file still cannot be deleted after its contents are erased, you are asked
whether to delete it at the next reboot. with `-f` nothing is scheduled and the
file stays in place; a declined question leaves it in place too, with exit code 4.

## before selling or giving away a device

erasing files does not guarantee that their data is gone from the device:

- ssds: wear leveling and remapped blocks mean a write goes to a new physical block. the old block
  can stay readable until the drive reuses it, and this tool cannot reach it.
- copy-on-write and journaling filesystems write new blocks or keep copies in a journal, so old data
  may survive an overwrite.
- snapshots and backups keep older versions of the files, which this tool does not reach. `--wipe-free`
  has the same limits, because it writes only through the filesystem.

this tool does not erase a whole device: it does not touch partitions, the partition table or space
outside the filesystems it is given. for a disk that leaves your hands, use an external tool:

- nwipe (linux) and ShredOS (a bootable image) wipe whole disks. both overwrite the disk or use the
  drive's own erase commands; check their documentation for what they do on your drive.
- the drive's own secure erase, through the vendor's tool.
- on macos, rely on the volume encryption from the limitations section (filevault), enabled before the
  data was written, instead of overwriting.

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
- network filesystems, fuse mounts, remote drives and filesystems whose type the tool does not
  recognize are refused: their free space may belong to a server or to other users. erase only
  warns about these types. on bsd systems the filesystem type is not determined, so `--wipe-free`
  is not available there.
- snapshots and copies the filesystem keeps are not free space and are not reached.
- other programs writing to the volume may fail while it is full.
- on large disks this takes a long time. with `-v` the tool prints a line for every
  gigabyte written and a total for each pass.

## excluding entries

`--exclude <glob>` skips matching entries inside the trees given with `-r`. it can be repeated, and
it needs `-r`.

- a pattern without `/` is compared with the name of an entry, at any depth. it matches files and
  directories alike.
- a pattern with `/` is compared with the path of an entry relative to its command-line argument:
  `--exclude 'src/*.o'` skips `build/src/x.o` when `build` is the argument.
- `*` matches any characters except `/`, and `?` matches one character except `/`. there are no
  other wildcards, so `[` is an ordinary character.
- a leading `./` and a trailing `/` are removed, so `build/` is the same pattern as `build`. an empty
  pattern, `.` and an absolute path are refused (exit code 2).
- command-line arguments are never excluded, even when they match.
- an excluded entry is neither erased nor walked. the directories that hold it are kept, and the run
  still ends with exit code 0. the summary shows `excluded by --exclude` and
  `directories kept because of --exclude`.

## large erases

with `-r`, a path with more than 1000 objects (files, directories and links) or more than 10 GiB is
not confirmed with `y`: its last name component must be typed instead, exactly as it is named. the
limits are checked for each path. when only the whole run is above them, the path with the most
objects is asked. when some paths are not named, `continue?` follows as well. an end of input or a
wrong name is refused (exit code 3). `-f` skips this check.

the name must match exactly. spaces at the start and end of what is typed are removed, so a name that
starts or ends with spaces cannot be confirmed.

## verify

`--verify` reads each file back after the last overwrite pass and compares it with what was written.
the last pass writes pseudo-random bytes from a random seed (not written to disk), so the check can
reproduce them and read the file in any order. only the last pass is checked: the earlier passes
(zeros, ones and random) are not read back, and the file is read once, after its last pass.

- a match erases the file as usual. a mismatch keeps the file under its name, prints
  `verify failed: contents differ after the last pass, the file is kept` and ends the run with exit
  code 4. the other paths are still erased.
- the read uses the same open file as the overwrite, after the data is synced. on linux the kernel's
  cache for the file is dropped first, as a hint. on other systems the read may come from the cache,
  so the check is weaker there.
- a match does not prove that the old data is gone. ssds, copy-on-write filesystems, caches and remapped
  blocks can keep an older copy that the check does not see. the note printed at the start of every
  run with `--verify` says so.
- the file is opened for reading as well as writing, so the owner's read bit is added when it is
  missing, as the write bit is for any erase. a kept file keeps that bit.
- not for devices yet: a device is refused with exit code 1 before anything is done, also with
  `--dry-run`. not with `--wipe-free`, which is an argument error (exit code 2). for files, `--dry-run`
  ignores it and no note is printed.

## lists of paths

`--files-from <FILE>` adds the paths listed in a file, one per line, to the ones on the command line.
`-` reads them from stdin. with `-0` (or `--null`) the entries are separated by nul bytes, as
`find -print0` writes them.

- a line is taken as it is: spaces belong to the path, and a `\r` at the end of a line stays in it,
  so a crlf list does not match the files.
- empty entries are skipped. the number of paths read is printed.
- `-` without `-f` is an argument error (exit code 2), refused before stdin is read, because the answer to the question cannot come from stdin. with `--dry-run` it is allowed.
- an unreadable list, or on windows a path that is not utf-8, is refused (exit code 1). on unix a
  path is kept as bytes, so it does not need to be utf-8.

## logs, json and progress

`--log <FILE>` writes one json object per line for every object the erase handles, after it is
handled:

```json
{"time":"2026-10-22T10:00:00Z","path":"build/a.o","kind":"file","size":4096,"passes":1,"result":"ok","detail":""}
```

- `time` is utc in rfc 3339, with whole seconds. `kind` is `file`, `dir`, `link` or `other` (fifos,
  sockets, devices, and entries that could not be inspected). `size` is in bytes. `passes` is set for files only, and is 0 for the rest.
- `result` is `ok`, `error`, `skipped`, `kept` or `refused`. `detail` holds the warning or error text,
  and is empty when there is none.
- the file must not exist, and it is never overwritten. on unix it is created with mode 0600, because
  the names can be sensitive.
- file contents are never logged. names that are not utf-8 are written with replacement characters.
- it is not written by `--dry-run`, and it cannot be combined with `--wipe-free`. a log inside the tree
  being erased is refused.
- a line that cannot be written stops the erase: the remaining paths are not erased, and the run ends
  with exit code 4.
- a line is written after its object is handled. if the process is killed with sigkill in the middle of
  an object, that object has no line, although it may be partly erased.

`--dry-run --json` prints the dry-run summary as one line of json on stdout instead of the text. it
has the keys `paths` and `passes`, the counts `files`, `dirs`, `links`, `bytes`, `hard_linked`,
`skipped`, `refused`, `unreadable`, `other`, `excluded` and `kept_dirs`, and `objects` (files, dirs and
links together). `--json` needs `--dry-run`.

a progress line is drawn on stderr during long writes: the overwrite of a file and `--wipe-free`. it is
drawn only when stderr is a terminal, at most twice a second. `-q` or `--quiet` turns it off; warnings
and errors are still printed.

## man page and completions

the man page and the shell completions are made by a dev-only example. they are not part of the
binary, so run this from a checkout with cargo:

```sh
cargo run --example gen_docs -- man target/docs
cargo run --example gen_docs -- completions bash target/docs
```

`man` writes the man page, and `completions` writes the completion file for `bash`, `zsh`, `fish` or
`powershell`. the directory is created if it is missing, and the path of each file is printed.

## exit codes

| code | meaning |
| --- | --- |
| 0 | everything was erased; a dry run (also with `--json`); `--wipe-free` finished; `--help` or `--version`; directories kept because of `--exclude` |
| 1 | a path or the request was refused before anything was erased: a missing file, a directory without `-r`, a protected path, a special file, a mount point given as a target, a hard-linked file under root given as a path, `--verify` on a device, `--wipe-free` on a file or on a network, fuse or unrecognized filesystem (also on bsd), a log that exists or lies inside the tree, an unreadable `--files-from` file, or an empty list when no other path is given |
| 2 | bad command line: an unknown flag, no paths, `-n 0`, an empty path argument (`""`), an empty `--exclude` pattern (`''`), an `--exclude` pattern that is `.` or absolute, `--json` without `--dry-run`, `--exclude` without `-r`, `--null` without `--files-from`, `--kill-holders` outside windows, paths, `--files-from`, `--log`, `--exclude`, `--json` or `--verify` together with `--wipe-free` |
| 3 | the confirmation was declined: an answer other than `y` or `yes`, no input at all, or a wrong name for a large erase, and the same for the question of `--wipe-free`. a signal that arrives before the first path is erased, `-f` included, also ends with `aborted` and this code. a signal that arrives while a question is open is not caught: it kills the process, and the tool has no exit code for it |
| 4 | the operation did not complete: at least one path was not erased (a hard-linked file under root found inside a tree is refused too, and so is a file whose deletion at reboot was declined), a log line could not be written, a `--verify` mismatch (the file is kept), `--wipe-free` wrote nothing or failed, or the erase was interrupted by ctrl+c or sigterm (`interrupted:` is printed) |
| 130 | a second ctrl+c or sigterm: the tool exits at once, without restoring the name of the file being erased |

with `-f` there is no question, so code 3 cannot come from a declined question. when stdin is not a
terminal and the question gets no answer, the tool says to use `-f`.

## license

copyright (c) 2026 kirill belolipetsky. mit license, see [LICENSE](LICENSE).

erased files cannot be recovered with this tool. use it with care.

## development

the same commands run in ci (`.github/workflows/ci.yml`) on ubuntu, macos and windows. formatting is
checked on ubuntu only, and a separate job checks the build with rust 1.88.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```
