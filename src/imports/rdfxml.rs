//! RDF/XML as N-Triples text, with limits on what the XML may ask for.

/// RDF/XML with a longer start tag is refused: quick-xml's cost per start tag
/// grows faster than its size (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
const MAX_XML_TAG_BYTES: usize = 64 * 1024;
/// Entity declarations allowed in an RDF/XML DOCTYPE, and their total size.
const MAX_XML_ENTITIES: usize = 256;
const MAX_XML_ENTITY_BYTES: usize = 64 * 1024;

/// RDF/XML as N-Triples text. Stops at the first error; fails only when that
/// leaves no triple at all.
pub fn rdf_xml_to_ntriples(bytes: &[u8], base: &str) -> Result<String, String> {
    check_xml_tags(bytes)?;
    check_xml_entities(bytes)?;
    let parser = oxrdfxml::RdfXmlParser::new();
    let parser = parser.clone().with_base_iri(base).unwrap_or(parser);
    let mut out = String::new();
    let mut count = 0usize;
    for t in parser.for_slice(bytes) {
        match t {
            Ok(t) => {
                use std::fmt::Write;
                let _ = writeln!(out, "{t} .");
                count += 1;
            }
            Err(e) if count == 0 => return Err(e.to_string()),
            Err(_) => break,
        }
    }
    Ok(out)
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
/// (`<!ENTITY owl "http://www.w3.org/2002/07/owl#">`), which pass.
fn check_xml_entities(bytes: &[u8]) -> Result<(), String> {
    let Some(dt) = find(bytes, b"<!DOCTYPE") else {
        return Ok(());
    };
    let refuse = |why: &str| {
        Err(format!(
            "its DOCTYPE {why}; refused to avoid entity expansion"
        ))
    };
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
        if value.contains(&b'&') || value.contains(&b'%') {
            return refuse("declares an entity that refers to another entity");
        }
        total += len;
        if total > MAX_XML_ENTITY_BYTES {
            return refuse(&format!(
                "declares more than {} KiB of entity text",
                MAX_XML_ENTITY_BYTES / 1024
            ));
        }
        i = value_start + len + 1;
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imports::Format;
    use crate::imports::load::parse;

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
        let nt = rdf_xml_to_ntriples(xml, "http://ex.com/rx").unwrap();
        let o = parse(nt.as_bytes(), Format::Turtle, "http://ex.com/rx").unwrap();
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
