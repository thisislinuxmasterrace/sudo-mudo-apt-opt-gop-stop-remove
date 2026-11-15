# architecture

a code map for contributors. user behaviour is described in the [topic pages](README.md).

## overview

one binary crate, executable `mudo`; there is no library. `src/invocation/` checks sudo before parsing, including help and version, and protects the invoking unix home. `examples/gen_docs.rs` generates the man page and completions from `src/cli.rs`; fish uses a phrase predicate because clap's generator limits subcommand depth.

the code has three erase paths: files and trees (`src/erase/`), whole block devices on linux (`src/device/`) and the free space of a volume (`src/wipe.rs`). they share validation (`safety`), warnings (`storage`), the overwrite patterns, the interrupt flag and the log.

## module map

| path | role |
| --- | --- |
| `src/main.rs`, `cli.rs`, `paths.rs`, `run.rs` | invocation, arguments, path lists and dispatch |
| `src/confirm.rs`, `prompt.rs` | confirmations and prompts |
| `src/preview/`, `preview.rs`, `assessment.rs`, `assessment/` | summaries, json and per-entry assessments |
| `src/explain.rs`, `copies.rs` | advice and possible copies |
| `src/advisory.rs`, `copies/git.rs`, `snapshots.rs` | bounded read-only tool lookups for explicit explanations |
| `src/safety.rs`, `storage/`, `entry.rs` | path protection, storage facts and windows reparse classification |
| `src/erase/`, `verify.rs` | file and tree erasure, read-back verification |
| `src/wipe.rs`, `device/` | free-space and linux device erasure |
| `src/log.rs`, `json.rs`, `progress.rs` | reports and progress |
| `src/ui.rs` | human output, stream-specific terminal colors, headings, prompts and result summaries |
| `src/platform/` | windows restart manager, posix delete and reboot deletion |

`src/erase/` separates unix descriptor-based and windows path-based walks, mount and hard-link checks, overwrite patterns, sparse ranges, interrupts and reports. its tests include unix entry substitution through test-only hooks.

`erase/streams.rs` enumerates windows named data streams, opens them before writing and compares their file identities with the main stream. directory streams are refused. recursive walks share a 64-level bound; unix hard-link inventory encounters this bound before content erasure.

## data flow

`main` parses the command line, rejects `--kill-holders` outside windows, reads `--files-from`, normalizes and de-duplicates the paths, then calls one of two runs.

`run_erase`:

1. a block device among the paths hands the whole run to `device::run`.
2. validate every path (`safety`, hard link rule, mount-point target); any failure ends with exit code 1 before anything is touched.
3. check the `--log` location, print storage warnings.
4. count the tree (`preview`), unless `-f` and no dry run.
5. dry run: print the summary or json and return.
6. print the notes, ask the confirmation (strict where needed), install the interrupt handlers, create the log.
7. erase each path (`erase::erase_path`), collecting each entry's advisory facts before rename or deletion for the json log; map the outcome to an exit code. text prints risks once and keeps per-entry errors. unix root runs inventory internal hard links (`erase/tree_links.rs`) through checked directory descriptors, unlink redundant selected names, then overwrite the last name only after a fresh handle link-count check.

`run_wipe_free` validates the directory, refuses network and unknown filesystems, shows the space, asks, installs the handlers and calls `wipe::wipe_free_space`.

## invariants

- nothing is erased before every path is validated, and nothing before the confirmation. the interrupt handlers exist only from the confirmation to the end of the erase.
- symlinks are never followed. paths are normalized with `components()` first, so `link/` is a link.
- unix renames each entry and checks its identity before deletion; files that cannot be removed are refused before overwrite. the descriptor-based recursive walk cannot be redirected by a swapped directory. a detected substitution is restored when possible, but the later unlink is not atomic with the check. linux uses `RENAME_NOREPLACE` where supported; the fallback checks the name before renaming and leaves a small window.
- before opening an entry, the owner's bits needed for access are added. on linux through an `O_PATH` handle and `/proc/self/fd`; elsewhere by name with the identity checked first.
- anything that cannot be read fails closed: the mount table, sysfs for devices, the filesystem type for `--wipe-free`.
- a protected-path comparison on unix is by `(dev, ino)`, not by spelling. `is_system_protected` reads no environment variable on unix.
- `--log` is created with `create_new`, flushed per line, and a failed write stops the erase.
- the requested pass sequence ends with random data; `--zero` adds one final zero pass. `--verify` reproduces the final bytes from a seed held in memory or checks zeros.
- no `unwrap()` or `expect()` outside tests; every `unsafe` block carries a `// safety:` comment.

## where each concern lives

use the module map above. exit codes are defined in `src/main.rs` and mapped in `src/run.rs` and `src/device/linux/mod.rs`. update the matching topic page when behaviour changes.

## tests

integration tests in `tests/*.rs` run the real binary; helpers are in `tests/common/mod.rs`. unit tests sit next to the code. see [development](development.md#tests) for test requirements and scripts.
