# Licence of the OWL 2 Primer texts

The files here are example texts of a W3C Recommendation, copied verbatim:

- Document: *OWL 2 Web Ontology Language Primer (Second Edition)*, W3C
  Recommendation 11 December 2012. Editors: Pascal Hitzler, Markus
  Krötzsch, Bijan Parsia, Peter F. Patel-Schneider, Sebastian Rudolph.
- URL: https://www.w3.org/TR/2012/REC-owl2-primer-20121211/ (this version;
  latest version https://www.w3.org/TR/owl2-primer/).
- Copyright notice, as the document gives it: "Copyright © 2012 W3C® (MIT,
  ERCIM, Keio), All Rights Reserved. W3C liability, trademark and document
  use rules apply."
- Status: W3C Recommendation 11 December 2012. From its "Status of this
  Document": "This document has been reviewed by W3C Members, by software
  developers, and by other W3C groups and interested parties, and is
  endorsed by the Director as a W3C Recommendation. It is a stable document
  and may be used as reference material or cited from another document."

The document's "document use" rules are the W3C Document License
(https://www.w3.org/copyright/document-license-2023/, in effect since 1
January 2023), which says:

> Permission to copy, and distribute the contents of this document, or the
> W3C document from which this statement is linked, in any medium for any
> purpose and without fee or royalty is hereby granted, provided that you
> include the following on ALL copies of the document, or portions thereof,
> that you use:
>
> - A link or URL to the original W3C document.
> - The pre-existing copyright notice of the original author, or if it
>   doesn't exist, a notice (hypertext is preferred, but a textual
>   representation is permitted) of the form: "Copyright ©
>   [$date-of-document] World Wide Web Consortium.
>   https://www.w3.org/copyright/document-license-2023/"
> - If it exists, the STATUS of the W3C document.
>
> When space permits, inclusion of the full text of this NOTICE should be
> provided. We request that authorship attribution be provided in any
> software, documents, or other items or products that you create pursuant
> to the implementation of the contents of this document, or any portion
> thereof.

and, of code examples:

> In addition, "Code Components" —Web IDL in sections clearly marked as Web
> IDL; and W3C-defined markup (HTML, CSS, etc.) and computer programming
> language code clearly marked as code examples— are licensed under the W3C
> Software License.

The link, the copyright notice and the status are above. The notice the
licence gives:

> "Copyright © 2023 W3C®. This software or document includes material
> copied from or derived from [title and URI of the W3C document]."

with the title and URI: *OWL 2 Web Ontology Language Primer (Second
Edition)*, https://www.w3.org/TR/2012/REC-owl2-primer-20121211/.

## What was copied, and how

Nothing in the files is changed. Each file is the text of one example
block of the Primer, with the HTML character references decoded once, and
nothing else: no line is trimmed, re-indented or re-ended. The Primer's own
faults stay in the files; the tests list them (`KNOWN` in
`tests/owx_oracle.rs`) and correct them in memory only. So does the reading
of U+00A0 as a space: the HTML writes the space before a `:` or `;` in its
Functional and Turtle examples as `&nbsp;`, which decodes to U+00A0.

The extraction, which a short script did and which can be repeated:

- Input: the HTML of the Recommendation, fetched 2026-11-23, 267,804 bytes,
  sha256 `e64c94d2900e24142940cd45126651082f8e9a23e23396899f39ce4be9543630`.
- Each example gives one block per syntax: a `div` whose class is the
  syntax holds a `div.exampleheader` and then one `pre`, the text. The
  selectors are `div.fssyntax > pre` (Functional), `div.owlxml > pre`
  (OWL/XML), `div.rdfxml > pre` (RDF/XML), `div.turtle > pre` (Turtle);
  the fifth, `div.manchester > pre`, was not copied. Each selects 59 blocks,
  and the 297 `pre` elements of the page are these 295 and two others.
- In document order, the n-th `div.fssyntax` starts example n, and the
  RDF/XML, Turtle, Manchester and OWL/XML blocks of the example follow it,
  in that order. The `pre` elements hold no markup, only text and the
  references `&lt;`, `&gt;`, `&amp;` and `&nbsp;`.
- Files: for each example n (two digits), `n.owx` and `n.ofn`; for the
  appendix ontology (example 59, section 13), also `59.rdf` and `59.ttl`.
  120 files, written as UTF-8.
- `MANIFEST.tsv` gives, for each file, its example number, its syntax and
  its section: the text of the last section heading
  (`span.mw-headline` in an `h2`, `h3` or `h4`) before the block, runs of
  white space as one space.
