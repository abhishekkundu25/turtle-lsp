//! RDF/XML as N-Triples text, for the cache of a fetched import (a cache
//! file holds Turtle). The RDF/XML frontend ([`crate::rdfxml`]) reads it,
//! with its limits on what the XML may ask for; a local import is analysed
//! by that frontend directly, over the file's own text.

use crate::rdf::dataset::node_term;
use crate::syntax::SyntaxError;

/// RDF/XML as N-Triples text, and where and why the XML stopped early: at
/// its first error, as "line L, column C: why" (columns count characters).
/// The triples read before that error are kept. Fails when the error comes
/// before the first triple, or the text is refused before it is read.
pub fn rdf_xml_to_ntriples(bytes: &[u8], base: &str) -> Result<(String, Option<String>), String> {
    let text = String::from_utf8_lossy(bytes);
    // A refusal has no place: it is about the whole text.
    if let Some(why) = crate::rdfxml::refused(&text) {
        return Err(why);
    }
    let parsed = crate::rdfxml::parse(&text, Some(base));
    let stop = parsed.errors.first().map(|e| stopped_at(&text, e));
    if parsed.triples.is_empty()
        && let Some(stop) = stop
    {
        return Err(stop);
    }
    let mut out = String::new();
    for t in &parsed.triples {
        use std::fmt::Write;
        let (s, o) = (node_term(&t.s, 0), node_term(&t.o, 0));
        let _ = writeln!(out, "{s} <{}> {o} .", t.p);
    }
    Ok((out, stop))
}

/// Where and why an RDF/XML text stops being read: "line L, column C: why",
/// at the end of the error's span.
pub(super) fn stopped_at(text: &str, e: &SyntaxError) -> String {
    let (line, column) = line_column(text.as_bytes(), u64::from(e.span.end));
    format!("line {line}, column {column}: {}", e.message())
}

