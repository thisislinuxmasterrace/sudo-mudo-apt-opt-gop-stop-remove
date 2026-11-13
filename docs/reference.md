# reference

every flag, exit code and file format. `sudo-mudo-apt-opt-gop-stop-remove --help` is the authoritative flag list; this page follows it.

contents: [flags](#flags) · [exit codes](#exit-codes) · [exclude globs](#exclude-globs) · [path lists](#path-lists) · [log format](#log-format) · [dry-run json](#dry-run-json) · [progress](#progress) · [man page and completions](#man-page-and-completions)

## flags

usage: `sudo-mudo-apt-opt-gop-stop-remove [OPTIONS] [PATHS]...`

| flag | default | meaning |
| --- | --- | --- |
| `[PATHS]...` | | files to erase. required unless `--wipe-free` or `--files-from` is given. a block device switches the run to [device erase](devices.md) |
| `-r`, `--recursive` | off | erase directories and their contents |
| `-n`, `--passes <N>` | 1 | number of overwrite passes, at least 1. zeros, ones, random, repeating; the last is random |
| `--dry-run` | off | show what would be erased and change nothing |
| `--explain` | off | a dry run that also prints advice for the paths: what an overwrite can and cannot do, and commands that would help (printed, never run) |
| `--json` | off | with `--dry-run`: the summary as one line of json on stdout. needs `--dry-run` |
| `-f`, `--force` | off | ask no questions (confirmation, lock holders, reboot). warnings are still printed. does not lift refusals |
| `--exclude <GLOB>` | | skip matching entries inside `-r` trees. repeatable. needs `-r` |
| `--files-from <FILE>` | | read more paths from `FILE`, one per line. `-` is stdin |
| `-0`, `--null` | off | with `--files-from`: entries are separated by nul bytes |
| `--log <FILE>` | | write a json lines log. the file must not exist |
| `--verify` | off | read the last pass back and compare. see [guarantees and limits](guarantees-and-limits.md#what---verify-proves) |
| `--wipe-free <DIR>` | | erase the free space of the filesystem holding `DIR`. see [wipe-free](wipe-free.md) |
| `-v`, `--verbose` | off | print progress for each pass |
| `-q`, `--quiet` | off | no progress line. warnings and errors are still printed |
| `--kill-holders` | off | close processes that hold a locked file. windows only; elsewhere exit code 2 |
| `--i-know-what-i-am-doing` | off | allow erasing protected system paths, and hard-linked files under root. a warning is printed every time. `-f` does not imply it |
| `-h`, `--help` | | print help (`-h` short, `--help` long) |
| `-V`, `--version` | | print the version |

combinations that are argument errors (exit code 2): `--json` without `--dry-run`, `--explain` together with `--json` or `--wipe-free`, `--exclude` without `-r`, `-0` without `--files-from`, `--files-from -` without `-f` (unless `--dry-run`), and `--wipe-free` together with paths, `--files-from`, `--exclude`, `--json`, `--log` or `--verify`.

## exit codes

| code | meaning |
| --- | --- |
| 0 | everything was erased; a dry run (also with `--json`); `--wipe-free` finished; `--help` or `--version`; directories kept because of `--exclude` |
| 1 | refused or failed before anything was erased: a missing path, a directory without `-r`, a protected path, a special file, a mount point given as a target, a hard-linked file under root given as a path, a device refusal, `--verify` or `-r` on a device, `--wipe-free` on a file or on a network, fuse or unrecognized filesystem, a log that exists or lies inside the tree, an unreadable `--files-from` file, an empty list when no other path is given |
| 2 | bad command line: an unknown flag, no paths, `-n 0`, an empty path argument (`""`), an empty `--exclude` pattern, an `--exclude` pattern that is `.` or absolute, any combination listed under [flags](#flags), `--kill-holders` outside windows |
| 3 | declined: an answer other than `y` or `yes`, no input, a wrong name for a large erase, the same for `--wipe-free`, a device run without a terminal, a wrong device path. also a signal that arrives before the first path is erased (`aborted` is printed), `-f` included |
| 4 | not finished: at least one path was not erased (including a mount point skipped inside a tree, a hard-linked file under root found inside a tree, a declined reboot-deletion or lock-holder question, and `-f` leaving a locked file), a log line could not be written, a `--verify` mismatch, `--wipe-free` wrote nothing or failed, or the erase was interrupted (`interrupted:` is printed) |
| 130 | a second ctrl+c, sigterm or sighup: the tool exits at once without restoring the name of the file being erased |

with `-f` there is no question, so code 3 cannot come from a declined question. a signal that arrives while a question is open is not caught: it kills the process and the tool has no exit code for it.

## exclude globs

`--exclude <glob>` skips matching entries inside the trees given with `-r`.

- a pattern without `/` is compared with the name of an entry at any depth, files and directories alike.
- a pattern with `/` is compared with the path of an entry relative to its command-line argument: `--exclude 'src/*.o'` skips `build/src/x.o` when `build` is the argument.
- `*` matches any characters except `/`. `?` matches one character except `/`. there are no other wildcards, so `[` is an ordinary character and a name that contains `*` or `?` cannot be matched literally.
- a leading `./` and a trailing `/` are removed, so `build/` is `build`. an empty pattern, `.` and an absolute path are refused (exit code 2).
- command-line arguments are never excluded.
- an excluded entry is neither erased nor walked. the directories that hold it are kept and the run still ends with exit code 0. the summary shows `excluded by --exclude` and `directories kept because of --exclude`.

## path lists

`--files-from <FILE>` adds paths from a file to the ones on the command line. `-` reads stdin. with `-0` the entries are separated by nul bytes, as `find -print0` writes them.

- a line is taken as it is: spaces belong to the path. one `\r` before the line break is dropped, so crlf lists work. with `-0` nothing is dropped.
- empty entries are skipped. `note: N paths read from FILE` is printed on stderr.
- `-` without `-f` is exit code 2, refused before stdin is read, because the question could not be answered from stdin. with `--dry-run` it is allowed.
- an unreadable list is exit code 1, and so is an empty list when no other path is given. on windows a path that is not utf-8 is refused (exit code 1); on unix paths are bytes.

## log format

`--log <FILE>` writes one json object per line for every object the erase handles, after it is handled:

```json
{"time":"2026-10-22T10:00:00Z","path":"build/a.o","kind":"file","size":4096,"passes":1,"result":"ok","detail":""}
```

| field | values |
| --- | --- |
| `time` | utc, rfc 3339, whole seconds |
| `path` | the path as handled. names that are not utf-8 are written with replacement characters |
| `path_bytes` | only when the path is not utf-8, on unix: its raw bytes in lowercase hex (`61ff62` for `a`, `0xff`, `b`). windows paths stay lossy |
| `kind` | `file`, `dir`, `link`, `other` (fifos, sockets, entries that could not be inspected); `device` or `partition` for a device run |
| `size` | bytes |
| `passes` | the number of passes for files; 0 for everything else |
| `result` | `ok`, `error`, `skipped`, `kept` or `refused` |
| `detail` | the warning or error text, empty when there is none |
| `verdict` | ordinary file runs: `weak` or `cosmetic`; `effective` is reserved and never issued automatically |
| `reasons` | array of assessment reasons, separate from the operation result |
| `facts` | `filesystem` (name or `null`), `medium` (`flash`, `rotational`, `unknown`), `compression` and `snapshots` (`unknown`), `encryption` (`not_checked`), `other_copies` (`found`, `none_found` or `not_checked`), `copies` (array of `{"kind", "path"}` with `kind` one of `git_work_tree`, `sync_folder`, `trash`, `editor_backup`; empty unless `other_copies` is `found`) |

assessment fields are collected before rename or deletion, even though the line is written afterwards. directory assessments describe metadata only; children have their own records. uninspected or skipped entries do not establish overwrite effectiveness. see [assessments](guarantees-and-limits.md#assessments). device records keep their existing format.

- the file is created with `create_new`, so an existing file is never overwritten. on unix it gets mode 0600, because the names can be sensitive.
- contents are never logged. a `--log` line is a record of what the tool did; it does not show that the data is unrecoverable (see [guarantees and limits](guarantees-and-limits.md#what---verify-proves)).
- `--dry-run` does not write it. a log path inside the erased tree is refused by device and inode of its directory chain. exit code 1.
- a line that cannot be written stops the erase: the remaining paths are not erased and the run ends with exit code 4.
- a line is written after its object is handled. a sigkill in the middle of an object leaves no line for it, although it may be partly erased.

## dry-run json

`--dry-run --json` prints one line of json on stdout instead of the text summary:

| key | value |
| --- | --- |
| `paths` | array of the paths. names that are not utf-8 are written with replacement characters |
| `path_bytes` | on unix, present only when a path is not utf-8: an array matching `paths`, with lowercase hex for such paths and `null` for utf-8 paths |
| `passes` | number of passes |
| `files`, `dirs`, `links` | counts |
| `objects` | files, dirs and links together |
| `bytes` | total size |
| `hard_linked` | hard-linked files |
| `skipped` | mount points and other filesystems skipped |
| `refused`, `unreadable`, `other` | refused entries, entries that could not be read, special files |
| `excluded` | entries excluded by `--exclude` |
| `kept_dirs` | directories kept because of `--exclude` |
| `assessments` | array for inspected entries: `path`, optional unix `path_bytes`, `verdict`, `reasons`, `facts` with the meanings above |
| `assessment_coverage` | `incomplete` when entries were skipped, excluded, refused or unreadable; otherwise `visited_entries` |

`--json` is not supported for devices: that is exit code 1.

## progress

a progress line is drawn on stderr during long writes (the overwrite of a file, `--wipe-free`), only when stderr is a terminal, at most twice a second. `-q` turns it off. it never changes what is erased or the exit code.

## man page and completions

the man page and shell completions come from a dev-only example. they are not part of the binary, so run it from a checkout:

```sh
cargo run --example gen_docs -- man target/docs
cargo run --example gen_docs -- completions bash target/docs
```

`completions` takes `bash`, `zsh`, `fish` or `powershell`. the directory is created if missing and the path of each file is printed.
