#!/usr/bin/env bash
# prints the CHANGELOG.md section of a release tag or --unreleased to stdout.
# a prerelease tag without a section of its own falls back to "## [Unreleased]".
# --unreleased accepts a fallback tag for validation after the versioned notes are finalized.
# exits 1 when no non-empty section is found, so a release cannot go out without notes.
set -euo pipefail

tag="${1:?usage: release-notes.sh <tag|--unreleased> [changelog-file] [fallback-tag]}"
file="${2:-CHANGELOG.md}"
version="${tag#v}"
if [ "$tag" = "--unreleased" ]; then
  version="Unreleased"
fi

extract() {
  awk -v want="$1" '
    /^## \[/ {
      if (on) exit
      if (index($0, "## [" want "]") == 1) { on = 1; next }
    }
    on && /^\[[^]]+\]: / { next }  # link reference definitions at the end of the file
    on { print }
  ' "$file"
}

is_blank() { [ -z "$(printf '%s' "$1" | tr -d '[:space:]')" ]; }

notes="$(extract "$version")"
if is_blank "$notes" && [ "$tag" = "--unreleased" ] && [ -n "${3:-}" ]; then
  version="${3#v}"
  notes="$(extract "$version")"
fi
if is_blank "$notes" && [[ "$version" == *-* ]]; then
  notes="$(extract "Unreleased")"
fi
if is_blank "$notes"; then
  echo "::error::$file has no section '## [$version]' with content" >&2
  exit 1
fi
printf '%s\n' "$notes"
