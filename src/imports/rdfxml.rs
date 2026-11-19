//! RDF/XML as N-Triples text, with limits on what the XML may ask for.

/// RDF/XML with a longer start tag is refused: quick-xml's cost per start tag
/// grows faster than its size (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
const MAX_XML_TAG_BYTES: usize = 64 * 1024;
/// Entity declarations allowed in an RDF/XML DOCTYPE, and their total size.
const MAX_XML_ENTITIES: usize = 256;
const MAX_XML_ENTITY_BYTES: usize = 64 * 1024;

/// RDF/XML as N-Triples text, and where and why the XML stopped early: at
/// its first error, as "line L, column C: why" (columns count characters).
/// The triples read before that error are kept. Fails when the error comes
/// before the first triple.
pub fn rdf_xml_to_ntriples(bytes: &[u8], base: &str) -> Result<(String, Option<String>), String> {
    check_xml_tags(bytes)?;
    let swaps = check_xml_entities(bytes)?;
    // `oxrdfxml` reads only double-quoted entity values. A swap keeps every
    // offset, so the positions of errors stay those of the file.
    let swapped;
    let input = if swaps.is_empty() {
        bytes
    } else {
        let mut copy = bytes.to_vec();
        for i in swaps {
            copy[i] = b'"';
        }
        swapped = copy;
        &swapped[..]
    };
    let parser = oxrdfxml::RdfXmlParser::new();
    let parser = parser.clone().with_base_iri(base).unwrap_or(parser);
    let mut out = String::new();
    let mut count = 0usize;
    let mut triples = parser.for_slice(input);
    while let Some(t) = triples.next() {
        match t {
            Ok(t) => {
                use std::fmt::Write;
                let _ = writeln!(out, "{t} .");
                count += 1;
            }
            Err(e) => {
                let (line, column) = line_column(input, triples.buffer_position());
                let at = format!("line {line}, column {column}: {e}");
                return if count == 0 {
                    Err(at)
                } else {
                    Ok((out, Some(at)))
                };
            }
        }
    }
    Ok((out, None))
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

/// Refuse XML with a start tag longer than [`MAX_XML_TAG_BYTES`].
fn check_xml_tags(bytes: &[u8]) -> Result<(), String> {
    let mut i = 0;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'<') {
        let start = i + off;
        let rest = &bytes[start..];
        let skip_to = |end: &[u8]| {
            rest.windows(end.len())
                .position(|w| w == end)
                .map_or(bytes.len(), |p| start + p + end.len())
        };
        if rest.starts_with(b"<!--") {
            i = skip_to(b"-->");
            continue;
        }
        if rest.starts_with(b"<![CDATA[") {
            i = skip_to(b"]]>");
            continue;
        }
        let mut quote = None;
        let mut end = bytes.len();
        for (j, &b) in rest.iter().enumerate().skip(1) {
            match (quote, b) {
                (None, b'"' | b'\'') => quote = Some(b),
                (Some(q), _) if q == b => quote = None,
                (None, b'>') => {
                    end = start + j + 1;
                    break;
                }
                _ => {}
            }
            if j > MAX_XML_TAG_BYTES {
                return Err(format!(
                    "an XML tag is longer than {} KiB",
                    MAX_XML_TAG_BYTES / 1024
                ));
            }
        }
        i = end;
    }
    Ok(())
}

