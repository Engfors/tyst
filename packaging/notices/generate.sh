#!/usr/bin/env bash
# Regenerates THIRD_PARTY_NOTICES.md: header.md (models, native libraries, UI, AppImage) plus
# the license of every Rust crate in Cargo.lock that ships, via cargo-about.
# Usage: packaging/notices/generate.sh
# Needs cargo-about 0.9.3. The binary is behind the cli feature:
#   cargo install cargo-about --version 0.9.3 --locked --features cli
# CI installs that same release (taiki-e/install-action, cargo-about@0.9.3).
set -euo pipefail
cd "$(dirname "$0")/../.."
{
  cat packaging/notices/header.md
  cargo about generate --workspace --all-features --locked --fail \
    -c packaging/notices/about.toml packaging/notices/about.hbs
} >THIRD_PARTY_NOTICES.md
echo "wrote THIRD_PARTY_NOTICES.md ($(wc -l <THIRD_PARTY_NOTICES.md) lines)"
