//! What a text is written in, from how it starts: the RDF syntaxes
//! turtle-lsp reads, and the OWL syntaxes it names but does not read. The
//! import loader and the RDF/XML frontend both ask.

/// How far a sniff looks: past an XML declaration, comments and a DOCTYPE
/// with entities to the root element.
pub(crate) const SNIFF_BYTES: usize = 64 * 1024;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";

/// What a text is written in, as its start tells: RDF/XML, OWL/XML,
/// Functional, Manchester, JSON-LD, else Turtle (N-Triples is Turtle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sniffed {
    Turtle,
    JsonLd,
    RdfXml,
    OwlXml,
    Functional,
    Manchester,
}

impl Sniffed {
    /// Read the start of `text`: past a BOM and whitespace; for XML, past
    /// the declaration, comments and the DOCTYPE to the root element and its
    /// namespaces; else past `#` comment lines. Looks at most at the first
    /// [`SNIFF_BYTES`].
    pub(crate) fn of(text: &str) -> Sniffed {
        let mut end = text.len().min(SNIFF_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let s = text[..end].trim_start_matches('\u{feff}').trim_start();
        if let Some(rest) = s.strip_prefix('<') {
            return if starts_xml(rest) {
                xml_root(s)
            } else {
                Sniffed::Turtle
            };
        }
        // JSON: an object that starts with a key (or is empty), or an array of objects.
        if let Some(rest) = s.strip_prefix('{') {
            let rest = rest.trim_start();
            if rest.starts_with('"') || rest.starts_with('}') {
                return Sniffed::JsonLd;
            }
        }
        if let Some(rest) = s.strip_prefix('[')
            && rest.trim_start().starts_with('{')
        {
            return Sniffed::JsonLd;
        }
        // The first word of an OWL text: `Prefix` or `Ontology`, then `(`
        // (Functional) or `:` (Manchester). Turtle never starts so: its
        // `PREFIX` is followed by a prefix name, and `Prefix:` as a name
        // would need a declaration before it.
        let s = without_comments(s);
        let Some(rest) = s
            .strip_prefix("Prefix")
            .or_else(|| s.strip_prefix("Ontology"))
        else {
            return Sniffed::Turtle;
        };
        if rest.starts_with(':') {
            Sniffed::Manchester
        } else if rest.trim_start().starts_with('(') {
            Sniffed::Functional
        } else {
            Sniffed::Turtle
        }
    }

    /// What a text in an OWL syntax holds and how to make RDF of it, as
    /// the end of a sentence ("holds OWL/XML, which turtle-lsp does not
    /// read. In Protégé: …"); `None` for the RDF syntaxes.
    pub(crate) fn unread(self) -> Option<String> {
        let name = match self {
            Sniffed::Turtle | Sniffed::JsonLd | Sniffed::RdfXml => return None,
            Sniffed::OwlXml => "OWL/XML",
            Sniffed::Functional => "OWL Functional syntax",
            Sniffed::Manchester => "OWL Manchester syntax",
        };
        Some(format!(
            "holds {name}, which turtle-lsp does not read. In Protégé: File > Save as > RDF/XML or Turtle"
        ))
    }
}

/// Whether `<` then `rest` starts XML rather than a Turtle `<iri>`: a
/// declaration, comment or DOCTYPE, or a name followed by an attribute, an
/// end of tag and another tag, or `/>`.
fn starts_xml(rest: &str) -> bool {
    if rest.starts_with('?') || rest.starts_with('!') {
        return true;
    }
    let name_len = rest
        .find(|c: char| !(c.is_alphanumeric() || "_-.:".contains(c)))
        .unwrap_or(rest.len());
    let after = &rest[name_len..];
    name_len > 0
        && (after.starts_with(char::is_whitespace)
            || after.starts_with("/>")
            || after
                .strip_prefix('>')
                .is_some_and(|t| t.trim_start().starts_with('<')))
}

/// `s` past `#` comment lines and the whitespace around them.
fn without_comments(mut s: &str) -> &str {
    while let Some(comment) = s.strip_prefix('#') {
        s = comment
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_start();
    }
    s
}

/// OWL/XML when the root element of the XML text `s` is `Ontology` in the
/// OWL namespace (or in none) and has no attribute in the RDF namespace (an
/// `owl:Ontology rdf:about` root is RDF/XML); RDF/XML otherwise, also when
/// the root is not found. RDF/XML is the generous answer: the parser then
/// says what is wrong.
fn xml_root(s: &str) -> Sniffed {
    let b = s.as_bytes();
    let mut i = 0;
    let after = |i: usize, end: &str| s[i..].find(end).map(|p| i + p + end.len());
    let root = loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) != Some(&b'<') {
            return Sniffed::RdfXml;
        }
        let next = if s[i..].starts_with("<?") {
            after(i, "?>")
        } else if s[i..].starts_with("<!--") {
            after(i, "-->")
        } else if s[i..].starts_with("<!") {
            declaration_end(b, i).map(|e| e + 1)
        } else {
            break &s[i + 1..];
        };
        match next {
            Some(n) => i = n,
            None => return Sniffed::RdfXml,
        }
    };
    let name_end = root
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(root.len());
    let (prefix, local) = match root[..name_end].split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, &root[..name_end]),
    };
    let attrs = attributes(&root[name_end..]);
    let ns = |prefix: Option<&str>| {
        let decl = prefix.map_or("xmlns".to_string(), |p| format!("xmlns:{p}"));
        attrs.iter().find(|(n, _)| *n == decl).map(|(_, v)| *v)
    };
    let in_rdf = |name: &str| {
        name.split_once(':')
            .is_some_and(|(p, _)| p != "xmlns" && ns(Some(p)).map_or(p == "rdf", |v| v == RDF_NS))
    };
    let owl = matches!(ns(prefix), None | Some(OWL_NS));
    if local == "Ontology" && owl && !attrs.iter().any(|(n, _)| in_rdf(n)) {
        Sniffed::OwlXml
    } else {
        Sniffed::RdfXml
    }
}

