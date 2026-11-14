# development

how to build, check and test the tool, and the conventions for changing it. the code map is in [architecture](architecture.md).

contents: [toolchain](#toolchain) · [checks](#checks) · [tests](#tests) · [scripts](#scripts) · [ci](#ci) · [conventions](#conventions)

## toolchain

rust 1.88 or newer (`rust-version = "1.88"`, edition 2024). the code uses let-chains, so the minimum must not be lowered. re-check the minimum with `cargo check --all-targets --locked` on a 1.88 toolchain when dependencies or the edition change.

```sh
cargo build --release   # target/release/sudo-mudo-apt-opt-gop-stop-remove
```

## checks

the same commands run in ci:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

the windows code only compiles on windows or with the windows target. to check that it still builds from another os (needs `rustup target add x86_64-pc-windows-gnu`; it compiles, it does not run):

```sh
cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings
```

## tests

- integration tests in `tests/*.rs` run the real binary on files they create in temporary directories, grouped by topic. helpers: `tests/common/mod.rs`. unit tests sit next to the code.
- the tool erases data for real. run `cargo test` in a throwaway environment (a container or a vm) rather than on a machine with data you care about, and never point a manual run at real data.
- some tests need unix or linux, and the root branch of the hard link rule runs only as root (`sudo`). copy-on-write filesystems prove nothing about in-place overwrites: use an ext4 image, for example through `fuse2fs`, for real overwrite checks.
- there are no tests that erase a real block device. ci runs unit tests of the device functions and the refusal tests only.
- add tests where a unit can be tested without writing real data. disk-level scenarios go to a throwaway vm.

## scripts

run by hand or by a ci job, outside `cargo test`:

| script | what it checks | how it runs |
| --- | --- | --- |
| `npm/tests.mjs` | offline release archive, package and launcher checks (needs node, python 3, tar and unzip) | `node --test npm/tests.mjs` |
| `tests/windows/locks.ps1` | locked-file scenarios on windows (needs pwsh 7+; touches only a fresh temp directory) | the `windows locks` workflow, or `pwsh -File tests/windows/locks.ps1 -Binary <path>` |
| `tests/network/network_fs.sh <binary> <mount dir> <nfs\|cifs>` | a file on a network mount gets the warning and is still erased; `--wipe-free` there is refused | the manual `network filesystems` workflow (linux, root) |
| `tests/manual/macos_bind_mount.sh [binary]` | a `-r` walk on macos does not enter a bind mount (`mount_nullfs`) or touch its source | by hand, macos only, needs `sudo` |

## ci

| workflow | trigger | content |
| --- | --- | --- |
| `ci.yml` | push and pull request to main | fmt (ubuntu only), clippy and tests on ubuntu, macos and windows; a `msrv` job on 1.88 |
| `windows-locks.yml` | push, pull request, manual | builds release and runs `tests/windows/locks.ps1` on windows-latest |
| `network-fs.yml` | manual | mounts nfs and samba on localhost and runs `tests/network/network_fs.sh` |
| `release.yml` | `v*` tags, manual | checks the tag against `Cargo.toml` and `CHANGELOG.md`, runs clippy and tests, generates the man page and completions, and builds archives for seven targets with a `.sha256` each. a tag push also creates a github release; a manual run builds but does not release. see [releasing](../RELEASING.md) |

dependabot opens weekly update prs for cargo and github actions. rand 0.x major and minor updates are ignored (each minor of a 0.x crate is breaking; the code targets 0.10), and `dtolnay/rust-toolchain` is ignored because the msrv pin is changed by hand. actions are pinned to versions; check node versions before bumping.

## conventions

- everything tracked is public and english only: code, comments, program messages, docs, tests, workflows and commit messages. prose, comments and messages are lowercase; identifiers, flags and external names keep their case (`SystemRoot`, `C:\`).
- comments explain intent, constraints or trade-offs, never what the code says. each `unsafe` block gets a `// safety:` comment that states the invariant.
- no `unwrap()` or `expect()` outside tests, no `todo!`, `dbg!` or `TODO` comments.
- rustfmt defaults; clippy clean with `-D warnings`.
- reuse existing helpers (`prompt::ask`, `storage::warnings`, `preview::human_bytes`).
- a file over about 500 lines is split by responsibility. a split is a pure move: bodies are compared before and after, and tests stay grouped by topic.
- a change that alters what `-f` skips, a refusal, or any flag needs the matching page in `docs/` updated in the same change.
- every new error message names the path and the reason.
- temporary files and partial states are cleaned up on error and on kill.
- the binary name is long on purpose and gets no short alias.

review checklist for changes: do the protections still hold (symlinks, protected paths, mount points, dry run)? does a device path keep its refusals and its typed-path confirmation under `-f`? does a permission change still work for mode-000 entries? does the windows target still compile?
