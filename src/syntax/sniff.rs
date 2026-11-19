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

/// Whether `<` then `rest` starts XML rather than a Turtle `<iri>`. An
/// XML declaration, comment or DOCTYPE is XML. Then a Turtle IRI
/// reference followed by whitespace and another term (`<urn:s> <urn:p>`,
/// `<urn:s> a`, `<urn:s> ex:p`, a TriG `<urn:g> {`) is Turtle, even when
/// the IRI would also be an XML name ([`iri_then_term`]). Else XML is any other
/// `<?` or `<!`, or an XML name followed by whitespace, `/>`, or `>` and
/// another tag.
fn starts_xml(rest: &str) -> bool {
    if ["?xml", "!--", "!DOCTYPE"]
        .iter()
        .any(|p| rest.starts_with(p))
    {
        return true;
    }
    if iri_then_term(rest) {
        return false;
    }
    if rest.starts_with('?') || rest.starts_with('!') {
        return true;
    }
    let name = xml_name(rest);
    let after = &rest[name.len()..];
    !name.is_empty()
        && (after.starts_with(char::is_whitespace)
            || after.starts_with("/>")
            || after
                .strip_prefix('>')
                .is_some_and(|t| t.trim_start().starts_with('<')))
}

/// Whether `rest`, just after a `<`, is the rest of a Turtle IRI reference,
/// then whitespace and the start of another term: an IRI reference, `a`, a
/// prefixed name, or a TriG graph's `{`. An IRI that is an XML name without
/// a colon (`<Ontology>`, `<s>`) does not count: a root element in no
/// namespace, then a child, is well-formed XML, and a Turtle text seldom
/// starts with a relative IRI.
fn iri_then_term(rest: &str) -> bool {
    let Some(after) = iri_ref(rest) else {
        return false;
    };
    let iri = &rest[..rest.len() - after.len() - 1];
    if xml_name(iri) == iri && !iri.contains(':') {
        return false;
    }
    let next = after.trim_start();
    if next.len() == after.len() {
        return false;
    }
    if let Some(iri) = next.strip_prefix('<') {
        return iri_ref(iri).is_some();
    }
    if next.starts_with('{') {
        return true;
    }
    if let Some(a) = next.strip_prefix('a') {
        return a.starts_with(|c: char| c.is_whitespace() || "<[(\"".contains(c));
    }
    next.split_once(':')
        .is_some_and(|(prefix, _)| super::names::is_pn_prefix(prefix))
}

/// The text after the `>` of a Turtle IRI reference whose `<` comes just
/// before `rest`, if `rest` starts one: no space, `<`, `"`, `{`, `}`, `|`,
/// `^` or backquote before the `>`.
fn iri_ref(rest: &str) -> Option<&str> {
    let end = rest.find(|c: char| c <= ' ' || "<>\"{}|^`".contains(c))?;
    rest[end..].strip_prefix('>')
}

/// The XML name at the start of `s` (empty when there is none):
/// `NameStartChar NameChar*` of XML 1.0, which are Turtle's `PN_CHARS_U`
/// and `PN_CHARS` with `:` and `.`.
fn xml_name(s: &str) -> &str {
    use super::names::{is_pn_chars, is_pn_chars_u};
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, c)) if c == ':' || is_pn_chars_u(c) => {}
        _ => return "",
    }
    let end = chars
        .find(|&(_, c)| !(c == ':' || c == '.' || is_pn_chars(c)))
        .map_or(s.len(), |(i, _)| i);
    &s[..end]
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
/// OWL namespace (or in none), with no attribute in the RDF namespace, and
/// either an `ontologyIRI` attribute or a first child that is an OWL/XML
/// element ([`OWL_XML_CHILDREN`]) in the same namespace, or no child and no
/// prefix (`<Ontology xmlns="…owl#"/>`). RDF/XML otherwise:
/// an `owl:Ontology` node element is RDF/XML whatever namespaces it
/// declares (`rdf:about`, or no attribute and properties such as
/// `rdfs:label` or `owl:imports` as children), and so is a root that is not
/// found. RDF/XML is the generous answer: the parser then says what is
/// wrong.
fn xml_root(s: &str) -> Sniffed {
    let mut i = 0;
    while past_markup(s, &mut i) == Some(true) {}
    let Some(root) = s[i..].strip_prefix('<').map(start_tag) else {
        return Sniffed::RdfXml;
    };
    let in_rdf = |name: &str| {
        name.split_once(':').is_some_and(|(p, _)| {
            p != "xmlns" && root.ns(Some(p)).map_or(p == "rdf", |v| v == RDF_NS)
        })
    };
    let (prefix, local) = root.name();
    let owl = root.ns(prefix);
    if local != "Ontology"
        || !matches!(owl, None | Some(OWL_NS))
        || root.attrs.iter().any(|(n, _)| in_rdf(n))
    {
        return Sniffed::RdfXml;
    }
    if root.attrs.iter().any(|(n, _)| *n == "ontologyIRI") {
        return Sniffed::OwlXml;
    }
    // The first child element, past whitespace, comments and processing
    // instructions. With none, the name as written decides: OWL/XML writes
    // `Ontology` in the default namespace, RDF/XML `owl:Ontology`.
    let child = root.content.and_then(|content| {
        let mut j = 0;
        while past_markup(content, &mut j) == Some(true) {}
        content[j..]
            .strip_prefix('<')
            .map(start_tag)
            .filter(|t| !t.name.is_empty())
    });
    let Some(child) = child else {
        return if prefix.is_none() {
            Sniffed::OwlXml
        } else {
            Sniffed::RdfXml
        };
    };
    let (prefix, local) = child.name();
    let ns = child.ns(prefix).or_else(|| root.ns(prefix));
    if ns == owl && OWL_XML_CHILDREN.contains(&local) {
        Sniffed::OwlXml
    } else {
        Sniffed::RdfXml
    }
}

