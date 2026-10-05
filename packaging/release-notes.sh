#!/usr/bin/env bash
# Prints the CHANGELOG.md section for version $1 (the lines under `## $1 ...` up to the next `## `),
# which becomes the text of the GitHub release. Fails if the section is missing or holds only
# whitespace. Used by .github/workflows/release.yml.
set -euo pipefail
version=${1:?usage: release-notes.sh VERSION [CHANGELOG]}
changelog=${2:-CHANGELOG.md}
# The version goes through the environment: `awk -v` would interpret backslash escapes in it.
notes=$(VERSION=$version awk '
  { sub(/\r$/, "") }
  /^## / { if (found) exit; split($0, w, " "); v = ENVIRON["VERSION"]
           if (w[2] == v || w[2] == "[" v "]") { found = 1; next } }
  found { print }
' "$changelog" | sed -e '/[^[:space:]]/,$!d')
if [ -z "${notes//[[:space:]]/}" ]; then
  echo "::error::no section for $version in $changelog" >&2
  exit 1
fi
printf '%s\n' "$notes"
