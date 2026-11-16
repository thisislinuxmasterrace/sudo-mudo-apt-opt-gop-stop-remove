# distribution

installing `sudo mudo apt opt gop stop remove` through release archives, cargo, npm, homebrew or scoop. the release procedure is in [releasing](../RELEASING.md).

## cargo

```sh
cargo install sudo-mudo-apt-opt-gop-stop-remove
```

needs rust 1.88 or newer. installs only the executable `mudo`; invoke it as `sudo mudo apt opt gop stop remove ...`. sudo does not search `~/.cargo/bin` or a user npm prefix; then run `sudo "$(command -v mudo)" apt opt gop stop remove ...`.

## release archives

each [github release](https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/releases) has archives and `.sha256` files for linux x86_64 and aarch64 (glibc and static musl), macos x86_64 and arm64, and windows x86_64 msvc. glibc builds require at least the runner's glibc (2.39 at the time of writing); use musl on older distributions.

each archive holds the binary, the license, the readme, the man page in `man/man1/` and shell completions in `completions/` (bash, zsh, fish and powershell). read the man page in place with `man -l man/man1/mudo.1`.

the `.sha256` files only detect a corrupted download, not a replaced release. the macos binaries are not signed or notarized, so a downloaded archive may need `xattr -d com.apple.quarantine mudo`. windows msvc builds link the c runtime statically; native system sudo still requires windows 11 24h2 or later and the [invocation setup](usage.md#invocation).

## npm

```sh
sudo npx --package=sudo-mudo-apt-opt-gop-stop-remove mudo apt opt gop stop remove --dry-run venom.exe
npm install -g sudo-mudo-apt-opt-gop-stop-remove
sudo mudo apt opt gop stop remove --help
```

the launcher selects a scoped platform package, prefers musl on musl systems, passes through the terminal and forwards termination signals.

## homebrew

```sh
brew install archhaze24/tap/sudo-mudo-apt-opt-gop-stop-remove
```

installs the binary, man page and completions on macos and linux arm64 and x64 (linux uses static musl).

## scoop

```powershell
scoop bucket add archhaze24 https://github.com/archhaze24/scoop-bucket
scoop install sudo-mudo-apt-opt-gop-stop-remove
```

installs the windows x64 executable. see [releasing](../RELEASING.md) for rendering the homebrew and scoop package files.

## from source

```sh
cargo build --release   # target/release/mudo
```

the man page and the completions come from a dev-only example, see [reference](reference.md#man-page-and-completions).
