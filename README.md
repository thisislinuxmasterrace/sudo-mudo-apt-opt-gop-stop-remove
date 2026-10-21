# sudo-mudo-apt-opt-gop-stop-remove

when you really want to delete something

## what is this?

a command-line tool that irreversibly erases files: it overwrites their
contents several times, then deletes them. works on windows, macos and linux.

> **status:** early development.

## usage

```sh
sudo-mudo-apt-opt-gop-stop-remove venom.exe           # erase a file, asks for confirmation
sudo-mudo-apt-opt-gop-stop-remove -n 1 disk-dump.img   # use a single overwrite pass
sudo-mudo-apt-opt-gop-stop-remove -f notes.txt         # skip the confirmation prompt
sudo-mudo-apt-opt-gop-stop-remove -r build/            # erase a directory tree
sudo-mudo-apt-opt-gop-stop-remove --dry-run notes.txt  # only show what would be erased
```

symbolic links are removed themselves and never followed, so their targets are
left untouched. read-only files and directories are made writable before erasing.

## limitations

overwriting does not guarantee that data is unrecoverable on every device or
filesystem. ssd wear leveling, copy-on-write filesystems (apfs, btrfs, zfs),
journaling, snapshots, backups and caches can keep old copies of the data.
for ssds, full-disk encryption (filevault, bitlocker, luks) is the more
reliable approach.

hard links share one copy of the data, so erasing one name erases the content
for all of them.

erased files cannot be recovered with this tool. use it with care.

## development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```