/// The elements of OWL/XML that an `Ontology` element holds: its
/// prefixes, imports and annotations, the declarations, every axiom, and
/// the SWRL rules Protégé writes there. An RDF/XML `owl:Ontology` holds
/// properties (`owl:imports`, `owl:versionInfo`), none of which is named so.
const OWL_XML_CHILDREN: &[&str] = &[
    "Prefix",
    "Import",
    "Annotation",
    "Declaration",
    "SubClassOf",
    "EquivalentClasses",
    "DisjointClasses",
    "DisjointUnion",
    "SubObjectPropertyOf",
    "EquivalentObjectProperties",
    "DisjointObjectProperties",
    "InverseObjectProperties",
    "ObjectPropertyDomain",
    "ObjectPropertyRange",
    "FunctionalObjectProperty",
    "InverseFunctionalObjectProperty",
    "ReflexiveObjectProperty",
    "IrreflexiveObjectProperty",
    "SymmetricObjectProperty",
    "AsymmetricObjectProperty",
    "TransitiveObjectProperty",
    "SubDataPropertyOf",
    "EquivalentDataProperties",
    "DisjointDataProperties",
    "DataPropertyDomain",
    "DataPropertyRange",
    "FunctionalDataProperty",
    "DatatypeDefinition",
    "HasKey",
    "SameIndividual",
    "DifferentIndividuals",
    "ClassAssertion",
    "ObjectPropertyAssertion",
    "NegativeObjectPropertyAssertion",
    "DataPropertyAssertion",
    "NegativeDataPropertyAssertion",
    "AnnotationAssertion",
    "SubAnnotationPropertyOf",
    "AnnotationPropertyDomain",
    "AnnotationPropertyRange",
    "DLSafeRule",
];

/// Past whitespace from offset `*i` of `s`, then past one XML declaration,
/// processing instruction, comment or DOCTYPE: `Some(true)` when one was
/// passed, `Some(false)` when `*i` is at the `<` of a tag, `None` when
/// neither is there or the markup does not end.
fn past_markup(s: &str, i: &mut usize) -> Option<bool> {
    let b = s.as_bytes();
    while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
        *i += 1;
    }
    if b.get(*i) != Some(&b'<') {
        return None;
    }
    let at = &s[*i..];
    let after = |end: &str| at.find(end).map(|p| *i + p + end.len());
    let next = if at.starts_with("<?") {
        after("?>")
    } else if at.starts_with("<!--") {
        after("-->")
    } else if at.starts_with("<!") {
        declaration_end(b, *i).map(|e| e + 1)
    } else {
        return Some(false);
    };
    *i = next?;
    Some(true)
}

/// A start tag as a sniff reads it.
struct Tag<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, &'a str)>,
    /// What follows the tag, when it ends in `>` (not `/>`).
    content: Option<&'a str>,
}

impl<'a> Tag<'a> {
    /// The prefix and the local part of the name.
    fn name(&self) -> (Option<&'a str>, &'a str) {
        match self.name.split_once(':') {
            Some((p, l)) => (Some(p), l),
            None => (None, self.name),
        }
    }

    /// The namespace this tag declares for `prefix` (`None`: the default).
    fn ns(&self, prefix: Option<&str>) -> Option<&'a str> {
        let decl = prefix.map_or("xmlns".to_string(), |p| format!("xmlns:{p}"));
        self.attrs.iter().find(|(n, _)| *n == decl).map(|(_, v)| *v)
    }
}

/// The start tag whose `<` comes just before `s`, as far as it can be read:
/// the attributes up to the first that is not `name="value"` or
/// `name='value'`.
fn start_tag(s: &str) -> Tag<'_> {
    let name = xml_name(s);
    let (attrs, rest) = attributes(&s[name.len()..]);
    Tag {
        name,
        attrs,
        content: rest.strip_prefix('>'),
    }
}

/// The attributes of a start tag from just after its name, up to its end
/// (or to the first that is not `name="value"` or `name='value'`), and the
/// text after them from the first that is not whitespace.
fn attributes(mut s: &str) -> (Vec<(&str, &str)>, &str) {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        let Some((name, rest)) = s.split_once('=') else {
            return (out, s);
        };
        let name = name.trim_end();
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            return (out, s);
        }
        let rest = rest.trim_start();
        let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return (out, s);
        };
        let Some((value, tail)) = rest[1..].split_once(q) else {
            return (out, s);
        };
        out.push((name, value));
        s = tail;
    }
}

