# Licence of the OWL 2 test inputs

The files under `fs/` are the OWL 2 Functional-Style Syntax texts of the
approved test cases of the OWL 2 Test Case Repository, and the files under
`rdfxml/` the RDF/XML texts of the same ontologies where a test has one,
copied verbatim from the W3C archive of that repository:

- Source: https://www.w3.org/2009/11/owl-test/all.rdf (the W3C mirror of
  `http://owl.semanticweb.org/exports/`, linked from
  https://www.w3.org/2009/owl-test-cases), `Last-Modified: Wed, 18 Nov 2009
  15:49:34 GMT`, sha256
  `5383f1ddf4cf2f03703a2f886f41d4e5bc375633a1cfa94a03254fd89330f8bb`.
- Each file under `fs/` is the value of a `test:fsPremiseOntology`,
  `test:fsConclusionOntology` or `test:fsNonConclusionOntology` of a test
  whose `test:status` is `Approved`. `MANIFEST.tsv` names, for each file,
  the test, its role, types, status, syntaxes, semantics, species,
  profiles, author (`creator`) and IRI.
- Each file under `rdfxml/` is the value of the
  `test:rdfXmlPremiseOntology`, `test:rdfXmlConclusionOntology` or
  `test:rdfXmlNonConclusionOntology` of the same test and role as the
  `fs/` file of its name (`X.premise.rdf` beside `X.premise.ofn`): the 57
  rows of `MANIFEST.tsv` flagged `also-rdfxml-text`.

The W3C archive itself states no licence. The repository the archive
mirrors states one on its copyright page
(`http://owl.semanticweb.org/page/OWL_Test_Cases:Copyrights`, archived at
https://web.archive.org/web/20160702195814/http://owl.semanticweb.org/page/OWL_Test_Cases:Copyrights),
under "Terms of Licensing":

> All content of "OWL Test Cases" is published under the Creative Commons
> Attribution License 3.0. This previous clause includes all textual and
> other multimedia content that is contributed or uploaded to "OWL Test
> Cases". The license explicitly allows that contributed content is
> modified or used for derivatory works by other contributors. These terms
> of licensing cover the content on pages of "OWL Test Cases", as well as
> all RDF, OWL, or other data exports and collections created from such
> content.
>
> […] Original authorship attribution per test case is retained for all
> test cases that are published under the above terms.

So these files are used under the Creative Commons Attribution License 3.0
(https://creativecommons.org/licenses/by/3.0/).

Attribution: the authors of the test cases, named per file in the
`creator` column of `MANIFEST.tsv` (Alan Ruttenberg, Birte Glimm, Evren
Sirin, Mike Smith), and the W3C OWL Working Group. The files are not
changed; this copy holds only the Functional-Syntax texts and those
RDF/XML texts, one per file.
