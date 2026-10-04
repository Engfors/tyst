#!/usr/bin/env bash
# Checks a built AppImage (issue #14, PR7-009): the app binary is there and stripped, its
# libraries resolve inside the image or are ones the image expects from the host, the licenses
# ship with it, no model is bundled, and no file in it is group or world writable.
# Usage: packaging/linux/smoke-test-appimage.sh path/to/Tyst.AppImage
set -euo pipefail
image=$(realpath "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$work" && "$image" --appimage-extract >/dev/null)
root=$work/squashfs-root
bin=$root/usr/bin/tyst
fail=0

[ -x "$bin" ] || { echo "usr/bin/tyst is missing"; exit 1; }
if readelf -S "$bin" | grep -q '\.debug_info'; then
  echo "usr/bin/tyst carries debug info"
  fail=1
fi

needed=$(readelf -d "$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
[ -n "$needed" ] || { echo "readelf found no NEEDED entries"; exit 1; }
for lib in $needed; do
  if [ -e "$root/usr/lib/$lib" ]; then
    where=bundled
  elif ldconfig -p | grep -q "^[[:space:]]*$lib "; then
    where=host
  else
    echo "NEEDED $lib is neither in the image nor on this system"
    fail=1
    continue
  fi
  echo "NEEDED $lib ($where)"
done
# The WebKit and GTK stack must come with the image; the host's would not match.
for lib in libwebkit2gtk-4.1.so.0 libgtk-3.so.0; do
  if echo "$needed" | grep -qx "$lib" && [ ! -e "$root/usr/lib/$lib" ]; then
    echo "$lib is not bundled"
    fail=1
  fi
done

if ! find "$root/usr/lib" -name THIRD_PARTY_NOTICES.md | grep -q .; then
  echo "THIRD_PARTY_NOTICES.md is not in the image"
  fail=1
fi

models=$(find "$root" -iname '*.onnx' -o -iname '*.onnx.data' | sed "s|$root/||")
if [ -n "$models" ]; then
  echo "model files in the image:"; echo "$models"
  fail=1
fi

writable=$(find "$root" -perm /022 ! -type l | sed "s|$root/||")
if [ -n "$writable" ]; then
  echo "group or world writable files in the image:"; echo "$writable" | head -20
  fail=1
fi

[ $fail -eq 0 ] && echo "AppImage smoke test passed ($(du -h "$image" | cut -f1))"
exit $fail
