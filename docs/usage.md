# usage

how to erase files and trees. flags are listed in the [reference](reference.md#flags); what the tool refuses is in [safety](safety.md); what an erase does and does not guarantee is in [guarantees and limits](guarantees-and-limits.md).

contents: [what an erase does](#what-an-erase-does) · [before anything is erased](#before-anything-is-erased) · [confirmation](#confirmation) · [trees](#trees) · [recipes](#recipes) · [in scripts](#in-scripts) · [interrupts](#interrupts) · [locked files on windows](#locked-files-on-windows)

the examples use the full binary name, `sudo-mudo-apt-opt-gop-stop-remove`. it has no short alias on purpose.

## what an erase does

for each file the tool:

1. renames it to a random hidden name in the same directory (`.` followed by 16 hex digits). on unix it also checks that the renamed entry is still the same file, and a file that cannot be removed is refused before its contents are touched.
2. overwrites the contents. `-n` sets the number of passes, default 1. passes run in the order zeros, ones, random, and repeat; the last pass is always random. extra passes only add wear: on flash, copy-on-write and compressing filesystems they do not make the erase more reliable.
3. deletes it.

a symbolic link is removed as a link and never followed, so its target is untouched. read-only files and directories, and entries the owner cannot access at all, are made accessible to their owner first.

```sh
sudo-mudo-apt-opt-gop-stop-remove venom.exe             # one pass
sudo-mudo-apt-opt-gop-stop-remove -n 3 disk-dump.img    # three passes
sudo-mudo-apt-opt-gop-stop-remove --verify -f notes.txt # read the last pass back and compare
```

`--verify` is explained in [guarantees and limits](guarantees-and-limits.md#what---verify-proves).

## before anything is erased

every path is validated first. one refused path aborts the whole run with exit code 1 before anything is touched.

then the tool prints a note that overwriting does not guarantee erasure (once per run, also with `-f`) and a summary of what it will touch: files, directories, links, total size, hard-linked files, skipped mount points, excluded entries, special files (refused) and entries it could not read. then it asks (see below).

`--dry-run` prints the same summary and changes nothing:

```text
would erase build (1 passes)
  ...summary lines...
```

the text summary shows up to 20 entry assessments; `--dry-run --json` includes all inspected entries. directory metadata and child entries are assessed separately. skipped, excluded, refused, special or unreadable entries limit coverage; see [assessments](guarantees-and-limits.md#assessments).

`--explain` does a dry run and then prints advice for the paths. each condition found (flash storage, an unknown medium, a copy-on-write, network or fuse filesystem, a symbolic link, a git work tree, sync folder, trash folder or editor backups) gets one line, with commands that would help. the commands are printed and never run, and the text does not claim that data is unrecoverable. it asks nothing and erases nothing. for a device it only shows the dry run, and it does not combine with `--json` or `--wipe-free`.

with `-f` and no `--dry-run` the preliminary summary is not shown and the tree is not counted. assessments are collected during the erase walk and printed on stderr, without another walk or question.

## confirmation

without `-f` the tool lists the first 20 paths (then `... and N more`) and asks for `y` or `yes`. anything else, or no input, ends the run with exit code 3 and `aborted, nothing was erased.`

with `-r`, a path with more than 1000 objects (files, directories and links) or more than 10 GiB is not confirmed with `y`. its last name component must be typed instead, exactly as named, so a mistyped command is less likely to erase a big tree.

- the limits are checked for each path. when only the whole run is above them, the path with the most objects is asked.
- when not every path is named, `continue?` follows.
- spaces at both ends of what is typed are removed, so a name that starts or ends with a space cannot be confirmed.
- end of input or a wrong name: exit code 3.

`-f` skips every question: this confirmation, the strict one, the windows lock-holder question and the reboot-deletion question. warnings are still printed. `-f` never lifts a refusal: see [safety](safety.md).

## trees

a directory needs `-r`. the tool walks it depth-first and erases the contents before the directory.

- `--exclude <glob>` skips matching entries; the directories that hold them are kept and the run still ends with exit code 0. rules: [reference](reference.md#exclude-globs).
- a mount point or another filesystem inside the tree is skipped and reported, the directory that holds it is kept, and the run ends with exit code 4. see [safety](safety.md#mount-points).
- a directory whose parent cannot be modified is not removed: its contents are erased first, then the error is reported for the directory.
- a path given twice is erased once.

## recipes

secret files (`.env`, keys, dumps): look first, then erase.

```sh
sudo-mudo-apt-opt-gop-stop-remove --dry-run .env id_ed25519 dump.sql
sudo-mudo-apt-opt-gop-stop-remove .env id_ed25519 dump.sql
```

a build directory, keeping what you want to keep:

```sh
sudo-mudo-apt-opt-gop-stop-remove -r --exclude '*.lock' --exclude .git build/
```

a list of paths from `find`. a list on stdin needs `-f` (the question would also read stdin), so check the plan with a dry run first:

```sh
find build -name '*.tmp' -print0 | sudo-mudo-apt-opt-gop-stop-remove --dry-run --json -0 --files-from - | jq .objects
find build -name '*.tmp' -print0 | sudo-mudo-apt-opt-gop-stop-remove -f -0 --files-from -
```

a record of what was erased, for handing over a machine or a directory. a record is not proof of erasure: see [guarantees and limits](guarantees-and-limits.md#what---verify-proves).

```sh
sudo-mudo-apt-opt-gop-stop-remove -r --log erase-$(date +%F).jsonl old-project/
```

old data in the unused space of a volume: see [wipe-free](wipe-free.md).

## in scripts

cron and ci have no terminal, so they need `-f`. `-q` turns the progress line off. branch on the exit code:

```sh
sudo-mudo-apt-opt-gop-stop-remove -q -f -r /srv/scratch/old-run
case $? in
  0) echo erased ;;
  4) echo "not everything was erased" ;;
  *) echo "refused or failed" ;;
esac
```

codes and formats: [reference](reference.md). when stdin is not a terminal and a question gets no answer, the tool says to use `-f`.

## interrupts

the handlers are installed only while an erase runs, after the confirmation. at a question, ctrl+c kills the process at once and nothing is erased.

during an erase, the first ctrl+c, sigterm or sighup (a closing terminal) stops the write loops at the next write. the file being overwritten gets its name back, but its contents may already be partly overwritten. `interrupted: ...` is printed, paths not reached are not touched, and the run ends with exit code 4. a signal before the first path is erased ends with `aborted` and exit code 3.

a second signal exits at once with code 130 and does not restore the name, so a file can stay under its hidden name. a kill or a power loss has the same result, and a directory can be left partly erased under its original name.

## locked files on windows

a file held open by another program is not erased by default.

- `--kill-holders` closes the programs that hold the file. it asks first unless `-f` is given. the tool asks the windows restart manager to shut the holders down and forces it when they do not close (`RmForceShutdown`), so services among them are stopped too. if the question is declined, the file is not erased and the run ends with exit code 4.
- if the file still cannot be deleted after its contents are erased, the tool asks whether to delete it at the next reboot. with `-f` nothing is scheduled and the file stays; a declined question leaves it in place too. both end with exit code 4.

`--kill-holders` outside windows is an argument error (exit code 2). this code has not been tested on real windows hardware; ci runs the scenarios in `tests/windows/locks.ps1` on a windows runner.
