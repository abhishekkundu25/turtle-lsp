#!/usr/bin/env bash
# Download the OWL Functional syntax files listed in ofn-sources.tsv into
# test/corpus-ofn/ (not tracked), for `ofn_real_files` in src/ofn/tests.rs
# (OFN_REAL=1). Not under test/corpus/: the corpus tests read that folder's
# presence as "the corpus is fetched". Re-running skips files already
# present; pass -f to re-download.
#
# Every URL is https (plain http is refused, redirects included). Each file's
# sha256 is compared with its row after the download and for files already
# present: a change prints a warning and keeps the file; with CORPUS_STRICT=1
# it fails the run. A failed download makes the exit status 1.
# OFN_CORPUS_DIR overrides test/corpus-ofn (for tests).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"
out="${OFN_CORPUS_DIR:-$here/../../test/corpus-ofn}"
mkdir -p "$out"
out=$(cd "$out" && pwd)
force=${1:-}
failed=0

# An interrupted download leaves its .part file; remove it.
trap 'rm -f "$out"/*.part' EXIT

while IFS=$'\t' read -r name url sha; do
  case "$name" in ''|'#'*) continue ;; esac
  file="$out/$name"
  if [ -s "$file" ] && [ "$force" != -f ]; then
    check_sum "$name" "$file" "$sha"
    continue
  fi
  if download -o "$file.part" "$url"; then
    mv "$file.part" "$file"
    printf '%-24s %10s bytes\n' "$name" "$(wc -c < "$file" | tr -d ' ')"
    check_sum "$name" "$file" "$sha"
  else
    rm -f "$file.part"
    printf '%-24s FAILED %s\n' "$name" "$url"
    failed=$((failed + 1))
  fi
done < "$here/ofn-sources.tsv"

status=0
checksum_summary || status=1
[ "$failed" -eq 0 ] || status=1
exit "$status"
