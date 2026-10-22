#!/usr/bin/env bash
# Download the public JSON-LD corpus listed in jsonld-sources.tsv into
# test/corpus/jsonld/ (not tracked), with the remote @context documents it
# needs, so tests/jsonld_oracle.rs runs offline. Re-running skips files
# already present; pass -f to re-download.
#
# Every URL is https (plain http is refused, redirects included). Each file's
# sha256 is compared with jsonld-sha256.tsv after the download and for files
# already present: a change prints a warning and keeps the file; with
# CORPUS_STRICT=1 it fails the run. A failed download makes the exit status 1.
# CORPUS_DIR overrides test/corpus (for tests).
#
# Writes test/corpus/jsonld/index.tsv, read by the oracle test:
#   doc<TAB>PATH<TAB>BASE         a document and the base IRI to parse it with
#   context<TAB>PATH<TAB>IRI      the file to serve for a remote context IRI
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"
out="${CORPUS_DIR:-$here/../../test/corpus}/jsonld"
mkdir -p "$out"
out=$(cd "$out" && pwd)
force=${1:-}
sums="$here/jsonld-sha256.tsv"
index="$out/index.tsv.part"
: > "$index"
fetched=0 failed=0

# An interrupted run leaves its .part files and a partial index; remove them.
trap 'rm -f "$index"; find "$out" -name "*.part" -type f -delete' EXIT

# expected_sha KEY: the sha256 recorded for a path under jsonld/, or nothing.
expected_sha() {
  awk -F'\t' -v p="$1" '$1 == p { print $2; exit }' "$sums"
}

# get URL FILE: download unless present; 0 when FILE exists afterwards. The
# file's sha256 is checked either way.
get() {
  local key=${2#"$out"/}
  if [ -s "$2" ] && [ "$force" != -f ]; then
    check_sum "$key" "$2" "$(expected_sha "$key")"
    return 0
  fi
  mkdir -p "$(dirname "$2")"
  if download --max-time 120 -H "Accept: application/ld+json, application/json;q=0.9" -o "$2.part" "$1"; then
    mv "$2.part" "$2"
    fetched=$((fetched + 1))
    check_sum "$key" "$2" "$(expected_sha "$key")"
  else
    rm -f "$2.part"
    printf 'FAILED %s\n' "$1"
    failed=$((failed + 1))
    return 1
  fi
}

while IFS=$'\t' read -r kind name url licence source serves; do
  case "$kind" in ''|'#'*) continue ;; esac
  case "$kind" in
    doc)
      get "$url" "$out/$name" && printf 'doc\t%s\t%s\n' "$name" "$url" >> "$index"
      ;;
    context)
      if get "$url" "$out/$name"; then
        for iri in ${serves//,/ }; do
          printf 'context\t%s\t%s\n' "$name" "$iri" >> "$index"
        done
      fi
      ;;
    schemaorg-examples)
      # One schema.org examples file holds many examples; each JSON-LD
      # <script> block becomes NAME/eg-NNNN.jsonld.
      txt="$out/$name.txt"
      get "$url" "$txt" || continue
      mkdir -p "$out/$name"
      perl -0777 -e '
        my $dir = $ARGV[1];
        open my $f, "<", $ARGV[0] or die; my $t = <$f>;
        for my $ex (split /^TYPES:/m, $t) {
          my ($id) = $ex =~ /^\s*#(eg-\d+)/ or next;
          my ($json) = $ex =~ /^JSON:\s*\n(.*?)(?=^[A-Z-]+:\s*$|\z)/ms or next;
          my ($ld) = $json =~ m{<script[^>]*application/ld\+json[^>]*>(.*?)</script>}s or next;
          $ld =~ s/^\s+|\s+$//g;
          open my $o, ">", "$dir/$id.jsonld" or die; print $o "$ld\n"; close $o;
          print "$id\n";
        }' "$txt" "$out/$name" | while read -r id; do
        printf 'doc\t%s\t%s\n' "$name/$id.jsonld" "$url#$id" >> "$index"
      done
      ;;
    *) printf 'unknown kind %s for %s\n' "$kind" "$name" ;;
  esac
done < "$here/jsonld-sources.tsv"

mv "$index" "$out/index.tsv"
docs=$(grep -c '^doc' "$out/index.tsv" || true)
contexts=$(grep -c "^context" "$out/index.tsv" || true)
printf 'jsonld: %s documents, %s context IRIs, %s downloaded, %s failed\n' \
  "$docs" "$contexts" "$fetched" "$failed"
status=0
checksum_summary || status=1
[ "$failed" -eq 0 ] || status=1
exit "$status"