/// The attributes of a start tag from just after its name, up to its end
/// (or to the first that is not `name="value"` or `name='value'`).
fn attributes(mut s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        let Some((name, rest)) = s.split_once('=') else {
            return out;
        };
        let name = name.trim_end();
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            return out;
        }
        let rest = rest.trim_start();
        let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return out;
        };
        let Some((value, tail)) = rest[1..].split_once(q) else {
            return out;
        };
        out.push((name, value));
        s = tail;
    }
}

/// The offset of the `>` that ends the `<!` declaration (a DOCTYPE) at
/// `start`, found as quick-xml finds it: the first `>` that closes every `<`
/// after the `<!`, quotes not considered.
pub(crate) fn declaration_end(bytes: &[u8], start: usize) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const PROTEGE_HEAD: &str = "\u{feff}<?xml version=\"1.0\"?>\n<!-- written by hand -->\n<!DOCTYPE Ontology [\n    <!ENTITY xsd \"http://www.w3.org/2001/XMLSchema#\" >\n    <!ENTITY rdf 'http://www.w3.org/1999/02/22-rdf-syntax-ns#' >\n]>\n\n";

    #[test]
    fn each_format_is_told_apart_past_a_bom_comments_and_a_doctype() {
        use Sniffed::*;
        let owx = format!(
            "{PROTEGE_HEAD}<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"\n     xml:base=\"http://example.org/o\"\n     xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n     ontologyIRI=\"http://example.org/o\">\n    <Declaration><Class IRI=\"#A\"/></Declaration>\n</Ontology>\n"
        );
        let rdf = format!(
            "{PROTEGE_HEAD}<rdf:RDF xmlns=\"http://example.org/o#\"\n     xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n</rdf:RDF>\n"
        );
        let cases: &[(&str, Sniffed)] = &[
            (&owx, OwlXml),
            (&rdf, RdfXml),
            // OWL/XML with a prefix for the OWL namespace.
            (
                "<owl:Ontology xmlns:owl='http://www.w3.org/2002/07/owl#' ontologyIRI='http://example.org/o'/>",
                OwlXml,
            ),
            // A node element as root is RDF/XML: an `rdf:` attribute says so.
            (
                "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" rdf:about=\"http://example.org/o\"/>",
                RdfXml,
            ),
            (
                "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" r:about=\"\"/>",
                RdfXml,
            ),
            // `Ontology` in another namespace, and any other root: RDF/XML.
            ("<Ontology xmlns=\"http://example.org/ns#\"/>", RdfXml),
            ("<?xml version='1.0'?>\n<catalog xmlns='urn:x'/>", RdfXml),
            (
                "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>)",
                Functional,
            ),
            (
                "# a comment\n\n  Ontology (<http://example.org/o>\n)",
                Functional,
            ),
            (
                "\u{feff}Prefix: ex: <http://example.org/o#>\nOntology: <http://example.org/o>",
                Manchester,
            ),
            ("Ontology: <http://example.org/o>\nClass: ex:A", Manchester),
            // SPARQL-style Turtle prefixes, in any case.
            ("PREFIX ex: <http://example.org/o#>\nex:a a ex:B .", Turtle),
            ("Prefix : <http://example.org/o#>\n:a a :B .", Turtle),
            ("prefix ex: <http://example.org/o#>", Turtle),
            ("# Prefix(\nex:a a ex:B .", Turtle),
            ("Ontology", Turtle),
        ];
        for (text, want) in cases {
            assert_eq!(Sniffed::of(text), *want, "{text}");
        }
        // Past a DOCTYPE longer than the old 512-byte look.
        let entities: String = (0..40)
            .map(|i| format!("<!ENTITY e{i} \"http://example.org/entity/number/{i}#\">\n"))
            .collect();
        let long = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [\n{entities}]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"/>"
        );
        assert!(long.find("<Ontology").unwrap() > 1024);
        assert_eq!(Sniffed::of(&long), OwlXml);
        let unread = |text: &str| Sniffed::of(text).unread().unwrap();
        assert!(unread(&long).starts_with("holds OWL/XML, "));
        let e = unread("Prefix(:=<http://e/>)");
        assert!(e.contains("OWL Functional syntax"), "{e}");
        let e = unread("Prefix: : <http://e/>");
        assert!(e.contains("OWL Manchester syntax"), "{e}");
        for rdf in [Turtle, JsonLd, RdfXml] {
            assert_eq!(rdf.unread(), None);
        }
    }

    #[test]
    fn a_sniff_survives_what_never_ends() {
        for text in [
            "<!-- never closed <Ontology xmlns='http://www.w3.org/2002/07/owl#'/>",
            "<?xml version='1.0'",
            "<!DOCTYPE x [ <!ENTITY a 'b'> ",
            "<Ontology xmlns='http://www.w3.org/2002/07/owl#",
            "<Ontology a=\"1\" b c='2'>",
            "<Ontology",
            "<",
            "<!",
            "<?",
            "# only a comment",
            "#",
            "\u{feff}",
        ] {
            let _ = Sniffed::of(text);
        }
        // Cut inside a character, at the edge of what a sniff looks at.
        let mut big = "#".repeat(SNIFF_BYTES - 1);
        big.push('é');
        assert_eq!(Sniffed::of(&big), Sniffed::Turtle);
    }
}
