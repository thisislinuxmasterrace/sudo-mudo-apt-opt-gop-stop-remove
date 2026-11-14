# documentation

index of all pages. the project landing page is the [readme](../README.md).

contents: [start here](#start-here) · [pages](#pages)

## start here

| you are | read in this order |
| --- | --- |
| new to the tool | [readme](../README.md) quick start, then [usage](usage.md) |
| deciding whether it is enough for your data | [guarantees and limits](guarantees-and-limits.md), then [devices](devices.md) or [wipe-free](wipe-free.md) |
| wondering why a path was refused | [safety](safety.md), then [exit codes](reference.md#exit-codes) |
| scripting it (cron, ci, pipelines) | [usage](usage.md#in-scripts), [exit codes](reference.md#exit-codes), [log format](reference.md#log-format), [dry-run json](reference.md#dry-run-json) |
| erasing a disk before selling it | [guarantees and limits](guarantees-and-limits.md#before-selling-or-giving-away-a-device), [devices](devices.md) |
| a contributor | [architecture](architecture.md), [development](development.md), then the topic page you change |

## pages

| page | content |
| --- | --- |
| [usage.md](usage.md) | erasing files and trees, what the tool prints, confirmations, recipes, interrupts, locked files on windows |
| [safety.md](safety.md) | refusals and their reasons: symlinks, protected paths, mount points, hard links, root, special files |
| [guarantees-and-limits.md](guarantees-and-limits.md) | what an overwrite does on hdd, ssd, copy-on-write and network storage, what `--verify` proves, what is not verified |
| [wipe-free.md](wipe-free.md) | `--wipe-free`: how it works, what it refuses, its limits |
| [devices.md](devices.md) | erasing a whole block device on linux: targets, refusals, confirmation |
| [reference.md](reference.md) | flags, exit codes, `--log` and `--dry-run --json` formats, `--files-from`, `--exclude`, man page and completions |
| [architecture.md](architecture.md) | code map, data flow, invariants, where each concern lives |
| [distribution.md](distribution.md) | the install channels once a release exists: cargo, release archives, npm, homebrew, scoop |
| [development.md](development.md) | toolchain, checks, tests, manual scripts, ci, msrv, conventions |
