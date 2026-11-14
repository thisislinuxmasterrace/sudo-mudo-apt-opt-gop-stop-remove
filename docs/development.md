# development

the code map is in [architecture](architecture.md).

## toolchain

rust 1.88 or newer (`rust-version = "1.88"`, edition 2024). the code uses let-chains, so the minimum must not be lowered. re-check the minimum with `cargo check --all-targets --locked` on a 1.88 toolchain when dependencies or the edition change.

```sh
cargo build --release   # target/release/mudo
```

## checks

the same commands run in ci:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo --config "target.$(rustc -vV | sed -n 's/^host: //p').runner=['sudo','-n']" test --all-targets --locked
```

the windows code only compiles on windows or with the windows target. to check that it still builds from another os (needs `rustup target add x86_64-pc-windows-gnu`; it compiles, it does not run):

```sh
cargo clippy --all-targets --target x86_64-pc-windows-gnu -- -D warnings
```

## tests

tests erase data. run them in a throwaway container or vm, never against real data. unix integration tests use genuine sudo and the runner above. some tests require unix or linux; root is needed for the root hard-link branch. use an ext4 image, for example through `fuse2fs`, to check in-place overwrites; copy-on-write filesystems cannot prove them.

windows hosted ci runs unit and direct-invocation refusal tests. positive cli integration requires an isolated windows 11 vm with native sudo and uac; missing native sudo is a coverage gap. no tests erase a real block device: ci tests device functions and refusals only.

## scripts

run by hand or by a ci job, outside `cargo test`:

| script | what it checks | how it runs |
| --- | --- | --- |
| `npm/tests.mjs` | offline release archive, package and launcher checks (needs node, python 3, tar and unzip) | `node --test npm/tests.mjs` |
| `tests/windows/locks.ps1` | locked-file scenarios on windows, including unicode paths and json logging (needs pwsh 7+; touches only a fresh temp directory) | the `windows locks` workflow, or `pwsh -File tests/windows/locks.ps1 -Binary <path>` |
| `tests/network/network_fs.sh <binary> <mount dir> <nfs\|cifs>` | a file on a network mount gets the warning and is still erased; `--wipe-free` there is refused | the manual `network filesystems` workflow (linux, root) |
| `tests/manual/macos_bind_mount.sh [binary]` | a `-r` walk on macos does not enter a bind mount (`mount_nullfs`) or touch its source | by hand, macos only, needs `sudo` |

## ci

| workflow | trigger | content |
| --- | --- | --- |
| `ci.yml` | push and pull request to main | fmt (ubuntu only), clippy and tests on ubuntu, macos and windows; a `msrv` job on 1.88 |
| `windows-locks.yml` | push, pull request, manual | builds release and runs `tests/windows/locks.ps1` on windows-latest |
| `network-fs.yml` | manual | mounts nfs and samba on localhost and runs `tests/network/network_fs.sh` |
| `release.yml` | `v*` tags, manual | checks the tag against `Cargo.toml` and `CHANGELOG.md`, runs clippy and tests, generates the man page and completions, and builds archives for seven targets with a `.sha256` each. a tag push also creates a github release; a manual run builds but does not release. see [releasing](../RELEASING.md) |

dependabot ignores rand 0.x major and minor updates and `dtolnay/rust-toolchain`; the msrv pin is changed by hand. check node versions before bumping actions.

## conventions

tracked prose and messages are public, english and lowercase; identifiers and external names keep their case. comments explain intent or constraints; each `unsafe` block needs a `// safety:` invariant. no `unwrap()` or `expect()` outside tests, `todo!`, `dbg!` or `TODO`. use rustfmt defaults and clippy `-D warnings`.

update the relevant docs when flags, refusals or `-f` behaviour change. errors should name the path and reason; clean temporary files on error and kill. keep tests grouped by topic and split large files by responsibility.
