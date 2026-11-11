#!/usr/bin/env bash
# Checks that data/vocab.tsv is what the pinned sources of
# data/vocab-sources.tsv and the corrections of data/vocab-patches.tsv give.
# Run by CI (the audit job); locally: tools/check-vocab.sh. Needs curl and node.
#
#   tools/check-vocab.sh [--write] [-f]
#
# Downloads each fetched row into target/vocab-sources/, then runs
# tools/build-vocab.mjs, which verifies every source's sha256, regenerates the
# list and compares it with the committed one. --write writes the list instead
# (npm run vocab). -f downloads every source again.
#
# A file already present with its row's sha256 is not downloaded again; one
# with another hash is. A download goes to a .part name and is renamed once
# its hash matches, so an interrupted run leaves only a .part file. A download
# whose hash is not its row's fails the run, naming the row, the expected hash
# and the one found; the file stays at its .part name for reading.
#
# When a publisher changes a file: read the change, copy the new hash into
# its row (and the version, if it has one), then run npm run vocab.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=corpus/lib.sh
. "$root/tools/corpus/lib.sh"

write=
force=
for arg in "$@"; do
  case "$arg" in
    --write) write=--write ;;
    -f) force=1 ;;
    *) echo "usage: tools/check-vocab.sh [--write] [-f]" >&2; exit 2 ;;
  esac
done

dir=$root/target/vocab-sources
mkdir -p "$dir"
list=$(node "$root/tools/build-vocab.mjs" --fetched --sources "$dir")

downloaded=0
present=0
failed=0
while IFS=$'\t' read -r file url want where; do
  [ -n "$file" ] || continue
  if [ -z "$force" ] && [ -f "$file" ] && [ "$(sha256_of "$file")" = "$want" ]; then
    present=$((present + 1))
    continue
  fi
  if ! download -o "$file.part" "$url"; then
    printf '%s: download failed: %s\n' "$where" "$url" >&2
    failed=$((failed + 1))
    continue
  fi
  got=$(sha256_of "$file.part")
  if [ "$got" != "$want" ]; then
    printf '%s: %s changed\n  expected sha256 %s\n  found    sha256 %s\n  kept at %s\n' \
      "$where" "$url" "$want" "$got" "${file#"$root"/}.part" >&2
    failed=$((failed + 1))
    continue
  fi
  mv "$file.part" "$file"
  downloaded=$((downloaded + 1))
  printf 'downloaded %s (%s bytes)\n' "${file#"$root"/}" "$(wc -c < "$file" | tr -d ' ')"
done <<<"$list"

echo "sources: $downloaded downloaded, $present already present, $failed failed"
if [ "$failed" -gt 0 ]; then
  exit 1
fi
node "$root/tools/build-vocab.mjs" ${write:+"$write"} --sources "$dir"
