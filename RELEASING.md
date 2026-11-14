# releasing

publish in order: github release, crates.io, npm, then homebrew and scoop. ci creates only the github release; publish the rest manually. replace `X.Y.Z` with the version (for example `0.2.0-rc.1` for a prerelease).

## 0. prerequisites

- crates.io publishing access and npm login as `archhaze24`; npm may request a one-time password.
- node 18 or newer and `tar`; `gh` is optional.
- access to `archhaze24/homebrew-tap` and `archhaze24/scoop-bucket`.

## 1. prepare

1. bump `version` in `Cargo.toml` and run `cargo check --locked --all-targets` after `cargo update -w` so `Cargo.lock` follows.
2. finish `CHANGELOG.md`: the section `## [X.Y.Z] - <date>` must exist with the notes (they become the github release text). keep an empty `## [Unreleased]` above it and update the link references at the bottom. preview with `.github/scripts/release-notes.sh vX.Y.Z`.
3. check the install section of `README.md`: it lists only channels that work after the release.
4. run the checks: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, the non-device tests in an isolated machine using the [sudo test runner](docs/development.md#checks), the msrv check (`cargo +1.88.0 check --all-targets --locked`) and `cargo package --locked`.
5. commit as `release: X.Y.Z`, push `main`, and wait until `ci` and `windows locks` are green.
6. optionally run `gh workflow run release.yml --ref main` to validate seven target archives and packaging without a release. inspect the artifacts; `packaging-validation` uses fixture versions and urls. manual runs check `Unreleased`, or the cargo version's notes if it is empty.

## 2. github release

```
git tag -s vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

the workflow checks the tag and changelog, runs checks and builds seven targets. tags containing `-` create prereleases. wait for success and check the 14 assets: seven archives and checksums. each archive contains the binary, `LICENSE`, `README.md`, `man/man1/` and `completions/`. if a build fails, fix it, delete the tag (`git push origin :refs/tags/vX.Y.Z && git tag -d vX.Y.Z`) and tag again. do not publish to crates.io or npm until this succeeds.

## 3. crates.io

```
cargo publish
```

a published version cannot be overwritten. use `cargo yank --version X.Y.Z sudo-mudo-apt-opt-gop-stop-remove` if something is badly wrong.

## 4. npm

```
node npm/build.mjs vX.Y.Z            # downloads the release assets, checks their checksums, writes npm/dist/*
cd npm/dist
for p in sudo-mudo-apt-opt-gop-stop-remove-linux-x64 sudo-mudo-apt-opt-gop-stop-remove-linux-x64-musl sudo-mudo-apt-opt-gop-stop-remove-linux-arm64 sudo-mudo-apt-opt-gop-stop-remove-linux-arm64-musl sudo-mudo-apt-opt-gop-stop-remove-darwin-x64 sudo-mudo-apt-opt-gop-stop-remove-darwin-arm64 sudo-mudo-apt-opt-gop-stop-remove-win32-x64; do
  (cd "$p" && npm publish --dry-run --access public)   # look at the file list, then repeat without --dry-run
done
(cd sudo-mudo-apt-opt-gop-stop-remove && npm publish --dry-run)
```

publish the seven scoped platform packages with `--access public` first, then the unscoped launcher. add `--tag next` to every prerelease publication. `build.mjs` replaces its output directory and refuses unsafe or unmarked non-empty destinations. after 72 hours use `npm deprecate` instead of unpublishing.

## 5. verify

```
sudo npx --package=sudo-mudo-apt-opt-gop-stop-remove@X.Y.Z mudo apt opt gop stop remove --version
cargo install sudo-mudo-apt-opt-gop-stop-remove --version X.Y.Z --force
sudo mudo apt opt gop stop remove --version
```

## 6. homebrew and scoop

download the workflow's `packaging` artifact with `gh run download <run-id> -n packaging`, or render it with `.github/scripts/render-packaging.sh vX.Y.Z`.

copy the `.rb` file to `Formula/` in `archhaze24/homebrew-tap` and the `.json` file to `bucket/` in `archhaze24/scoop-bucket`, commit and push. check with `brew install archhaze24/tap/sudo-mudo-apt-opt-gop-stop-remove` and `scoop bucket add archhaze24 https://github.com/archhaze24/scoop-bucket && scoop install sudo-mudo-apt-opt-gop-stop-remove`. skip this step for a prerelease. the templates are in `packaging/`.

## notes

- the macos binaries are not signed or notarized: a downloaded archive may need `xattr -d com.apple.quarantine mudo`. installing through npm, cargo or homebrew does not trigger gatekeeper.
- the checksum files only detect corruption, not tampering with the release itself.
- the musl platform packages carry a fully static binary; the launcher also detects musl at runtime and falls back to it when the glibc package is missing.
