#!/usr/bin/env bash
# Imports the release signing key from $KEY (ASCII-armoured private key) and writes its
# fingerprint as the step output `key`. Without a key, a tag build fails and a dry run goes on
# unsigned (empty `key`). A key other than the published release key (README, SECURITY.md) fails
# the build. Used by .github/workflows/release.yml.
set -euo pipefail
expected=6603C03926348BD98CE768DEE35BFE2B59ECA014
out=${GITHUB_OUTPUT:-/dev/stdout}
if [ -z "${KEY:-}" ]; then
  if [ "${REF_TYPE:-}" = tag ]; then
    echo "::error::RELEASE_GPG_KEY is not set in the release environment; a release must be signed"
    exit 1
  fi
  echo "no RELEASE_GPG_KEY: building unsigned (dry run)"
  echo "key=" >>"$out"
  exit 0
fi
printf '%s\n' "$KEY" | gpg --batch --import 2>&1 | grep -v '^gpg: key .*: secret key imported' || true
fpr=$(gpg --batch --list-secret-keys --with-colons | awk -F: '/^fpr:/ { print $10; exit }')
[ -n "$fpr" ] || { echo "::error::RELEASE_GPG_KEY holds no secret key"; exit 1; }
[ "$fpr" = "$expected" ] || { echo "::error::RELEASE_GPG_KEY is $fpr, not the release key $expected"; exit 1; }
echo "signing with $fpr"
echo "key=$fpr" >>"$out"
