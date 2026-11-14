# changelog

all notable changes are listed here. the format follows [keep a changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [semantic versioning](https://semver.org/).

## [Unreleased]

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
