# sudo-mudo-apt-opt-gop-stop-remove

when you really want to nuke the shit out of some files

`mudo` hides file names, overwrites the contents, then deletes the files. it handles directory trees and free space too. linux block-device erasure is experimental.

**overwriting is best effort.** ssds, snapshots, copy-on-write filesystems and backups can keep old data. `--verify` checks the new contents, not whether every old copy is gone. [limits](docs/guarantees-and-limits.md).

## build

no release is published yet. build with rust 1.88 or newer:

```sh
cargo build --release
sudo ./target/release/mudo apt opt gop stop remove --help
```

the package name stays long; the executable is only `mudo`.

## use

once `mudo` is on sudo's path:

```sh
# inspect first
sudo mudo apt opt gop stop remove --dry-run -r build/

# erase a file, after confirmation
sudo mudo apt opt gop stop remove notes.txt

# erase a tree, keeping .git
sudo mudo apt opt gop stop remove -r --exclude .git old-project/
```

yes, `sudo` and all five words are required, even for `--help` and `--version`.

on windows, use native system `sudo.exe` from an elevated console, with windows 11 24h2 or later, uac and inline sudo enabled. an admin console alone is insufficient. [setup](docs/usage.md#invocation).

`-f` skips questions, not protections. symlinks are removed without following their targets. protected paths and unix hard links still have safeguards. [safety](docs/safety.md).

## more

- [usage](docs/usage.md): confirmation, scripts, interrupts and windows locks
- [reference](docs/reference.md): flags, exit codes and json formats
- [free-space wiping](docs/wipe-free.md) and [linux devices](docs/devices.md)
- [all documentation](docs/README.md), including build and contribution notes

[mit license](LICENSE).