/// The offset of the `>` that ends the `<!` declaration (a DOCTYPE) at
/// `start`, found as quick-xml finds it: the first `>` that closes every `<`
/// after the `<!`, quotes not considered.
fn declaration_end(bytes: &[u8], start: usize) -> Option<usize> {
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
            // N-Triples and Turtle whose first IRI is also an XML name.
            ("<urn:s> <urn:p> <urn:o> .\n", Turtle),
            ("<urn:s>\t<urn:p> \"o\" .", Turtle),
            ("<urn:a/> <urn:p> <urn:o> .", Turtle),
            ("<urn:s> a <urn:C> .", Turtle),
            ("<urn:s> ex:p 1 .", Turtle),
            ("<urn:s> :p 1 .", Turtle),
            ("<urn:g> { <urn:s> <urn:p> <urn:o> }", Turtle),
            (
                "<http://example.org/s> <http://example.org/p> <http://example.org/o> .",
                Turtle,
            ),
            // XML names with any name character: `·`, letters past ASCII,
            // combining marks.
            ("<e:C·D xmlns:e=\"urn:ex:\"/>", RdfXml),
            ("<é:Ü xmlns:é='urn:ex:'/>", RdfXml),
            ("<e:C\u{301}D xmlns:e='urn:ex:'/>", RdfXml),
            ("<e:a‿b xmlns:e='urn:ex:'>\n<e:p/></e:a‿b>", RdfXml),
            ("<a:b/>", RdfXml),
            ("<a:b>\n<!-- c --></a:b>", RdfXml),
            // A root in no namespace, then a child.
            ("<Ontology>\n  <Prefix/>", OwlXml),
            ("<catalog>\n  <entry/>\n</catalog>", RdfXml),
        ];
        for (text, want) in cases {
            assert_eq!(Sniffed::of(text), *want, "{text}");
        }
        // Past a DOCTYPE longer than the old 512-byte look.
        let entities: String = (0..40)
            .map(|i| format!("<!ENTITY e{i} \"http://example.org/entity/number/{i}#\">\n"))
            .collect();
        let long = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [\n{entities}]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n</Ontology>"
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

    /// An `owl:Ontology` root is RDF/XML unless it is OWL/XML's `Ontology`:
    /// with `ontologyIRI`, or holding OWL/XML elements. Declaring the RDF
    /// namespace says neither, as Protégé declares it in OWL/XML too.
    #[test]
    fn an_ontology_root_is_owl_xml_only_when_it_holds_owl_xml() {
        use Sniffed::*;
        const NS: &str = "xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"";
        let cases: &[(String, Sniffed)] = &[
            // RDF/XML: an anonymous ontology with properties.
            (
                format!("<owl:Ontology {NS}><rdfs:label>My ontology</rdfs:label></owl:Ontology>"),
                RdfXml,
            ),
            (
                format!("<owl:Ontology {NS}>\n  <!-- imports -->\n  <owl:imports rdf:resource=\"http://example.org/b\"/>\n</owl:Ontology>"),
                RdfXml,
            ),
            (
                format!("<owl:Ontology {NS}><rdf:type rdf:resource=\"http://example.org/T\"/></owl:Ontology>"),
                RdfXml,
            ),
            // Empty: the name as written decides.
            (format!("<owl:Ontology {NS}/>"), RdfXml),
            (format!("<owl:Ontology {NS}>\n</owl:Ontology>"), RdfXml),
            ("<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"/>".into(), OwlXml),
            ("<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"></Ontology>".into(), OwlXml),
            // `Prefix` in another namespace is a property.
            (
                format!("<owl:Ontology {NS} xmlns:e=\"http://example.org/\"><e:Prefix>x</e:Prefix></owl:Ontology>"),
                RdfXml,
            ),
            // OWL/XML as Protégé writes it, with `xmlns:rdf` declared, with
            // and without `ontologyIRI`.
            (
                "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"\n     xml:base=\"http://example.org/o\"\n     xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n     xmlns:xml=\"http://www.w3.org/XML/1998/namespace\"\n     xmlns:xsd=\"http://www.w3.org/2001/XMLSchema#\"\n     xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n     ontologyIRI=\"http://example.org/o\">\n    <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n</Ontology>\n".into(),
                OwlXml,
            ),
            (
                "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"\n     xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n    <!-- anonymous -->\n    <Prefix name=\"rdf\" IRI=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>\n    <Declaration><Class IRI=\"#A\"/></Declaration>\n</Ontology>\n".into(),
                OwlXml,
            ),
            (
                format!("<owl:Ontology {NS}>\n<owl:Declaration><owl:Class IRI=\"#A\"/></owl:Declaration></owl:Ontology>"),
                OwlXml,
            ),
            (
                "<Ontology xmlns='http://www.w3.org/2002/07/owl#'><SubClassOf/></Ontology>".into(),
                OwlXml,
            ),
        ];
        for (text, want) in cases {
            assert_eq!(Sniffed::of(text), *want, "{text}");
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
