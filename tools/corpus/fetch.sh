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
  case "$format" in
    nq-head)
      # A slice of a large dump: the first 8 MB of the gzip stream, decompressed
      # as far as it goes and cut after the last complete line.
      file="$out/$name.nq"
      [ -s "$file" ] && [ "$force" != -f ] && continue
      if curl -sSL --fail --max-time 600 -r 0-8388607 -A "turtle-lsp-corpus" -o "$file.gz.part" "$url"; then
        gzip -dc < "$file.gz.part" 2>/dev/null | head -c 16777216 > "$file.part"
        rm -f "$file.gz.part"
        # Keep everything up to the last newline.
        perl -0777 -pe 's/[^\n]*\z//' "$file.part" > "$file" && rm -f "$file.part"
        printf '%-24s %10s bytes\n' "$name" "$(wc -c < "$file" | tr -d ' ')"
      else
        rm -f "$file.gz.part"
        printf '%-24s FAILED %s\n' "$name" "$url"
      fi
      continue
      ;;
    sparql-examples)
      # Unpacked into its own directory; the harness reads the .ttl files.
      dir="$out/$name"
      [ -d "$dir" ] && [ "$force" != -f ] && continue
      rm -rf "$dir" "$dir.part"
      mkdir -p "$dir.part"
      if curl -sSL --fail --max-time 600 -A "turtle-lsp-corpus" "$url" | tar -xz -C "$dir.part" --strip-components 1; then
        mv "$dir.part" "$dir"
        printf '%-24s %10s .ttl files\n' "$name" "$(find "$dir" -name '*.ttl' | wc -l | tr -d ' ')"
      else
        rm -rf "$dir.part"
        printf '%-24s FAILED %s\n' "$name" "$url"
      fi
      continue
      ;;
  esac
  file="$out/$name.$format"
  [ -s "$file" ] && [ "$force" != -f ] && continue
  case "$format" in
    ttl) accept="text/turtle" ;;
    nt) accept="application/n-triples" ;;
    nq) accept="application/n-quads" ;;
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
