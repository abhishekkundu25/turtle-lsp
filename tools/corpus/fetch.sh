#!/usr/bin/env bash
# Download the public corpus listed in sources.tsv into test/corpus/ (not
# tracked). Re-running skips files already present; pass -f to re-download.
#
# Every URL is https (plain http is refused, redirects included). After a file
# is stored, and for files already present, its sha256 is compared with the
# column in sources.tsv: a change prints a warning and keeps the file; with
# CORPUS_STRICT=1 it fails the run. A download that fails is listed and makes
# the exit status 1, after the other sources have run.
#
# Overrides (for tests): CORPUS_DIR (default test/corpus), CORPUS_SOURCES
# (default tools/corpus/sources.tsv).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"
out=${CORPUS_DIR:-$here/../../test/corpus}
sources=${CORPUS_SOURCES:-$here/sources.tsv}
mkdir -p "$out"
out=$(cd "$out" && pwd)
export CORPUS_DIR=$out
force=${1:-}
failed=0

# An interrupted download leaves only its .part files; remove them.
trap 'rm -rf "$out"/*.part "$out"/*.gz.part "$out"/*.tar.part "$out"/*.cut' EXIT

# failed_source NAME URL
failed_source() {
  printf '%-24s FAILED %s\n' "$1" "$2"
  failed=$((failed + 1))
}

while IFS=$'\t' read -r name format url sha; do
  case "$name" in ''|'#'*) continue ;; esac
  case "$format" in
    nq-head)
      # A slice of a large dump: the first 8 MB of the gzip stream, decompressed
      # as far as it goes and cut after the last complete line.
      file="$out/$name.nq"
      if [ -s "$file" ] && [ "$force" != -f ]; then check_sum "$name" "$file" "$sha"; continue; fi
      if download -r 0-8388607 -o "$file.gz.part" "$url"; then
        # The slice ends mid-stream, so gzip reports an error and head closes
        # the pipe early: neither is a failure.
        { gzip -dc < "$file.gz.part" 2>/dev/null | head -c 16777216 > "$file.part"; } || true
        rm -f "$file.gz.part"
        # Keep everything up to the last newline.
        perl -0777 -pe 's/[^\n]*\z//' "$file.part" > "$file.cut"
        mv "$file.cut" "$file"
        rm -f "$file.part"
        printf '%-24s %10s bytes\n' "$name" "$(wc -c < "$file" | tr -d ' ')"
        check_sum "$name" "$file" "$sha"
      else
        rm -f "$file.gz.part"
        failed_source "$name" "$url"
      fi
      continue
      ;;
    sparql-examples)
      # Unpacked into its own directory; the harness reads the .ttl files.
      dir="$out/$name"
      if [ -d "$dir" ] && [ "$force" != -f ]; then check_sum "$name" "$dir" "$sha"; continue; fi
      rm -rf "$dir" "$dir.part"
      mkdir -p "$dir.part"
      if download -o "$dir.tar.part" "$url" && tar -xzf "$dir.tar.part" -C "$dir.part" --strip-components 1; then
        rm -f "$dir.tar.part"
        mv "$dir.part" "$dir"
        printf '%-24s %10s .ttl files\n' "$name" "$(find "$dir" -name '*.ttl' | wc -l | tr -d ' ')"
        check_sum "$name" "$dir" "$sha"
      else
        rm -rf "$dir.part" "$dir.tar.part"
        failed_source "$name" "$url"
      fi
      continue
      ;;
  esac
  file="$out/$name.$format"
  if [ -s "$file" ] && [ "$force" != -f ]; then check_sum "$name" "$file" "$sha"; continue; fi
  case "$format" in
    ttl) accept="text/turtle" ;;
    nt) accept="application/n-triples" ;;
    nq) accept="application/n-quads" ;;
    trig) accept="application/trig" ;;
    rdf) accept="application/rdf+xml" ;;
    *) printf '%-24s unknown format %s\n' "$name" "$format"; failed=$((failed + 1)); continue ;;
  esac
  if download -H "Accept: $accept" -o "$file.part" "$url"; then
    mv "$file.part" "$file"
    printf '%-24s %10s bytes\n' "$name" "$(wc -c < "$file" | tr -d ' ')"
    check_sum "$name" "$file" "$sha"
  else
    rm -f "$file.part"
    failed_source "$name" "$url"
  fi
done < "$sources"

# The JSON-LD corpus (tools/corpus/jsonld-sources.tsv).
jsonld_status=0
"$here/fetch-jsonld.sh" "$force" || jsonld_status=$?

checksum_summary || failed=$((failed + 1))
if [ "$failed" -gt 0 ] || [ "$jsonld_status" -ne 0 ]; then
  printf 'corpus: %s source(s) failed or changed (JSON-LD status %s)\n' "$failed" "$jsonld_status" >&2
  exit 1
fi
