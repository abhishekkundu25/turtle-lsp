#!/usr/bin/env bash
# Checks that each folder under vendor/ is its crate as published on crates.io
# plus its patches, and nothing else. Run by CI (the audit job);
# locally: tools/check-vendor.sh. Needs curl, tar, patch and diff.
#
# When a vendored copy moves to another release, change its row in CRATES
# (the checksum is the "cksum" of the release in the crates.io index) and
# regenerate the patches (vendor/<crate>/PATCH.md).
set -euo pipefail

# name version sha256 strip-depth patch-files (comma-separated, "-" for none)
# The patches are diffs of oxigraph's tree (a/lib/<crate>/src/...);
# each crate has src/ at its top, hence depth 3.
CRATES=(
  "spareval 0.2.7 85b51144836ff9ca2f792b7a2422c4170ff04460d86cd8f2f764cf53e339cbec 3 cancellation-in-joins.patch,dir-lang-string-equality.patch"
  "spargebra 0.4.7 bef3c186ffb9bf2dde7c5edd5528583f309fcb819b5773cf0e313194a990304f 3 left-factor-nested-calls.patch,parse-fuel.patch"
)

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for row in "${CRATES[@]}"; do
  read -r name version sha256 strip patches <<<"$row"
  vendor="$root/vendor/$name"
  file="$work/$name.crate"

  curl --fail --silent --show-error --location --retry 3 \
    --output "$file" \
    "https://static.crates.io/crates/$name/$name-$version.crate"

  if command -v sha256sum >/dev/null; then
    actual=$(sha256sum "$file" | cut -d' ' -f1)
  else
    actual=$(shasum -a 256 "$file" | cut -d' ' -f1)
  fi
  if [ "$actual" != "$sha256" ]; then
    echo "$name-$version.crate has sha256 $actual, expected $sha256" >&2
    exit 1
  fi

  tar -xzf "$file" -C "$work"
  upstream="$work/$name-$version"

  # What the vendored copy adds to the crate: the licence file, the notes on
  # the patches and the patch files. The crate's own Cargo.lock is not
  # vendored.
  excludes=(--exclude=Cargo.lock --exclude=LICENSE-MIT --exclude=PATCH.md)
  if [ "$patches" != "-" ]; then
    IFS=, read -ra files <<<"$patches"
    for p in "${files[@]}"; do
      patch --quiet --strip="$strip" --directory="$upstream" --input="$vendor/$p"
      excludes+=("--exclude=$p")
    done
  fi

  if ! diff --recursive --unified "${excludes[@]}" "$upstream" "$vendor"; then
    echo "vendor/$name differs from $name $version plus its patches" >&2
    exit 1
  fi
  if [ "$patches" = "-" ]; then
    echo "vendor/$name is $name $version, unchanged"
  else
    echo "vendor/$name is $name $version plus ${patches//,/, }"
  fi
done
