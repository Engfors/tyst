#!/usr/bin/env bash
# Repacks the AppImage the Tauri bundler made (issue #14, PR7-003/PR7-005): files in the image
# lose group and world write bits, and the release build embeds update information (with a
# .zsync file next to the image) and a GPG signature. The runtime is the one from the original
# image, so nothing is downloaded here.
# Usage: packaging/linux/finish-appimage.sh path/to/Tyst.AppImage [--update-info STRING] [--sign KEY_ID]
set -euo pipefail

image=$(realpath "$1")
shift
update_info=
sign_key=
while [ $# -gt 0 ]; do
  case $1 in
    --update-info) update_info=$2; shift 2 ;;
    --sign) sign_key=$2; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

tool=${APPIMAGETOOL:-${XDG_CACHE_HOME:-$HOME/.cache}/tyst-pinned-tools/appimagetool-x86_64.AppImage}
[ -x "$tool" ] || { echo "appimagetool not found at $tool (run packaging/linux/fetch-tools.sh)" >&2; exit 1; }

work=$(mktemp -d)
trap 'chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"' EXIT
offset=$(APPIMAGE_EXTRACT_AND_RUN=1 "$image" --appimage-offset)
head -c "$offset" "$image" >"$work/runtime"
(cd "$work" && "$image" --appimage-extract >/dev/null)
chmod -R go-w "$work/squashfs-root"

args=(--no-appstream --runtime-file "$work/runtime")
[ -n "$update_info" ] && args+=(--updateinformation "$update_info")
[ -n "$sign_key" ] && args+=(--sign --sign-key "$sign_key")
# The .zsync file names the image, so it is built under its final name.
mkdir "$work/out"
out=$work/out/$(basename "$image")
(cd "$work" && ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" "${args[@]}" squashfs-root "$out")
section() { objcopy -O binary --only-section="$1" "$out" "$work/$1" && tr -d '\0' <"$work/$1"; }
if [ -n "$update_info" ] && [ "$(section .upd_info)" != "$update_info" ]; then
  echo "update information missing from the repacked image" >&2
  exit 1
fi
if [ -n "$sign_key" ] && ! section .sha256_sig | grep -q 'BEGIN PGP SIGNATURE'; then
  echo "signature missing from the repacked image" >&2
  exit 1
fi
mv "$out" "$image"
zsync=$(find "$work" -name '*.zsync' | head -1)
if [ -n "$update_info" ]; then
  [ -n "$zsync" ] || { echo "no .zsync file was made (is zsyncmake installed?)" >&2; exit 1; }
  mv "$zsync" "$image.zsync"
fi
echo "repacked $image"
