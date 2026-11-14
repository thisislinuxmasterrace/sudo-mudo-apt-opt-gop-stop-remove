# architecture

a code map for contributors. read this first, then open only the module you change. behaviour is described on the topic pages: [usage](usage.md), [safety](safety.md), [devices](devices.md), [wipe-free](wipe-free.md), [reference](reference.md).

contents: [overview](#overview) · [module map](#module-map) · [data flow](#data-flow) · [invariants](#invariants) · [where each concern lives](#where-each-concern-lives) · [tests](#tests)

## overview

one binary crate, package `sudo-mudo-apt-opt-gop-stop-remove`, executable `mudo`, with two runtime dependencies on every platform (`clap`, `rand`), `nix` and `libc` on unix, and `windows-sys` on windows. there is no library crate. `src/invocation/` checks the sudo launch before parsing, including help and version. unix resolves sudo's invoking account for home protection; windows checks elevation and native system sudo process ancestry. `examples/gen_docs.rs` includes `src/cli.rs` to build the man page and completions; it uses dev-dependencies only, so the binary does not change. fish uses an explicit phrase predicate around generated leaf options because clap's fish generator limits subcommand depth.

the code has three erase paths: files and trees (`src/erase/`), whole block devices on linux (`src/device/`) and the free space of a volume (`src/wipe.rs`). they share validation (`safety`), warnings (`storage`), the overwrite patterns, the interrupt flag and the log.

## module map

| path | role |
| --- | --- |
| `src/main.rs` | argument handling, mode dispatch, exit code constants, `NAME` |
| `src/cli.rs` | clap definition, flags, defaults, the `--exclude` pattern parser |
| `src/paths.rs` | `--files-from` reading, path normalization |
| `src/run.rs` | `run_erase` and `run_wipe_free`: validation, dry run, log, erase loop, exit code |
| `src/confirm.rs` | the confirmation: the plain one, the strict one for big `-r` runs, the wipe one |
| `src/prompt.rs` | the single yes/no question helper, and the exact-text question |
| `src/preview.rs`, `src/preview/json_output.rs`, `src/preview/tests.rs` | counts and assesses visited entries; text summary, dry-run json and unit tests |
| `src/assessment.rs`, `src/assessment/windows.rs`, `src/assessment/tests.rs` | conservative per-entry verdict, reasons and facts shared by preview and erase reports; windows binds metadata and filesystem type to one no-follow handle |
| `src/explain.rs` | `--explain`: advice lines from the assessment facts; commands are printed, never run |
| `src/copies.rs` | advisory search for other copies by path: git work tree, sync folder, trash, editor backups |
| `src/safety.rs` | path validation (`validate_target`, `validate_wipe_dir`), protected locations |
| `src/storage/` | filesystem checks: `mod.rs` (type lists, `classify`, warnings), `fs_name.rs` (filesystem name per os, linux magic table, mountinfo lookups, windows volume root), `flash.rs` (sysfs flash detection, and `block_dir`, the sysfs directory of a path's device), `crypt.rs` (dm-crypt and ecryptfs detection), `compression.rs` (compression from the mount options), `space.rs` (`available_bytes`) |
| `src/erase/` | erase of files, directories and symlinks. see below |
| `src/entry.rs` | classification of unsupported windows reparse points |
| `src/verify.rs` | `--verify`: the read-back check, and the bytes of the last pass made from a seed |
| `src/wipe.rs` | `--wipe-free`: fills free space with temporary files, pass by pass |
| `src/device/` | whole block devices, linux only (`mod.rs` is the entry, `is_device_run`; elsewhere refused with code 1). `linux/`: `target` (names, overlaps), `facts` (refusals, layout), `summary`, `log_check`, `write` (`O_DIRECT`), `sysfs`, `mounts` |
| `src/log.rs`, `src/log/tests.rs` | the `--log` json lines writer (`create_new`, 0600 on unix, checks against the erased tree) and its unit tests |
| `src/json.rs` | json written by hand, no `serde_json`, for `--log` and `--dry-run --json`; escaping in one place |
| `src/progress.rs` | the progress line on stderr, drawn only on a terminal |
| `src/platform/` | windows-only ffi (`windows.rs`): restart manager, posix delete, reboot deletion |

`src/erase/`:

| file | role |
| --- | --- |
| `mod.rs` | entry points and `Options` |
| `unix.rs`, `at.rs` | unix walk through open directory descriptors; the `openat`-family primitives |
| `win.rs` | windows: walk by path, lock handling, posix delete (calls into `platform`) |
| `walk.rs` | tree walk and the `--exclude` matcher (`glob_match`) |
| `mount.rs` | mount points and device ids |
| `hard_link.rs` | the hard link rule |
| `overwrite.rs`, `pattern.rs` | passes and overwrite patterns |
| `sparse.rs` | linux data ranges from the checked file descriptor, with full-range fallback |
| `interrupt.rs` | ctrl+c, sigterm and sighup during an erase |
| `report.rs` | captured assessments, stderr reports, log lines and their stop |
| `tests.rs` | unit tests of the erase |
| `swap_tests.rs` | unix: an entry swapped between the check and the act, through the test-only hooks in `at::seam` |

## data flow

`main` parses the command line, rejects `--kill-holders` outside windows, reads `--files-from`, normalizes and de-duplicates the paths, then calls one of two runs.

`run_erase`:

1. a block device among the paths hands the whole run to `device::run`.
2. validate every path (`safety`, hard link rule, mount-point target); any failure ends with exit code 1 before anything is touched.
3. check the `--log` location, print storage warnings.
4. count the tree (`preview`), unless `-f` and no dry run.
5. dry run: print the summary or json and return.
6. print the notes, ask the confirmation (strict where needed), install the interrupt handlers, create the log.
7. erase each path (`erase::erase_path`), collecting each entry's advisory facts before rename or deletion and reporting them with log lines; map the outcome to an exit code.

`run_wipe_free` validates the directory, refuses network and unknown filesystems, shows the space, asks, installs the handlers and calls `wipe::wipe_free_space`.

## invariants

- nothing is erased before every path is validated, and nothing before the confirmation. the interrupt handlers exist only from the confirmation to the end of the erase.
- symlinks are never followed. paths are normalized with `components()` first, so `link/` is a link.
- on unix a file is renamed to a hidden name and checked to be the same inode before it is overwritten and deleted. a file that cannot be removed is refused before its contents are touched. directories still erase their contents first, because a mount point inside them cannot be renamed away.
- the unix `-r` walk uses `fstatat`, `renameat` and `unlinkat` on open directory descriptors, so a swapped directory cannot redirect the erase.
- on unix every delete (file, link, emptied directory) first renames the entry to a random hidden name and checks it against the stat taken before (device, inode, type). a detected substitution is restored when possible; the later unlink goes by the hidden name and is not atomic with the identity check. on linux the renames use `RENAME_NOREPLACE`, so neither the hidden name nor the original one, when it is put back, replaces an entry created in between; elsewhere, and on filesystems without it, the name is checked just before the rename, which leaves a small window.
- before opening an entry, the owner's bits needed for access are added. on linux through an `O_PATH` handle and `/proc/self/fd`; elsewhere by name with the identity checked first.
- anything that cannot be read fails closed: the mount table, sysfs for devices, the filesystem type for `--wipe-free`.
- a protected-path comparison on unix is by `(dev, ino)`, not by spelling. `is_system_protected` reads no environment variable on unix.
- `--log` is created with `create_new`, flushed per line, and a failed write stops the erase.
- the last pass is always random, and `--verify` can reproduce it from a seed held in memory only.
- no `unwrap()` or `expect()` outside tests; every `unsafe` block carries a `// safety:` comment.

## where each concern lives

| to change | look at |
| --- | --- |
| a flag or its default | `src/cli.rs`, then the `Options` in `src/erase/mod.rs` and `src/main.rs` |
| what is refused | `src/safety.rs` (paths), `src/erase/hard_link.rs`, `src/erase/mount.rs`, `src/device/linux/facts.rs` |
| exit codes | constants in `src/main.rs`, mapping in `src/run.rs` and `src/device/linux/mod.rs` |
| warnings and filesystem lists | `src/storage/mod.rs` |
| confirmation wording and limits | `src/confirm.rs` |
| the summary and its json | `src/preview.rs` |
| overwrite patterns and passes | `src/erase/pattern.rs`, `src/erase/overwrite.rs` |
| `--exclude` matching | `src/erase/walk.rs`, parser in `src/cli.rs` |
| log format | `src/log.rs`, `src/erase/report.rs` |
| windows behaviour | `src/erase/win.rs`, `src/platform/windows.rs` |

a behaviour change usually needs the matching page in `docs/` updated; `README.md` stays a landing page.

## tests

integration tests run the real binary and are grouped by topic in `tests/*.rs`: `assessments`, `erase_basic`, `exclude`, `exit_codes`, `explain`, `files_from`, `hard_links_and_mounts`, `log_json`, `progress_quiet`, `prompt`, `signals`, `sparse`, `strict_confirm`, `verify`, `wipe_free`. helpers are in `tests/common/mod.rs`. unit tests sit next to the code. scripts that are not part of `cargo test` are in [development](development.md#scripts).
