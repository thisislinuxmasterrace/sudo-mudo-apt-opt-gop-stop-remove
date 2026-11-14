#!/usr/bin/env bash
# renders the homebrew formula and the scoop manifest for a release tag.
#
#   render-packaging.sh vX.Y.Z [ASSET_DIR [OUT_DIR]]
#
# checksums come from the release's `<name>-<tag>-<target>.<ext>.sha256` files: read from ASSET_DIR
# when given, otherwise downloaded from the public github release url (no token).
# writes <name>.rb and <name>.json into OUT_DIR (default: ./packaging-out).
# pass "-" as ASSET_DIR to download while still choosing OUT_DIR.
set -euo pipefail

tag="${1:?usage: render-packaging.sh <tag> [asset-dir|-] [out-dir]}"
assets="${2:--}"
out="${3:-packaging-out}"
version="${tag#v}"
name="sudo-mudo-apt-opt-gop-stop-remove"
repo="thisislinuxmasterrace/sudo-mudo-apt-opt-gop-stop-remove"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if ! [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$tag' is not a vX.Y.Z tag" >&2
  exit 2
fi

# target:extension of every archive that a template uses
archives=(
  "aarch64-apple-darwin:tar.gz"
  "x86_64-apple-darwin:tar.gz"
  "aarch64-unknown-linux-musl:tar.gz"
  "x86_64-unknown-linux-musl:tar.gz"
  "x86_64-pc-windows-msvc:zip"
)

mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sed_args=(-e "s|@VERSION@|$version|g" -e "s|@TAG@|$tag|g")
for entry in "${archives[@]}"; do
  target="${entry%%:*}"
  ext="${entry#*:}"
  file_name="${name}-${tag}-${target}.${ext}.sha256"
  if [ "$assets" = "-" ]; then
    curl --fail --silent --show-error --location --proto '=https' \
      -o "$work/$file_name" "https://github.com/$repo/releases/download/$tag/$file_name"
    file="$work/$file_name"
  else
    file="$assets/$file_name"
  fi
  [ -f "$file" ] || { echo "error: missing $file" >&2; exit 1; }
  hash="$(awk 'NR == 1 { print $1 }' "$file")"
  if ! [[ "$hash" =~ ^[0-9a-f]{64}$ ]]; then
    echo "error: $file does not start with a sha256 hex digest" >&2
    exit 1
  fi
  sed_args+=(-e "s|@SHA_${target//-/_}@|$hash|g")
done

sed "${sed_args[@]}" "$root/packaging/homebrew/$name.rb.in" > "$out/$name.rb"
sed "${sed_args[@]}" "$root/packaging/scoop/$name.json.in" > "$out/$name.json"

if grep -n '@[A-Z_]*@\|@SHA_' "$out/$name.rb" "$out/$name.json"; then
  echo "error: unreplaced placeholder left in the output" >&2
  exit 1
fi
echo "rendered $out/$name.rb and $out/$name.json for $tag"
