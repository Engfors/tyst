#!/usr/bin/env bash
# Downloads the AppImage tools at pinned versions, checks their SHA-256 and puts them where the
# Tauri bundler looks for them ($XDG_CACHE_HOME/tauri), so `tauri build --bundles appimage`
# never fetches an unpinned tool itself (issue #14, PR7-003). Also fetches appimagetool, which
# finish-appimage.sh uses to repack the image.
# Usage: packaging/linux/fetch-tools.sh
set -euo pipefail

cache=${XDG_CACHE_HOME:-$HOME/.cache}
pinned=$cache/tyst-pinned-tools
tauri=$cache/tauri
mkdir -p "$pinned" "$tauri"

# name in the Tauri cache | URL | SHA-256
tools=(
  "AppRun-x86_64|https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64|f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f"
  "linuxdeploy-07333c6-x86_64.AppImage|https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-x86_64.AppImage|36a2d7e274d12e1050d0e9ecfe11d339ed54720b2bec464c286d53f8b07f5c62"
  "linuxdeploy-plugin-appimage.AppImage|https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/1-alpha-20250213-1/linuxdeploy-plugin-appimage-x86_64.AppImage|992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3"
  "appimagetool-x86_64.AppImage|https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage|ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
)

for entry in "${tools[@]}"; do
  IFS='|' read -r name url sha <<<"$entry"
  file=$pinned/$name
  if ! echo "$sha  $file" | sha256sum --check --status 2>/dev/null; then
    echo "fetching $name"
    curl -fsSL --retry 3 -o "$file.part" "$url"
    if ! echo "$sha  $file.part" | sha256sum --check --status; then
      echo "$name: SHA-256 mismatch (got $(sha256sum "$file.part" | cut -d' ' -f1))" >&2
      rm -f "$file.part"
      exit 1
    fi
    mv "$file.part" "$file"
  fi
  chmod 755 "$file"
  # Tauri patches its linuxdeploy copy in place, so the cache gets a fresh copy every time.
  [ "$name" = appimagetool-x86_64.AppImage ] || install -m 755 "$file" "$tauri/$name"
done
echo "tools pinned in $tauri; appimagetool: $pinned/appimagetool-x86_64.AppImage"
