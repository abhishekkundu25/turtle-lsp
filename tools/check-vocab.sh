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
# (npm run vocab). -f downloads every source again: it deletes each file
# before its download, so the files that stay are the ones this run verified.
#
# A file already present with its row's sha256 is not downloaded again; one
# with another hash is. A download goes to a .part name and is renamed once
# its hash matches, so an interrupted run leaves only a .part file. A download
# whose hash is not its row's fails the run, naming the row, the expected hash
# and the one found; the file stays at its .part name for reading.
#
# A download has 90 seconds, less when it stalls, and two more tries. Once a
# host fails, its other rows are not tried in this run, so a host that is down
# costs the time of one file.
#
# Exit status: 0 when the list is what the sources give; 3 when a download
# failed and no hash differed, the one failure another run may not have (it
# downloads only what this one did not verify, when run without -f); 2 for a
# wrong argument; 1 for anything else: a changed hash, a refused source, a
# list that differs.
#
# When a publisher changes a file: read the change, copy the new hash into
# its row (and the version, if it has one), then run npm run vocab.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=corpus/lib.sh
. "$root/tools/corpus/lib.sh"

# One download of the manifest: lib.sh's (https only), with this script's
# limits and name. curl takes the last --max-time and -A it is given. A stall
# is under 1,000 bytes a second for 20 seconds.
fetch() {
  download --connect-timeout 15 --speed-limit 1000 --speed-time 20 --max-time 90 --retry 2 -A turtle-lsp "$@"
}

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
unreachable=0
changed=0
down=" "
while IFS=$'\t' read -r file url want where; do
  [ -n "$file" ] || continue
  if [ -n "$force" ]; then
    rm -f "$file"
  elif [ -f "$file" ] && [ "$(sha256_of "$file")" = "$want" ]; then
    present=$((present + 1))
    continue
  fi
  host=${url#https://}
  host=${host%%/*}
  if [[ $down == *" $host "* ]]; then
    printf '%s: not tried, %s failed above: %s\n' "$where" "$host" "$url" >&2
    unreachable=$((unreachable + 1))
    continue
  fi
  if ! fetch -o "$file.part" "$url"; then
    printf '%s: download failed: %s\n' "$where" "$url" >&2
    unreachable=$((unreachable + 1))
    down="$down$host "
    continue
  fi
  got=$(sha256_of "$file.part")
  if [ "$got" != "$want" ]; then
    printf '%s: %s changed\n  expected sha256 %s\n  found    sha256 %s\n  kept at %s\n' \
      "$where" "$url" "$want" "$got" "${file#"$root"/}.part" >&2
    changed=$((changed + 1))
    continue
  fi
  mv "$file.part" "$file"
  downloaded=$((downloaded + 1))
  printf 'downloaded %s (%s bytes)\n' "${file#"$root"/}" "$(wc -c < "$file" | tr -d ' ')"
done <<<"$list"

echo "sources: $downloaded downloaded, $present already present, $unreachable not downloaded, $changed changed"
if [ "$changed" -gt 0 ]; then
  exit 1
fi
if [ "$unreachable" -gt 0 ]; then
  exit 3
fi
node "$root/tools/build-vocab.mjs" ${write:+"$write"} --sources "$dir"
