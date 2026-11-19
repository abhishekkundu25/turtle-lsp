//! What an RDF/XML text may ask of the XML reader, checked before it reads
//! a byte: no start tag longer than [`MAX_XML_TAG_BYTES`], and no DOCTYPE
//! entity that could expand beyond the input. Single-quoted entity values,
//! which `oxrdfxml` does not read, are given double quotes in place.

use crate::syntax::sniff::declaration_end;
use std::borrow::Cow;

/// RDF/XML with a longer start tag is refused: quick-xml's cost per start tag
/// grows faster than its size (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
pub(super) const MAX_XML_TAG_BYTES: usize = 64 * 1024;
/// Entity declarations allowed in an RDF/XML DOCTYPE, and their total size.
const MAX_XML_ENTITIES: usize = 256;
const MAX_XML_ENTITY_BYTES: usize = 64 * 1024;

/// Why a text is not given to the XML reader, and where.
#[derive(Debug)]
pub(super) struct Refused {
    /// The bytes of the start of the long tag or of the DOCTYPE.
    pub(super) at: std::ops::Range<usize>,
    pub(super) why: String,
}

/// The bytes the XML reader is given: `bytes`, with the quotes of each
/// single-quoted entity value that holds no `"` swapped for `"`. A swap
/// keeps every offset, so the positions the reader reports are those of
/// `bytes`.
pub(super) fn prepared(bytes: &[u8]) -> Result<Cow<'_, [u8]>, Refused> {
    check_xml_tags(bytes)?;
    let swaps = check_xml_entities(bytes)?;
    if swaps.is_empty() {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut copy = bytes.to_vec();
    for i in swaps {
        copy[i] = b'"';
    }
    Ok(Cow::Owned(copy))
}

/// Why `bytes` is refused, if it is ([`prepared`] without the copy).
pub(super) fn refusal(bytes: &[u8]) -> Option<String> {
    check_xml_tags(bytes)
        .and_then(|()| check_xml_entities(bytes))
        .err()
        .map(|r| r.why)
}

/// Refuse XML with a start tag longer than [`MAX_XML_TAG_BYTES`].
fn check_xml_tags(bytes: &[u8]) -> Result<(), Refused> {
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
                return Err(Refused {
                    at: start..start + 1,
                    why: format!("an XML tag is longer than {} KiB", MAX_XML_TAG_BYTES / 1024),
                });
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
fn check_xml_entities(bytes: &[u8]) -> Result<Vec<usize>, Refused> {
    let Some(dt) = find(bytes, b"<!DOCTYPE") else {
        return Ok(Vec::new());
    };
    let refuse = |why: &str| {
        Err(Refused {
            at: dt..dt + b"<!DOCTYPE".len(),
            why: format!("its DOCTYPE {why}; refused to avoid entity expansion"),
        })
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RDF: &str = "xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"";

    /// `<rdf:RDF>` around `body`, after a DOCTYPE declaring `entities`.
    fn with_doctype(entities: &str, body: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n{entities}\n]>\n<rdf:RDF {RDF}>\n{body}\n</rdf:RDF>\n"
        )
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
        let refused = check_xml_entities(laughs.as_bytes()).unwrap_err();
        // At the DOCTYPE.
        assert_eq!(&laughs[refused.at], "<!DOCTYPE");
        assert!(refused.why.ends_with("; refused to avoid entity expansion"));
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
    fn single_quoted_entity_values_are_swapped_in_place() {
        let xml = with_doctype(
            "<!ENTITY ex 'http://example.org/q#'>\n<!ENTITY dq \"http://example.org/d#\">",
            "<rdf:Description rdf:about=\"&ex;S\"><rdf:type rdf:resource=\"&dq;T\"/></rdf:Description>",
        );
        let at = |s: &str| xml.find(s).unwrap();
        let swaps = check_xml_entities(xml.as_bytes()).unwrap();
        assert_eq!(swaps, [at("'http"), at("#'>") + 1]);
        assert_eq!(refusal(xml.as_bytes()), None);
        let ready = prepared(xml.as_bytes()).unwrap();
        assert_eq!(ready.len(), xml.len());
        assert_eq!(
            String::from_utf8_lossy(&ready),
            xml.replace("'http://example.org/q#'", "\"http://example.org/q#\"")
        );
        // Nothing to swap: the text itself.
        let plain = with_doctype("<!ENTITY dq \"http://example.org/d#\">", "");
        assert!(matches!(prepared(plain.as_bytes()), Ok(Cow::Borrowed(_))));
        // A single-quoted value holding `"` is left to the parser, which
        // refuses it; so is a declaration after the DOCTYPE (a comment).
        let quoted = with_doctype("<!ENTITY e 'say \"hi\"'>", "");
        assert!(check_xml_entities(quoted.as_bytes()).unwrap().is_empty());
        let after = "<!DOCTYPE rdf:RDF [<!ENTITY a 'x'>]>\n<!-- <!ENTITY b 'y'> -->\n<rdf:RDF/>";
        let swaps = check_xml_entities(after.as_bytes()).unwrap();
        assert_eq!(swaps.len(), 2);
        assert!(swaps.iter().all(|&i| i < after.find("]>").unwrap()));
    }

    #[test]
    fn a_long_start_tag_is_refused_where_it_starts() {
        let mut long = "<?xml version='1.0'?><rdf:RDF ".to_string();
        long.push_str(&" ".repeat(MAX_XML_TAG_BYTES + 10));
        long.push_str("/>");
        let refused = prepared(long.as_bytes()).unwrap_err();
        assert_eq!(refused.at, 21..22);
        assert!(refused.why.contains("KiB"), "{}", refused.why);
        assert_eq!(refusal(long.as_bytes()), Some(refused.why));
        // Comments and CDATA are not tags.
        let mut fine = "<rdf:RDF><!--".to_string();
        fine.push_str(&"x".repeat(MAX_XML_TAG_BYTES + 10));
        fine.push_str("--></rdf:RDF>");
        assert!(prepared(fine.as_bytes()).is_ok());
    }
}
