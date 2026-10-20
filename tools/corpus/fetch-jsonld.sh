#!/usr/bin/env bash
# Download the public JSON-LD corpus listed in jsonld-sources.tsv into
# test/corpus/jsonld/ (not tracked), with the remote @context documents it
# needs, so tests/jsonld_oracle.rs runs offline. Re-running skips files
# already present; pass -f to re-download.
#
# Writes test/corpus/jsonld/index.tsv, read by the oracle test:
#   doc<TAB>PATH<TAB>BASE         a document and the base IRI to parse it with
#   context<TAB>PATH<TAB>IRI      the file to serve for a remote context IRI
set -u
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../test/corpus/jsonld"
mkdir -p "$out"
force=${1:-}
index="$out/index.tsv.part"
: > "$index"
fetched=0 failed=0

# get URL FILE: download unless present; 0 when FILE exists afterwards.
get() {
  [ -s "$2" ] && [ "$force" != -f ] && return 0
  mkdir -p "$(dirname "$2")"
  if curl -sSL --fail --max-time 120 -A "turtle-lsp-corpus" \
    -H "Accept: application/ld+json, application/json;q=0.9" -o "$2.part" "$1"; then
    mv "$2.part" "$2"
    fetched=$((fetched + 1))
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
docs=$(grep -c '^doc' "$out/index.tsv")
contexts=$(grep -c "^context" "$out/index.tsv")
printf 'jsonld: %s documents, %s context IRIs, %s downloaded, %s failed\n' \
  "$docs" "$contexts" "$fetched" "$failed"
