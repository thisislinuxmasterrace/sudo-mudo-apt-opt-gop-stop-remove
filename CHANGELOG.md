# changelog

all notable changes are listed here. the format follows [keep a changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-11-15

### added

- `--zero` adds a final zero pass, including read-back verification when requested; normal runs still end with random data.
- `schema_version: 1` in dry-run json and every erase-log record, with compatibility rules in the reference.
- linux fscrypt metadata detection, preserving dm-crypt facts when both layers encrypt the target.
- positive macos and windows compression flags in assessments.
- device `--explain` advice distinguishes overwrite, firmware erase and luks keyslot removal without running commands.
- `--explain` distinguishes git index and reachable-history evidence and checks direct btrfs or containing-dataset zfs snapshots with bounded read-only tools.
- windows named file data streams are overwritten and optionally verified before deletion; locked streams and directories with streams are refused.

### changed

- human output groups targets, summary, risks and results, with terminal colors and `NO_COLOR` support. targets are listed in plain text, summaries name only what is there (`1 file, 7 bytes, 1 pass`), and a forced erase prints its note and the erased target without headings.
- erase runs print one short general risk notice instead of repeating every entry's assessment; full facts remain in json, logs and `--explain`. `-q` also hides that notice; warnings about the given paths remain.
- `--explain` names found editor backups, repositories and snapshots by path, lists findings before the general limits, and keeps whole-disk erase commands in the documentation.
- `--help` groups flags by purpose and uses the full lowercase usage line; a wrong or missing word of the command phrase gets one plain message.
- summaries no longer mark coverage incomplete only because of `--exclude`; os errors are printed without their numbers.
- internal hard links within one selected recursive tree are supported; excluded or external names remain protected.

### fixed

- btrfs snapshot advice works for file targets by listing their containing directory.
- `--explain` no longer says that `.git` is untouched when a recursive erase removes the repository with its history.
- human summaries no longer present conservative weak verdict counts as separate detected problems.
- deeply nested trees stop at 64 relative levels instead of exhausting the recursive stack; deeper entries are kept.
- device interruption messages also read correctly when only one target remains.

## [0.1.0] - 2026-11-15

### changed

- the only executable is now `mudo`; package and archive names stay unchanged. every invocation, including help and version, requires `sudo mudo apt opt gop stop remove`. unix requires sudo user markers and root; windows requires native system sudo ancestry and elevation. an administrator shell alone is refused.
- windows msvc builds link the c runtime statically.

### fixed

- windows fat32 free-space wipes roll over before the per-file limit instead of reporting success with free space left.
- ctrl+c ends blocked windows holder and reboot prompts without waiting for an answer.
- the invoking unix account's home remains protected when sudo changes the home environment.

### added

- erase files, trees and links by overwriting and deleting them, with a dry run, a json summary, a json lines log, `--verify` and `--exclude`.
- `--wipe-free` fills the free space of a volume with random data.
- erasing a whole block device on linux (experimental, not tested on real disks).
- a conservative assessment for every object: a verdict (`weak` or `cosmetic`), reasons and facts such as filesystem, medium, encryption and other copies.
- a report of other copies found next to a target: a git work tree, a sync folder, a trash folder and editor backups.
- `--explain`: a dry run that also prints advice for the paths, with commands that would help. the commands are only printed.
- encryption facts on linux: dm-crypt and ecryptfs.
- compression facts on linux, read from the mount options: `enabled`, `not_supported` or `unknown`.
- sighup stops an erase like ctrl+c and sigterm.
- release archives with a man page and shell completions, and packaging for npm, homebrew and scoop.

[Unreleased]: https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/releases/tag/v0.1.0
