# 02: The generator library, on small sources

**What to build:** a maintainer can turn a manifest, a corrections file and
a set of source files into the text of the vocabulary list with one
function, and trust it before any real source is fetched. The function
reads no file and opens no connection. Its caller hands it the manifest's
text, the corrections' text and each row's bytes, and gets back the list's
text and a summary of terms added and removed against a previous list.

The library implements the spec's "The inputs" and "The generator": the
manifest's columns, fetched sources in Turtle, N-Triples and RDF/XML,
frozen extracts, the `add` correction, the checksum check before parsing,
what a term is, the table of deprecation markers, the comment rule with its
cap, the code-point order, the aliases on the `@ns` line, the size budget
and the failures. Kind, label and comment are chosen as the generator
chooses them today, so a vocabulary read from the same triples comes out
the same.

The old generator and the committed list are not touched. 03 puts the
library to use.

**Blocked by:** None (can start immediately).

**Status:** done

- [ ] `n3` and `rdfxml-streaming-parser` are `devDependencies` at exact versions (today `n3` 2.13.12 and `rdfxml-streaming-parser` 3.3.0). `@zazuko/rdf-vocabularies` stays, since 01's generator reads it. `npm audit --audit-level=moderate` passes.
- [ ] New `test/vocab-build.test.mjs`, on small sources under `test/vocab/` with `ex:` names and `http://example.org/` namespaces. It runs offline and compares text with `\n` line ends, since `npm run test:node` runs it on Linux, macOS and Windows.
- [ ] One case per format: the same small vocabulary as Turtle, N-Triples, RDF/XML and a frozen extract gives the same `term` lines.
- [ ] One case per deprecation marker of the spec's table, and one term with no marker that stays unflagged. A frozen extract keeps its flags.
- [ ] Comments: the first paragraph only; whitespace collapsed; a long comment ends at the last sentence end inside the cap; with no sentence end inside the cap it ends at a word and "…"; a short comment is unchanged.
- [ ] Terms: a subject in an alias namespace is a term of its vocabulary; a local name with whitespace, `#` or `/` is not a term; a vocabulary with two source rows is the union of both; terms and vocabularies come out in code-point order, with one case where that differs from the locale's order.
- [ ] The `@ns` line carries the aliases as further fields, and has two fields when there are none.
- [ ] Corrections: an `add` puts its term in place with every column.
- [ ] Each failure of the spec's list stops the build with a message that names the row or line: a hash that differs, a source with no term, rows that disagree, a correction with an unknown prefix, a correction for a term the source has, a frozen extract that uses an unknown prefix, an output over the budget.
- [ ] The same inputs give the same bytes on a second call, and the summary lists added and removed terms per vocabulary.
- [ ] Proof that the rules did not drift: fed the old package's own files for `foaf`, `skos`, `prov`, `sh` and `schema`, the library gives those vocabularies' terms as the committed list has them, in every column but the comment. This is a one-off script, not a committed test, because 03 removes the package. Its output goes in the report.
- [ ] No Rust file changes, so clippy does not apply. `cargo fmt --check` passes.
- [ ] These pass:
  - `timeout 120 node --test --test-timeout=30000 test/vocab-build.test.mjs`
  - `timeout 120 node --test --test-timeout=30000 test/versions.test.mjs test/notices.test.mjs`
  - `npm audit --audit-level=moderate`
- [ ] The report gives the number of packages in `package-lock.json` before and after.
