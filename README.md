# sudo-mudo-apt-opt-gop-stop-remove

when you really want to delete something

a command-line tool for windows, macos and linux. it overwrites file contents before deleting files and can erase directory trees. it can also fill the free space of a volume (`--wipe-free`) and, on linux, overwrite a whole disk or partition.

## what overwriting can and cannot do

file runs report an advisory verdict with reasons in the summary, dry-run json and log. `weak` means best effort; `cosmetic` means removing a link without overwriting its target. `effective` is reserved for proven conditions and is not issued automatically. directory entries describe metadata only; their visited children get separate assessments. see [assessment limits](docs/guarantees-and-limits.md#assessments).

the tool overwrites through the filesystem. a hard disk with a filesystem that rewrites blocks in place (ext4, xfs, ntfs, fat) offers the best chance of replacing the original file data, but this tool does not prove that every copy is gone. on ssd and nvme drives, on copy-on-write filesystems (apfs, btrfs, zfs, refs), with journals, snapshots, backups, caches or remote storage, an older copy can survive where the tool cannot reach it. when the data matters, use full-disk encryption from the start, or the drive's own secure erase. details: [guarantees and limits](docs/guarantees-and-limits.md).

status: version 0.1.0, early development. windows has been exercised in ci and in a limited windows 11 virtual machine test, not on real windows hardware. erasing devices on linux is experimental and not tested on real disks. the full list is in [guarantees and limits](docs/guarantees-and-limits.md#what-is-not-verified).

## install

there is no release yet. release archives, npm, homebrew and scoop are prepared and described in [distribution](docs/distribution.md), but nothing is published.

build from source (rust 1.88 or newer):

```sh
cargo build --release   # target/release/sudo-mudo-apt-opt-gop-stop-remove
```

## quick start

the examples use the full binary name. it has no short alias on purpose.

```sh
# look first, change nothing
sudo-mudo-apt-opt-gop-stop-remove --dry-run -r build/

# look first and get advice for these paths, change nothing
sudo-mudo-apt-opt-gop-stop-remove --explain notes.txt

# erase a file; asks for confirmation
sudo-mudo-apt-opt-gop-stop-remove notes.txt

# erase a tree, keeping some entries
sudo-mudo-apt-opt-gop-stop-remove -r --exclude '*.lock' --exclude .git build/

# scripts and cron have no terminal: -f asks no questions, -q hides progress
sudo-mudo-apt-opt-gop-stop-remove -q -f -r /srv/scratch/old-run

# paths from find, with a json lines record of what was done
find build -name '*.tmp' -print0 | sudo-mudo-apt-opt-gop-stop-remove -f -0 --files-from - --log erase.jsonl

# the free space of a volume, after the files are already deleted
sudo-mudo-apt-opt-gop-stop-remove --dry-run --wipe-free /home
sudo-mudo-apt-opt-gop-stop-remove --wipe-free /home
```

the exit code tells what happened: 0 done, 1 refused before erasing, 2 bad arguments, 3 declined, 4 not finished. see the [reference](docs/reference.md#exit-codes).

## documentation

| page | content |
| --- | --- |
| [docs/usage.md](docs/usage.md) | erasing files and trees, confirmations, recipes, interrupts, locked files on windows |
| [docs/safety.md](docs/safety.md) | what is refused and why: symlinks, protected paths, mount points, hard links, root |
| [docs/guarantees-and-limits.md](docs/guarantees-and-limits.md) | what an overwrite does on each kind of storage, what `--verify` proves, what is not verified |
| [docs/wipe-free.md](docs/wipe-free.md) | filling and clearing the free space of a volume |
| [docs/devices.md](docs/devices.md) | erasing a whole disk or partition (linux) |
| [docs/reference.md](docs/reference.md) | flags, exit codes, `--log` and `--dry-run --json` formats, `--files-from`, `--exclude` |
| [docs/architecture.md](docs/architecture.md) | code map and invariants for contributors |
| [docs/development.md](docs/development.md) | build, checks, tests, ci, msrv |

the full index is [docs/README.md](docs/README.md).

## license

copyright (c) 2026 kirill belolipetsky. mit license, see [LICENSE](LICENSE).
