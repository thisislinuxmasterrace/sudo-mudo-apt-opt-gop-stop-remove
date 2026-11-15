# guarantees and limits

`sudo mudo apt opt gop stop remove` writes through the filesystem. it cannot reach every place that may hold old data, and it never certifies that a file is unrecoverable.

## by kind of storage

| storage | what can survive |
| --- | --- |
| hard disk with an in-place filesystem | journals, caches, remapped blocks and other copies |
| ssd, nvme, usb flash, sd card | old physical blocks hidden by wear leveling and remapping |
| copy-on-write: apfs, btrfs, zfs, refs, bcachefs | old blocks and shared clusters |
| snapshots and backups | earlier versions outside the file being overwritten |
| network or remote storage | server-side copies and snapshots |

the tool warns about copy-on-write, network, fuse and unknown filesystems. flash warnings require linux and a known non-rotational device; no warning does not establish safe media. a file erase continues; [free-space wiping](wipe-free.md) refuses network, fuse and unknown filesystems.

each pass is synced before the next one. syncing does not make the drive overwrite its old physical blocks. on linux, sparse-file holes are skipped; unsupported range queries fall back to a whole-file overwrite. the byte estimate uses logical file sizes.

extra passes do not solve these limits.

linux can report fscrypt from metadata without reading contents, alongside an underlying dm-crypt layer. these facts do not prove that keys or backups are gone. macos `UF_COMPRESSED` and windows `FILE_ATTRIBUTE_COMPRESSED` are positive compression clues; an unset flag does not prove that an overwrite reaches old blocks. on windows directories the flag describes the default for newly created files.

## assessments

file runs report an operation result and a separate assessment:

- `weak`: best effort, including unknown storage conditions.
- `cosmetic`: a link was removed; its target was not overwritten.
- `effective`: reserved. this version never issues it automatically.

`result: ok` means the operation succeeded, not that every old copy disappeared. directories are assessed as metadata; visited children get their own records. skipped, excluded, refused or unreadable entries produce `assessment_coverage: incomplete` in dry-run json. `visited_entries` covers only inspected entries.

facts are advisory, collected before rename or deletion. they never authorize an erase or replace identity checks:

- linux detects rotational media through sysfs; other platforms leave media unknown.
- filesystem names are queried where supported. failures leave facts unknown.
- linux compression facts come from mount options, not individual files. missing options do not prove compression is off.
- linux detects dm-crypt, ecryptfs and supported fscrypt metadata. `none_detected` does not prove plaintext; self-encrypting drives and some underlying storage are not detected. other platforms are unchecked.
- snapshots remain unknown.
- nearby git work trees, known sync/trash folders and editor backups are listed as possible copies, never erased. `none_found` is not a backup search.

see [reference](reference.md#dry-run-json) for fields. device and free-space runs use their own reports.

## what `--verify` proves

`--verify` reads the file after its final synced pass and compares it with the new bytes. a match proves only that the filesystem returned those bytes at that moment. it does not inspect earlier passes, remapped blocks, snapshots or backups. a log is not an erasure certificate either.

the final pattern comes from a per-file random seed held in memory; the generator is not cryptographic. sparse-file verification reads the whole logical length, including holes. linux requests a cache drop with `posix_fadvise(DONTNEED)`; macos does not drop the cache.

with `--zero`, one additional pass ends with zeros instead; `--verify` then checks zeros. this chooses the final visible bytes, not stronger erasure. compression may store zeros without replacing earlier physical blocks.

a mismatch keeps the file under its name and returns exit 4; other paths continue. verification adds the owner's read permission, which a kept file retains. it is refused for devices (exit 1), conflicts with `--wipe-free` (exit 2), and is ignored in a dry run.

## other limits

- **ntfs streams:** named file data streams are opened and checked before writing, overwritten and optionally verified before file deletion. a locked or changed stream keeps the file with an error. directories with named streams are refused. concurrent stream changes are not atomic with deletion. native validation of this version is required; older versions overwrote only the default stream.
- **compression:** rewritten data may occupy new clusters, leaving old ones behind.
- **interrupts:** a file may be partly overwritten. a second signal or abrupt stop can leave its hidden name. see [interrupts](usage.md#interrupts).
- **hard links:** names share data. unix refuses external or unproven links under root unless explicitly overridden; proven internal links are removed before overwriting the last name. see [safety](safety.md#hard-links).
- **read-only parent:** contents may be erased even when their directory cannot be removed.
- **logs:** record paths, not contents. unix logs use mode 0600 and remain after erasure.

## before selling or giving away a device

deleting files or filling free space is not a whole-device sanitization method. consider encryption enabled before sensitive data was written, or the drive's own erase procedure. cryptographic erasure requires removing every usable key copy.

[linux device mode](devices.md) overwrites the named disk or partition, not space outside it. it is experimental, refuses ssds by default and does not use drive-level erase commands.

## what is not verified

windows checks cover hosted ci and an isolated windows 11 vm: locks, verification/logging, reparse points, cancellation and disposable fat32 volumes. a reboot-deletion sentinel disappeared after reboot; the complete interactive reboot fallback remains unverified. these are not hardware tests.

still unverified: real block-device erasure; antivirus/cloud-provider behavior; refs and compressed ntfs; windows log identity and volume mount points; macos bind mounts and mount-query failures on hardware; bsd; real-terminal progress and verification throughput.

recursive walks have finite stack and descriptor limits. descriptor errors are reported, but stack exhaustion can abort before cleanup. unlimited nesting is not supported.