/// The line and column (both from 1; the column in characters) of byte
/// `offset` of `text`.
fn line_column(text: &[u8], offset: u64) -> (usize, usize) {
    let end = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(text.len());
    let before = &text[..end];
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    // Skip UTF-8 continuation bytes.
    let column = before[line_start..]
        .iter()
        .filter(|&&b| b & 0xC0 != 0x80)
        .count()
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::load::Format;
    use crate::imports::load::parse;

    const RDF: &str = "xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"";

    /// `<rdf:RDF>` around `body`, after a DOCTYPE declaring `entities`.
    fn with_doctype(entities: &str, body: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n{entities}\n]>\n<rdf:RDF {RDF}>\n{body}\n</rdf:RDF>\n"
        )
    }

    fn ntriples(xml: &str) -> String {
        let (nt, truncated) = rdf_xml_to_ntriples(xml.as_bytes(), "http://example.org/x").unwrap();
        assert_eq!(truncated, None, "{nt}");
        nt
    }

    #[test]
    fn character_references_in_entity_values_are_expanded_once() {
        let xml = with_doctype(
            "<!ENTITY ex \"http://example.org/a&#35;\">\n<!ENTITY hex 'http://example.org/b&#x23;'>\n<!ENTITY esc \"x&#38;lol0;y\">",
            "<rdf:Description rdf:about=\"&ex;S\"><rdf:type rdf:resource=\"&hex;T\"/><rdf:value>&esc;</rdf:value></rdf:Description>",
        );
        let nt = ntriples(&xml);
        assert!(nt.contains("<http://example.org/a#S>"), "{nt}");
        assert!(nt.contains("<http://example.org/b#T>"), "{nt}");
        // `&#38;` gives `&`, which is not read as the start of a reference
        // again: no entity grows another.
        assert!(nt.contains("\"x&lol0;y\""), "{nt}");
    }

    #[test]
    fn single_quoted_entity_values_load_and_keep_their_offsets() {
        let xml = with_doctype(
            "<!ENTITY ex 'http://example.org/q#'>\n<!ENTITY dq \"http://example.org/d#\">",
            "<rdf:Description rdf:about=\"&ex;S\"><rdf:type rdf:resource=\"&dq;T\"/></rdf:Description>",
        );
        let nt = ntriples(&xml);
        assert!(nt.contains("<http://example.org/q#S>"), "{nt}");
        assert!(nt.contains("<http://example.org/d#T>"), "{nt}");
        // A single-quoted value holding `"` is left to the parser, which
        // refuses it.
        let quoted = with_doctype("<!ENTITY e 'say \"hi\"'>", "");
        assert!(rdf_xml_to_ntriples(quoted.as_bytes(), "http://example.org/").is_err());
    }

    #[test]
    fn a_mid_file_error_keeps_the_triples_before_it_and_says_where() {
        let xml = format!(
            "<?xml version=\"1.0\"?>\n<rdf:RDF {RDF} xmlns:ex=\"http://example.org/\">\n  <ex:A rdf:about=\"http://example.org/a\"/>\n  <ex:B rdf:about=\"http://example.org/b\"></ex:C>\n  <ex:D rdf:about=\"http://example.org/d\"/>\n</rdf:RDF>\n"
        );
        let (nt, truncated) = rdf_xml_to_ntriples(xml.as_bytes(), "http://example.org/").unwrap();
        assert!(nt.contains("<http://example.org/a>"), "{nt}");
        assert!(!nt.contains("<http://example.org/d>"), "{nt}");
        let truncated = truncated.expect("the error is reported");
        // The end tag on line 4 that closes nothing open: after it.
        assert!(truncated.starts_with("line 4, column 49: "), "{truncated}");
        // Columns count characters, not bytes.
        assert_eq!(line_column("a\né😀x".as_bytes(), 8), (2, 3));
        assert_eq!(line_column(b"ab", 99), (1, 3));
        // An error before any triple is a failure, with its place.
        let e = rdf_xml_to_ntriples(b"<rdf:RDF", "http://example.org/").unwrap_err();
        assert!(e.starts_with("line 1, column "), "{e}");
    }

    #[test]
    fn rdf_xml_becomes_ntriples() {
        let xml = br##"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#">
  <owl:Class rdf:about="#Gear">
    <rdfs:label xml:lang="en">Gear "wheel"</rdfs:label>
  </owl:Class>
</rdf:RDF>"##;
        let (nt, truncated) = rdf_xml_to_ntriples(xml, "http://ex.com/rx").unwrap();
        assert_eq!(truncated, None);
        let o = parse(nt.as_bytes(), Format::Turtle, "http://ex.com/rx", false).unwrap();
        assert!(o.analysis.parsed.errors.is_empty(), "{nt}");
        assert_eq!(
            o.analysis.label("http://ex.com/rx#Gear"),
            Some("Gear \"wheel\"")
        );
        assert!(rdf_xml_to_ntriples(b"<rdf:RDF", "http://ex.com/").is_err());
        // What the frontend's limits refuse is not converted.
        let mut long = b"<?xml version='1.0'?><rdf:RDF ".to_vec();
        long.extend(std::iter::repeat_n(b' ', 70 * 1024));
        long.extend(b"/>");
        let e = rdf_xml_to_ntriples(&long, "http://ex.com/").unwrap_err();
        assert!(e.contains("KiB"), "{e}");
        assert_eq!(e, "an XML tag is longer than 64 KiB");
        let laughs = with_doctype("<!ENTITY a \"x\">\n<!ENTITY b \"&a;&a;\">", "");
        let e = rdf_xml_to_ntriples(laughs.as_bytes(), "http://ex.com/").unwrap_err();
        assert_eq!(
            e,
            "its DOCTYPE declares an entity that refers to another entity; refused to avoid entity expansion"
        );
        // A local file says the same.
        let e = parse(laughs.as_bytes(), Format::RdfXml, "http://ex.com/", true)
            .err()
            .unwrap();
        assert_eq!(
            e,
            "its DOCTYPE declares an entity that refers to another entity; refused to avoid entity expansion"
        );
    }

    #[test]
    fn blank_nodes_and_triple_terms_are_written_as_ntriples() {
        let xml = format!(
            "<rdf:RDF {RDF} rdf:version=\"1.2\" xmlns:ex=\"http://example.org/\">\n  <ex:A rdf:about=\"http://example.org/a\">\n    <ex:part><ex:Part><ex:name xml:lang=\"en\">x \\ \"y\"</ex:name></ex:Part></ex:part>\n    <ex:list rdf:parseType=\"Collection\"><ex:B/><ex:C/></ex:list>\n    <ex:says rdf:parseType=\"Triple\"><rdf:Description rdf:about=\"http://example.org/s\"><ex:q rdf:resource=\"http://example.org/o\"/></rdf:Description></ex:says>\n  </ex:A>\n</rdf:RDF>\n"
        );
        let nt = ntriples(&xml);
        let o = parse(nt.as_bytes(), Format::Turtle, "http://example.org/x", false).unwrap();
        assert!(o.analysis.parsed.errors.is_empty(), "{nt}");
        let from_xml = crate::rdfxml::parse(&xml, Some("http://example.org/x"));
        assert_eq!(
            o.analysis.parsed.triples.len(),
            from_xml.triples.len(),
            "{nt}"
        );
        assert!(nt.contains("\"x \\\\ \\\"y\\\"\"@en"), "{nt}");
        assert!(
            nt.contains(
                "<<( <http://example.org/s> <http://example.org/q> <http://example.org/o> )>>"
            ),
            "{nt}"
        );
    }
}
