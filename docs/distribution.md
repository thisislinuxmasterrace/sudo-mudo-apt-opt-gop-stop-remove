# distribution

the ways to get `sudo-mudo-apt-opt-gop-stop-remove` once a release is published. no release exists yet, so none of the channels below has a published version. how a release is made is in [releasing](../RELEASING.md).

contents: [cargo](#cargo) · [release archives](#release-archives) · [npm](#npm) · [homebrew](#homebrew) · [scoop](#scoop) · [from source](#from-source)

## cargo

```sh
cargo install sudo-mudo-apt-opt-gop-stop-remove
```

needs rust 1.88 or newer.

## release archives

the github releases of [the repository](https://github.com/thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove/releases) carry one archive per target, `sudo-mudo-apt-opt-gop-stop-remove-<tag>-<target>.tar.gz` (`.zip` on windows), and a `.sha256` file for each. targets: `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. the musl builds are fully static. the glibc builds are made on the current ubuntu runners and need a glibc as new as theirs (2.39 at the time of writing); on an older distribution use the musl archive.

each archive holds the binary, the license, the readme, the man page in `man/man1/` and shell completions in `completions/` (bash, zsh, fish and powershell). read the man page in place with `man -l man/man1/sudo-mudo-apt-opt-gop-stop-remove.1`.

the `.sha256` files only detect a corrupted download, not a replaced release. the macos binaries are not signed or notarized, so a downloaded archive may need `xattr -d com.apple.quarantine sudo-mudo-apt-opt-gop-stop-remove`.

## npm

```sh
npx sudo-mudo-apt-opt-gop-stop-remove --dry-run notes.txt
npm install -g sudo-mudo-apt-opt-gop-stop-remove
```

the launcher package picks a platform package, `@archhaze24/sudo-mudo-apt-opt-gop-stop-remove-<os>-<arch>[-musl]`: linux x64 and arm64 (glibc and static musl), macos x64 and arm64, and windows x64. on musl systems the launcher prefers the musl package. the launcher passes the terminal through, so the confirmation questions work, and forwards termination signals to the tool.

## homebrew

```sh
brew install archhaze24/tap/sudo-mudo-apt-opt-gop-stop-remove
```

installs the binary, the man page and the completions on macos (arm64 and x64) and linux (arm64 and x64, from the static musl archives). the formula is added by hand to the personal tap repo `archhaze24/homebrew-tap` as `Formula/sudo-mudo-apt-opt-gop-stop-remove.rb` after a release. the template is `packaging/homebrew/sudo-mudo-apt-opt-gop-stop-remove.rb.in`.

## scoop

```powershell
scoop bucket add archhaze24 https://github.com/archhaze24/scoop-bucket
scoop install sudo-mudo-apt-opt-gop-stop-remove
```

installs the `.exe` for windows x64. the manifest is added by hand to the personal bucket repo `archhaze24/scoop-bucket` as `bucket/sudo-mudo-apt-opt-gop-stop-remove.json` after a release. the template is `packaging/scoop/sudo-mudo-apt-opt-gop-stop-remove.json.in`; `checkver` and `autoupdate` keep the bucket current with new releases.

both files are rendered by `.github/scripts/render-packaging.sh TAG [ASSET_DIR|- [OUT_DIR]]` from the `.sha256` files of the release, read from `ASSET_DIR` or downloaded from the public release url when `ASSET_DIR` is omitted or `-`. the release workflow runs it and attaches the result as the workflow artifact `packaging`.

## from source

```sh
cargo build --release   # target/release/sudo-mudo-apt-opt-gop-stop-remove
```

the man page and the completions come from a dev-only example, see [reference](reference.md#man-page-and-completions).
