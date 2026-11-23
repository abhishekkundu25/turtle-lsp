#!/usr/bin/env bash
# Convert the OWL Functional files of test/corpus-ofn/ (tools/corpus/fetch-ofn.sh)
# to OWL/XML with ROBOT, into test/corpus-ofn/robot-owx/ (not tracked), for
# `owx_robot_oracle` in tests/owx_robot.rs (OWX_ROBOT=1), which compares
# turtle-lsp's graph of each OWL/XML file with its graph of the Functional
# source.
#
# ROBOT is not fetched or committed here: point ROBOT at a robot.jar
# (https://github.com/ontodev/robot/releases, 1.9.11 was the one checked) and
# JAVA at a Java 11 or later (default: java on PATH). OFN_CORPUS_DIR overrides
# test/corpus-ofn. Imports are not followed: a catalog maps each one to an
# empty ontology of that IRI, as in tools/corpus/robot-ofn.sh (ROBOT writes the
# file's own axioms only, either way).
#
# Re-running converges: an output newer than its source is kept, and ROBOT
# writes under a temporary name (.part) that is moved into place only when it
# succeeds, so no partial output ever has the final name. A run that ends
# removes its .part files; one killed outright leaves them, and the next run
# overwrites and removes them.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dir="${OFN_CORPUS_DIR:-$here/../../test/corpus-ofn}"
: "${ROBOT:?set ROBOT to the path of robot.jar}"
java="${JAVA:-java}"
out="$dir/robot-owx"

while IFS=$'\t' read -r name _url _sha; do
  case "$name" in ''|'#'*) continue ;; esac
  [ -s "$dir/$name" ] || { echo "$name: missing; run tools/corpus/fetch-ofn.sh" >&2; exit 1; }
done < "$here/ofn-sources.tsv"

mkdir -p "$out/imports"
# A run stopped halfway leaves its .part files; remove them.
trap 'rm -f "$out"/*.part "$out"/imports/*.part' EXIT

while IFS=$'\t' read -r name _url _sha; do
  case "$name" in ''|'#'*) continue ;; esac
  src="$dir/$name"
  owx="$out/$name.owx"
  if [ "$owx" -nt "$src" ]; then
    printf '%-24s kept (newer than its source)\n' "$name"
    continue
  fi
  catalog="$out/imports/$name.xml"
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">'
    n=0
    { grep -o '^Import(<[^>]*>)' "$src" || true; } | sed 's/^Import(<\(.*\)>)$/\1/' | while IFS= read -r iri; do
      n=$((n + 1))
      printf 'Ontology(<%s>)\n' "$iri" > "$out/imports/$name.$n.ofn.part"
      mv "$out/imports/$name.$n.ofn.part" "$out/imports/$name.$n.ofn"
      printf '  <uri name="%s" uri="%s.%s.ofn"/>\n' "$iri" "$name" "$n"
    done
    echo '</catalog>'
  } > "$catalog.part"
  mv "$catalog.part" "$catalog"
  start=$(date +%s)
  "$java" -Xmx8g -jar "$ROBOT" convert --catalog "$catalog" -i "$src" --format owx -o "$owx.part"
  mv "$owx.part" "$owx"
  printf '%-24s %ss\n' "$name" "$(( $(date +%s) - start ))"
done < "$here/ofn-sources.tsv"
