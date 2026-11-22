#!/usr/bin/env bash
# Convert the OWL Functional files of test/corpus-ofn/ (tools/corpus/fetch-ofn.sh)
# to Turtle with ROBOT, into test/corpus-ofn/robot/ (not tracked), for
# `ofn_robot_oracle` in src/ofn/rdf_tests.rs (OFN_ROBOT=1), which compares
# ROBOT's graph of each file with turtle-lsp's mapping of it.
#
# ROBOT is not fetched or committed here: point ROBOT at a robot.jar
# (https://github.com/ontodev/robot/releases, 1.9.11 was the one checked) and
# JAVA at a Java 11 or later (default: java on PATH). ROBOT writes no
# N-Triples; its Turtle is read by oxttl in the test. Imports are not followed:
# a catalog maps each one to an empty ontology of that IRI (ROBOT writes the
# file's own axioms only, either way).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dir="${OFN_CORPUS_DIR:-$here/../../test/corpus-ofn}"
: "${ROBOT:?set ROBOT to the path of robot.jar}"
java="${JAVA:-java}"
mkdir -p "$dir/robot/imports"
while IFS=$'\t' read -r name _url _sha; do
  case "$name" in ''|'#'*) continue ;; esac
  [ -s "$dir/$name" ] || { echo "$name: missing; run tools/corpus/fetch-ofn.sh" >&2; exit 1; }
  catalog="$dir/robot/imports/$name.xml"
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">'
    n=0
    { grep -o '^Import(<[^>]*>)' "$dir/$name" || true; } | sed 's/^Import(<\(.*\)>)$/\1/' | while IFS= read -r iri; do
      n=$((n + 1))
      printf 'Ontology(<%s>)\n' "$iri" > "$dir/robot/imports/$name.$n.ofn"
      printf '  <uri name="%s" uri="%s.%s.ofn"/>\n' "$iri" "$name" "$n"
    done
    echo '</catalog>'
  } > "$catalog"
  start=$(date +%s)
  "$java" -Xmx8g -jar "$ROBOT" convert --catalog "$catalog" -i "$dir/$name" -o "$dir/robot/$name.ttl"
  printf '%-24s %ss\n' "$name" "$(( $(date +%s) - start ))"
done < "$here/ofn-sources.tsv"
