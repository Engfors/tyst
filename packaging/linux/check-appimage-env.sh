#!/usr/bin/env bash
# Fails if the AppImage's launcher sets an environment variable that
# crates/tyst-platform/src/appimage.rs does not strip for programs Tyst starts.
# Usage: packaging/linux/check-appimage-env.sh path/to/Tyst.AppImage
set -euo pipefail
image=$(realpath "$1")
src=$(realpath "$(dirname "$0")/../../crates/tyst-platform/src/appimage.rs")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$work" && "$image" --appimage-extract >/dev/null)
root=$work/squashfs-root
vars=$( {
  strings "$root/AppRun.wrapped" | grep -oE '^[A-Z][A-Z0-9_]*=' | tr -d '='
  cat "$root"/apprun-hooks/*.sh | grep -oE '^ *export [A-Z][A-Z0-9_]*' | awk '{print $2}'
} | sort -u)
[ -n "$vars" ] || { echo "found no launcher variables; has the AppImage layout changed?"; exit 1; }
missing=0
for v in $vars; do
  if ! grep -q "\"$v\"" "$src"; then
    echo "launcher sets $v, but appimage.rs does not handle it"
    missing=1
  fi
done
[ $missing -eq 0 ] && echo "all $(echo "$vars" | wc -l) launcher variables handled"
exit $missing
