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

windows hosted ci runs unit and direct-invocation refusal tests. positive cli integration needs native sudo, which hosted runners lack: run it in a [windows vm](#windows-vm). no tests erase a real block device: ci tests device functions and refusals only.

### fast linux vm checks

keep the default `target/` warm between checks in the same checkout. cargo enables incremental compilation in development and test builds by default. these builds keep file and line numbers for backtraces in project code and omit dependency debuginfo.

with `cargo-nextest` installed, run the non-device suite on an isolated linux vm where `/var/tmp` is on ext4:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
cargo nextest run --all-targets --locked --profile vm \
  --config "target.$host.runner=['sudo','-n','TMPDIR=/var/tmp','CI=1']"
```

the runner sets the temp directory after sudo and makes mount setup failures fail the tests. the vm profile skips test names containing `device` and starts the large directory fixtures first.

## windows vm

`tests/windows/vm/run.sh` runs the windows checks in a local windows 11 vm under native sudo: the unit and invocation tests that windows ci runs, the locked-file scenarios (`tests/windows/locks.ps1`) and the fat32 volume checks (`tests/windows/fat32.ps1`). it cross-builds with the `x86_64-pc-windows-gnu` target (needs mingw-w64 and `jq`), copies the binaries into the vm's shared folder and prints the vm's output; the exit code is 0 when every check passed. results are kept in `<shared>/results/`.

the vm is a [dockurr/windows](https://github.com/dockur/windows) container on a linux host with `/dev/kvm`. set it up once:

```sh
mkdir -p ~/win/storage ~/win/shared
docker run -d --name mudo-windows --device /dev/kvm --device /dev/net/tun --cap-add NET_ADMIN \
  -p 127.0.0.1:8006:8006 -e VERSION=11 -e RAM_SIZE=4G -e CPU_CORES=4 \
  -v ~/win/storage:/storage -v ~/win/shared:/shared --stop-timeout 120 dockurr/windows
```

wait for the install to finish (the screen is at `http://127.0.0.1:8006`), copy `tests/windows/vm/runner.ps1` and `install-runner.ps1` to `~/win/shared/runner/`, and run once in the vm, from an administrator powershell: `powershell -ExecutionPolicy Bypass -File \\host.lan\Data\runner\install-runner.ps1`. it installs pwsh 7, turns on native sudo in inline mode and registers a logon task that runs the runner with the highest privileges. the runner watches `\\host.lan\Data\jobs` and runs one job at a time in the logged-on console. `MUDO_WIN_CONTAINER`, `MUDO_WIN_SHARED`, `MUDO_WIN_TARGET_DIR` and `MUDO_WIN_TIMEOUT` change the defaults of `run.sh`.

the checks run the gnu build; release archives use msvc. the fat32 checks create, fill and delete two virtual disks inside the vm.

## scripts

run by hand or by a ci job, outside `cargo test`:

| script | what it checks | how it runs |
| --- | --- | --- |
| `npm/tests.mjs` | offline release archive, package and launcher checks (needs node, python 3, tar and unzip) | `node --test npm/tests.mjs` |
| `tests/windows/locks.ps1` | locked-file scenarios on windows, including unicode paths, json logging, a holder that shares delete and read-only files (needs pwsh 7+ and native sudo; touches only a fresh temp directory) | `tests/windows/vm/run.sh`, the `windows locks` workflow when the runner has native sudo, or `pwsh -File tests/windows/locks.ps1 -Binary <path>` |
| `tests/windows/fat32.ps1` | free-space wipe on disposable 128 mib and 5 gib fat32 virtual disks (administrator, vm only) | `tests/windows/vm/run.sh` |
| `tests/windows/vm/run.sh` | every windows check above plus the windows unit tests, in a local windows vm | see [windows vm](#windows-vm) |
| `tests/network/network_fs.sh <binary> <mount dir> <nfs\|cifs>` | a file on a network mount gets the warning and is still erased; `--wipe-free` there is refused | the manual `network filesystems` workflow (linux, root) |
| `tests/manual/macos_bind_mount.sh [binary]` | a `-r` walk on macos does not enter a bind mount (`mount_nullfs`) or touch its source | by hand, macos only, needs `sudo` |

## ci

| workflow | trigger | content |
| --- | --- | --- |
| `ci.yml` | push and pull request to main | fmt (ubuntu only), clippy and tests on ubuntu, macos and windows; a `msrv` job on 1.88 (host and `x86_64-pc-windows-gnu` check); a `cargo audit` job on the committed `Cargo.lock` |
| `windows-locks.yml` | push, pull request, manual | builds release and runs `tests/windows/locks.ps1` on windows-latest |
| `network-fs.yml` | manual | mounts nfs and samba on localhost and runs `tests/network/network_fs.sh` |
| `release.yml` | `v*` tags, manual | checks that the tagged commit is on main and that the tag matches `Cargo.toml` and `CHANGELOG.md`, runs clippy and tests, generates the man page and completions, and builds archives for seven targets with a `.sha256` each. a tag push also creates a github release; a manual run builds but does not release. see [releasing](../RELEASING.md) |

every action is pinned to a full commit sha with a version comment, and dependabot updates both. dependabot ignores rand 0.x major and minor updates; the toolchain versions (`stable`, the 1.88 msrv) are `toolchain:` inputs changed by hand. check node versions before bumping actions.

## conventions

tracked prose and messages are public, english and lowercase; identifiers and external names keep their case. comments explain intent or constraints; each `unsafe` block needs a `// safety:` invariant. no `unwrap()` or `expect()` outside tests, `todo!`, `dbg!` or `TODO`. use rustfmt defaults and clippy `-D warnings`.

update the relevant docs when flags, refusals or `-f` behaviour change. errors should name the path and reason; clean temporary files on error and kill. keep tests grouped by topic and split large files by responsibility.
