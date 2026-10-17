#!/usr/bin/env bash
# Download the public corpus listed in sources.tsv into test/corpus/ (not
# tracked). Re-running skips files already present; pass -f to re-download.
set -u
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../test/corpus"
mkdir -p "$out"
force=${1:-}
while IFS=$'\t' read -r name format url; do
  case "$name" in ''|'#'*) continue ;; esac
  file="$out/$name.$format"
  [ -s "$file" ] && [ "$force" != -f ] && continue
  case "$format" in
    ttl) accept="text/turtle" ;;
    nt) accept="application/n-triples" ;;
    trig) accept="application/trig" ;;
    rdf) accept="application/rdf+xml" ;;
  esac
  if curl -sSL --fail --max-time 600 -H "Accept: $accept" -A "turtle-lsp-corpus" -o "$file.part" "$url"; then
    mv "$file.part" "$file"
    printf '%-24s %10s bytes\n' "$name" "$(wc -c < "$file" | tr -d ' ')"
  else
    rm -f "$file.part"
    printf '%-24s FAILED %s\n' "$name" "$url"
  fi
done < "$here/sources.tsv"
