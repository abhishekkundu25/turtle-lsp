#!/usr/bin/env bash
# Checks that vendor/spareval is spareval 0.2.7 as published on crates.io plus
# cancellation-in-joins.patch, and nothing else. Run by CI (the audit job);
# locally: tools/check-vendor.sh. Needs curl, tar, patch and diff.
#
# When the vendored copy moves to another release, change VERSION and SHA256
# (the checksum is the "cksum" of the release in the crates.io index) and
# regenerate the patch (vendor/spareval/PATCH.md).
set -euo pipefail

VERSION=0.2.7
SHA256=85b51144836ff9ca2f792b7a2422c4170ff04460d86cd8f2f764cf53e339cbec

vendor=$(cd "$(dirname "$0")/../vendor/spareval" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl --fail --silent --show-error --location --retry 3 \
  --output "$work/spareval.crate" \
  "https://static.crates.io/crates/spareval/spareval-$VERSION.crate"

if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$work/spareval.crate" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$work/spareval.crate" | cut -d' ' -f1)
fi
if [ "$actual" != "$SHA256" ]; then
  echo "spareval-$VERSION.crate has sha256 $actual, expected $SHA256" >&2
  exit 1
fi

tar -xzf "$work/spareval.crate" -C "$work"
upstream="$work/spareval-$VERSION"

# The patch is a diff of oxigraph's tree (a/lib/spareval/src/eval.rs); the
# crate has src/eval.rs at its top.
patch --quiet --strip=3 --directory="$upstream" --input="$vendor/cancellation-in-joins.patch"

# What the vendored copy adds to the crate: the licence file (from the same
# oxigraph commit) and the notes on the patch. The crate's own Cargo.lock is
# not vendored.
if ! diff --recursive --unified \
  --exclude=Cargo.lock --exclude=LICENSE-MIT --exclude=PATCH.md --exclude=cancellation-in-joins.patch \
  "$upstream" "$vendor"; then
  echo "vendor/spareval differs from spareval $VERSION plus cancellation-in-joins.patch" >&2
  exit 1
fi
echo "vendor/spareval is spareval $VERSION plus cancellation-in-joins.patch"
