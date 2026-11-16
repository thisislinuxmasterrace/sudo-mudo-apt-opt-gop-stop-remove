# usage

for flags, refusals and limits, see [reference](reference.md#flags), [safety](safety.md) and [guarantees and limits](guarantees-and-limits.md).

## invocation

every command, including help and version, requires `sudo mudo apt opt gop stop remove` in that order. put options after `remove`:

```sh
sudo mudo apt opt gop stop remove --help
sudo mudo apt opt gop stop remove --version
```

unix requires root plus sudo's user markers, checked against the account database. a shell started through sudo (`sudo -s`, `sudo -i`) keeps those markers and is accepted; a root login that did not go through sudo is refused. both the invoking user's home and the elevated home stay protected. ordinary unix runs therefore use the root hard-link refusal too.

on windows 11 24h2 or later, enable uac and inline sudo (`sudo config --enable normal` in an elevated console). from an elevated console, run `sudo mudo apt opt gop stop remove ...`. native system `sudo.exe` must be an ancestor, including through a node launcher. an administrator shell alone, third-party sudo, new-window mode and unverifiable ancestry are refused; the ordinary non-elevated uac handoff can also lose the required parent. review [microsoft's security considerations](https://learn.microsoft.com/en-us/windows/advanced-settings/sudo/#security-considerations) for inline sudo.

the invocation check is a cooperative guard against mistakes, not an authentication or security boundary.

## what an erase does

each file is renamed to a random hidden name, overwritten, then deleted. on unix its identity is checked after the rename and a file that cannot be removed is refused before overwrite. on windows the rename and the delete go through the open file handle (on filesystems without posix delete, such as fat, the file goes when the handle closes); if the rename fails, the file is overwritten under its own name, and a file that cannot be deleted is found only after the overwrite. `-n` sets the passes (default 1): zeros, ones, random, repeating, with a random last pass. extra passes add wear without making flash, copy-on-write or compressed storage more reliable. symlinks are removed without following them; inaccessible and read-only entries are made accessible to their owner first.

```sh
sudo mudo apt opt gop stop remove venom.exe # one pass
sudo mudo apt opt gop stop remove -n 3 venom.exe # three passes
sudo mudo apt opt gop stop remove --verify -f venom.exe # read the last pass back and compare
```

`--verify` is explained in [guarantees and limits](guarantees-and-limits.md#what---verify-proves).

## before anything is erased

every path is validated first. one refused path aborts the whole run with exit code 1 before anything is touched.

the confirmation lists the targets, a summary (counts, size, passes, and any skipped, excluded or unreadable entries) and the risks, then asks. `--dry-run` prints the same list and summary and changes nothing. `--dry-run --json` adds per-entry [assessments](guarantees-and-limits.md#assessments).

`--explain` is a dry run with advice. it lists what was found for these paths first: flash or copy-on-write storage, encryption, compression, editor backups by name, git history, sync and trash folders, snapshots. the general limits come last. for git it says whether a recursive erase also removes the repository's `.git` or leaves its history in place. commands are printed, never run. read-only `git`, `btrfs` and `zfs` queries may run for each command-line target, limited to two seconds each; failures stay unknown. on unix `git` runs as the invoking user, so a repository owned by someone else gives "could not tell". git checks the index and reachable history, not renames, reflogs, unreachable objects or remotes. btrfs checks direct snapshots of the subvolume, zfs the snapshots of the containing dataset; the json snapshot fact stays `unknown`. for a device it also gives advice. it cannot be combined with `--json` or `--wipe-free`.

with `-f` and no `--dry-run`, nothing is counted and no summary is shown: the notes come first, then the result. unix root runs still inspect a tree for internal hard links. the result names a single target and counts several; targets kept only because of `--exclude` are counted separately. on windows it says `processed`, because deletion may be queued for reboot.

`-q` hides the progress line, the general notes and the success line. warnings about the given paths, errors and questions remain. `-v` prints each pass. `--log` keeps per-entry facts.

on a terminal, headings are cyan, warnings yellow, errors red and results green. `NO_COLOR` (even empty) or `TERM=dumb` turns color off. redirected output, json and logs are always plain.

## confirmation

without `-f` the tool lists the first 20 paths (then `... and N more`) and asks for `y` or `yes`. anything else, or no input, ends the run with exit code 3 and `aborted, nothing was erased.`

with `-r`, a path over 1000 objects or 10 GiB requires its exact last name component instead of `y`.

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

a list from stdin needs `-f`, because confirmation also reads stdin. preview it first:

```sh
find build -name '*.tmp' -print0 | sudo mudo apt opt gop stop remove --dry-run --json -0 --files-from - | jq .objects
find build -name '*.tmp' -print0 | sudo mudo apt opt gop stop remove -f -0 --files-from -
```

for unused volume space, see [wipe-free](wipe-free.md). a [log](reference.md#log-format) is a record, not proof of erasure.

## in scripts

cron and ci have no terminal, so they need `-f`. `-q` keeps only warnings about the paths and errors. branch on the exit code:

```sh
sudo mudo apt opt gop stop remove -q -f -r /srv/scratch/old-run
case $? in
  0) echo erased ;;
  4) echo "not everything was erased" ;;
  *) echo "refused or failed" ;;
esac
```

codes and formats: [reference](reference.md). when stdin is not a terminal and a question gets no answer, the tool says to use `-f`.

## interrupts

the handlers are installed only while an erase runs, after the initial confirmation. at that initial question, ctrl+c kills the process at once and nothing is erased.

during an erase, the first ctrl+c, sigterm or sighup (a closing terminal) stops the write loops at the next write. the file being overwritten gets its name back (on windows when the rename worked), but its contents may already be partly overwritten. closing the console window on windows waits up to four seconds for this stop. `interrupted: ...` is printed, paths not reached are not touched, and the run ends with exit code 4. a signal before the first path is erased ends with `aborted` and exit code 3.

a second signal exits at once with code 130 and does not restore the name, so a file can stay under its hidden name. a kill or a power loss has the same result, and a directory can be left partly erased under its original name.

## locked files on windows

a file held open by another program is not erased by default.

- `--kill-holders` closes the programs that hold the file. it asks first unless `-f` is given. the tool asks the windows restart manager to shut the holders down and forces it when they do not close (`RmForceShutdown`), so services among them are stopped too. only the programs shown in the question are closed: if a program that was not listed holds the file by then, nothing is closed (a listed program that has exited meanwhile does not matter), the file is kept and the run ends with exit code 4. if the question is declined, the file is not erased and the run ends with exit code 4.
- after holders close, the overwrite uses the size of the file opened on retry.
- ctrl+c during either windows question ends the wait for input and stops the operation before closing holders or scheduling reboot deletion. an interrupt observed after an answer also prevents the action; signal delivery and the action are not atomic.
- if the file still cannot be deleted after its contents are erased, the tool asks whether to delete it at the next reboot. with `-f` nothing is scheduled and the file stays; a declined question leaves it in place too. both end with exit code 4.

`--kill-holders` outside windows is an argument error (exit code 2). the lock scenarios have run on a windows ci runner and, in an earlier version, in a windows 11 virtual machine. they have not been tested on real windows hardware; see [verification limits](guarantees-and-limits.md#what-is-not-verified).
