# releasing

a release has four parts that go out in this order: the **github release** (archives, built by ci from a tag), **crates.io** (`cargo publish`), **npm** (wrapper packages assembled from the github release assets, so `npx sudo-mudo-apt-opt-gop-stop-remove` works) and the **homebrew and scoop** files. ci only builds and creates the github release. everything else is done by hand from a machine that is logged in; ci holds no publishing secrets.

replace `X.Y.Z` with the new version everywhere. for a prerelease use a semver prerelease version (`0.2.0-rc.1`, tag `v0.2.0-rc.1`).

## 0. prerequisites

- `cargo login` with a crates.io token that may publish new versions of this crate.
- `npm login` as `archhaze24`. every `npm publish` may ask for a one-time password. the launcher package `sudo-mudo-apt-opt-gop-stop-remove` is unscoped; the seven platform packages live under the personal scope `@archhaze24/` because npm's spam filter rejects new unscoped platform names, and they need `--access public`.
- node 18 or newer, `tar`, and `gh` (optional).
- the tap repo `archhaze24/homebrew-tap` and the bucket repo `archhaze24/scoop-bucket` exist; they are shared with other tools of the same owner.

## 1. prepare

1. bump `version` in `Cargo.toml` and run `cargo check --locked --all-targets` after `cargo update -w` so `Cargo.lock` follows.
2. finish `CHANGELOG.md`: the section `## [X.Y.Z] - <date>` must exist with the notes (they become the github release text). keep an empty `## [Unreleased]` above it and update the link references at the bottom. preview with `.github/scripts/release-notes.sh vX.Y.Z`.
3. check the install section of `README.md`: it lists only channels that work after the release.
4. run the checks: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked -- --skip device` in an isolated machine, the msrv check (`cargo +1.88.0 check --all-targets --locked`) and `cargo package --locked`.
5. commit as `release: X.Y.Z`, push `main`, and wait until `ci` and `windows locks` are green.
6. optional safety net: run the release workflow by hand on `main` (`gh workflow run release.yml --ref main`). it builds all seven targets, checks their archive checksums and renders the packaging templates without creating a release. a manual run checks the `Unreleased` notes, falling back to the cargo version's finalized notes when that section is empty. inspect the target artifacts and the `packaging-validation` artifact: its `v0.0.0-validation` version and urls are fixtures, not published release links. each target artifact contains an archive and its checksum; the archive contains the binary, `LICENSE`, `README.md`, `man/man1/` and `completions/`.

## 2. github release

```
git tag -s vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

the `release` workflow first checks that the tag, without the leading `v`, equals the version in `Cargo.toml` and that `CHANGELOG.md` has notes for it. then it runs clippy and the tests, generates the man page and the completions, builds seven targets (linux x86_64 and aarch64, each glibc and static musl; macos x86_64 and arm64; windows x86_64), and creates the release `vX.Y.Z`. a tag containing `-` is marked as a prerelease.

wait until it is green and check that the release has 14 files: seven archives and seven `.sha256` files. each archive holds the binary, `LICENSE`, `README.md`, `man/man1/` and `completions/`. if a build fails, fix it, delete the tag (`git push origin :refs/tags/vX.Y.Z && git tag -d vX.Y.Z`) and tag again. nothing is published to crates.io or npm before this step succeeds.

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

publish the platform packages first and the launcher last, otherwise installs briefly fail. add `--tag next` to every `npm publish` of a prerelease, otherwise it becomes `latest`. `build.mjs` rejects malformed tags before replacing output. it wipes its `--out` directory: it refuses the repo root, its ancestors, the home directory, `/`, and a non-empty directory without the marker file it writes. after 72 hours a version can no longer be unpublished; use `npm deprecate` instead.

## 5. verify

```
npx sudo-mudo-apt-opt-gop-stop-remove@X.Y.Z --version
cargo install sudo-mudo-apt-opt-gop-stop-remove --version X.Y.Z --force && sudo-mudo-apt-opt-gop-stop-remove --version
```

## 6. homebrew and scoop

the release workflow renders `sudo-mudo-apt-opt-gop-stop-remove.rb` and `sudo-mudo-apt-opt-gop-stop-remove.json` into the workflow artifact `packaging` (not a release asset). download it with `gh run download <run-id> -n packaging`, or render it yourself with `.github/scripts/render-packaging.sh vX.Y.Z`, which downloads the public `.sha256` files.

copy the `.rb` file to `Formula/` in `archhaze24/homebrew-tap` and the `.json` file to `bucket/` in `archhaze24/scoop-bucket`, commit and push. check with `brew install archhaze24/tap/sudo-mudo-apt-opt-gop-stop-remove` and `scoop bucket add archhaze24 https://github.com/archhaze24/scoop-bucket && scoop install sudo-mudo-apt-opt-gop-stop-remove`. skip this step for a prerelease. the templates are in `packaging/`.

## notes

- the macos binaries are not signed or notarized: a downloaded archive may need `xattr -d com.apple.quarantine sudo-mudo-apt-opt-gop-stop-remove`. installing through npm, cargo or homebrew does not trigger gatekeeper.
- the checksum files only detect corruption, not tampering with the release itself.
- the musl platform packages carry a fully static binary; the launcher also detects musl at runtime and falls back to it when the glibc package is missing.