/// Refuse DOCTYPE entities that could expand beyond the input: an entity whose
/// value refers to another entity (the "billion laughs" pattern), parameter or
/// external entities, and too many or too large declarations. Ontologies
/// exported by Protégé and friends only use flat namespace entities
/// (`<!ENTITY owl "http://www.w3.org/2002/07/owl#">`), which pass, as do
/// character references (`&#35;`, `&#x23;`): `oxrdfxml` expands those once,
/// and what they give is never read as a reference again.
///
/// Returns the offsets of the quotes to swap for `"`: those of each
/// single-quoted value inside the DOCTYPE that holds no `"`.
fn check_xml_entities(bytes: &[u8]) -> Result<Vec<usize>, String> {
    let Some(dt) = find(bytes, b"<!DOCTYPE") else {
        return Ok(Vec::new());
    };
    let refuse = |why: &str| {
        Err(format!(
            "its DOCTYPE {why}; refused to avoid entity expansion"
        ))
    };
    let end = declaration_end(bytes, dt).unwrap_or(bytes.len());
    let mut swaps = Vec::new();
    let mut i = dt;
    let (mut count, mut total) = (0usize, 0usize);
    while let Some(off) = find(&bytes[i..], b"<!ENTITY") {
        let mut j = i + off + b"<!ENTITY".len();
        count += 1;
        if count > MAX_XML_ENTITIES {
            return refuse(&format!("declares more than {MAX_XML_ENTITIES} entities"));
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) == Some(&b'%') {
            return refuse("declares a parameter entity");
        }
        // Name, then either a quoted value or SYSTEM/PUBLIC (external).
        while bytes.get(j).is_some_and(|b| !b.is_ascii_whitespace()) {
            j += 1;
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        let Some(&q) = bytes.get(j).filter(|b| matches!(b, b'"' | b'\'')) else {
            return refuse("declares an external entity");
        };
        let value_start = j + 1;
        let Some(len) = bytes[value_start..].iter().position(|&b| b == q) else {
            return refuse("has an unterminated entity value");
        };
        let value = &bytes[value_start..value_start + len];
        if value.contains(&b'%') || refers_to_an_entity(value) {
            return refuse("declares an entity that refers to another entity");
        }
        total += len;
        if total > MAX_XML_ENTITY_BYTES {
            return refuse(&format!(
                "declares more than {} KiB of entity text",
                MAX_XML_ENTITY_BYTES / 1024
            ));
        }
        let value_end = value_start + len;
        if q == b'\'' && value_end < end && !value.contains(&b'"') {
            swaps.extend([j, value_end]);
        }
        i = value_end + 1;
    }
    Ok(swaps)
}

/// Whether an entity value holds an `&` that does not start a character
/// reference (`&#` digits `;` or `&#x` hex digits `;`).
fn refers_to_an_entity(value: &[u8]) -> bool {
    let mut rest = value;
    while let Some(at) = rest.iter().position(|&b| b == b'&') {
        rest = &rest[at + 1..];
        let (digits, is_digit): (&[u8], fn(&u8) -> bool) = match rest {
            [b'#', b'x', tail @ ..] => (tail, u8::is_ascii_hexdigit),
            [b'#', tail @ ..] => (tail, u8::is_ascii_digit),
            _ => return true,
        };
        let n = digits.iter().take_while(|b| is_digit(b)).count();
        if n == 0 || digits.get(n) != Some(&b';') {
            return true;
        }
        rest = &digits[n + 1..];
    }
    false
}

/// The offset of the `>` that ends the `<!` declaration (a DOCTYPE) at
/// `start`, found as quick-xml finds it: the first `>` that closes every `<`
/// after the `<!`, quotes not considered.
pub(super) fn declaration_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(start + 2) {
        match b {
            b'<' => depth += 1,
            b'>' if depth == 0 => return Some(i),
            b'>' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
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
    fn rdf_xml_entity_expansion_is_refused() {
        let mut laughs =
            String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n<!ENTITY lol0 \"lol\">\n");
        for n in 1..9 {
            let refs: String = (0..10).map(|_| format!("&lol{};", n - 1)).collect();
            laughs.push_str(&format!("<!ENTITY lol{n} \"{refs}\">\n"));
        }
        laughs.push_str("]>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"http://e/a\"><rdf:value>&lol8;</rdf:value></rdf:Description></rdf:RDF>\n");
        assert!(check_xml_entities(laughs.as_bytes()).is_err());
        let flat = "<!DOCTYPE rdf:RDF [\n <!ENTITY owl \"http://www.w3.org/2002/07/owl#\" >\n <!ENTITY xsd 'http://www.w3.org/2001/XMLSchema#'>\n]>\n<rdf:RDF/>";
        assert!(check_xml_entities(flat.as_bytes()).is_ok());
        assert!(
            check_xml_entities(b"<!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]>")
                .is_err()
        );
        assert!(check_xml_entities(b"<!DOCTYPE x [<!ENTITY % p \"x\">]>").is_err());
        // A character reference is not an entity reference; anything else
        // after `&` is.
        for ok in ["a&#35;", "&#x23;b", "&#38;&#60;&#62;"] {
            assert!(!refers_to_an_entity(ok.as_bytes()), "{ok}");
        }
        for bad in [
            "&lol;", "&#;", "&#x;", "&#X23;", "&#12", "&#1a;", "a & b", "&",
        ] {
            assert!(refers_to_an_entity(bad.as_bytes()), "{bad}");
        }
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
        let at = |s: &str| xml.find(s).unwrap();
        let swaps = check_xml_entities(xml.as_bytes()).unwrap();
        assert_eq!(swaps, [at("'http"), at("#'>") + 1]);
        // A single-quoted value holding `"` is left to the parser, which
        // refuses it; so is a declaration after the DOCTYPE (a comment).
        let quoted = with_doctype("<!ENTITY e 'say \"hi\"'>", "");
        assert!(check_xml_entities(quoted.as_bytes()).unwrap().is_empty());
        assert!(rdf_xml_to_ntriples(quoted.as_bytes(), "http://example.org/").is_err());
        let after = "<!DOCTYPE rdf:RDF [<!ENTITY a 'x'>]>\n<!-- <!ENTITY b 'y'> -->\n<rdf:RDF/>";
        let swaps = check_xml_entities(after.as_bytes()).unwrap();
        assert_eq!(swaps.len(), 2);
        assert!(swaps.iter().all(|&i| i < after.find("]>").unwrap()));
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
        let mut long = b"<?xml version='1.0'?><rdf:RDF ".to_vec();
        long.extend(std::iter::repeat_n(b' ', MAX_XML_TAG_BYTES + 10));
        long.extend(b"/>");
        assert!(
            rdf_xml_to_ntriples(&long, "http://ex.com/")
                .unwrap_err()
                .contains("KiB")
        );
    }
}
