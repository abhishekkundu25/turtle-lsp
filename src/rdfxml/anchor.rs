//! The triples `oxrdfxml` reads from an RDF/XML text, each with the span of
//! the XML tag that ends where it is read: the anchor of its terms.
//!
//! - A triple's `p_span`, `o_span` and statement are the span of that tag:
//!   the start tag of a node element for its type and its property
//!   attributes, the empty tag of an `rdf:resource` property, the end tag of
//!   a property element with text or a nested node, and the end tag of a
//!   `parseType="Collection"` for every cell of the list.
//! - One statement per tag. Statements are sorted and never overlap; the
//!   triples of one tag share its statement, whose subject is their IRI
//!   subject when they all have the same one.
//! - Every IRI of a triple gets a [`Form::Anchor`] reference at the tag: the
//!   predicate, an IRI object, a datatype written with `rdf:datatype`, the
//!   IRIs inside a triple term (`parseType="Triple"`, [`Role::Quoted`]),
//!   and the subject on its first triple ([`Role::Subject`], where
//!   definition goes) and on no later one.
//! - Blank nodes are numbered in the order they first appear, so two reads
//!   of one text give equal parses (`oxrdfxml` names its own at random).
//! - Reading stops at the first error, an `xml-syntax` error at the tag it
//!   ends at (or at the place it is found); the triples before it stay. A
//!   triple term nested more than [`MAX_NESTING`] deep is such an error,
//!   as in Turtle. A text the [`limits`](super::limits) refuse is not read
//!   at all.

use super::limits;
use crate::syntax::{
    BlankId, Direction, Form, Iri, MAX_NESTING, Node, Parsed, RDF, Ref, Role, Severity, Span,
    Statement, SyntaxError, Triple, XSD,
};
use oxrdf::{BaseDirection, NamedOrBlankNode, Term};
use rustc_hash::{FxHashMap, FxHashSet};

/// The diagnostic code of an RDF/XML text that is not well-formed XML, is
/// not RDF/XML, or asks more of the reader than [`limits`] allow.
pub(super) const XML_SYNTAX: &str = "xml-syntax";

/// Read `text` into `out`, resolving relative IRIs against `base` until an
/// `xml:base` replaces it.
pub(super) fn read(text: &str, base: Option<&str>, out: &mut Parsed) {
    let input = match limits::prepared(text.as_bytes()) {
        Ok(input) => input,
        Err(refused) => {
            let at = Span::new(refused.at.start as u32, refused.at.end as u32);
            out.errors.push(SyntaxError::new(
                at,
                refused.why,
                Severity::Error,
                XML_SYNTAX,
            ));
            return;
        }
    };
    let parser = oxrdfxml::RdfXmlParser::new();
    let parser = match base {
        Some(base) => parser.clone().with_base_iri(base).unwrap_or(parser),
        None => parser,
    };
    // quick-xml counts a byte order mark in its positions before a root
    // element but not before an XML declaration: it reads the text without
    // one, and its positions are moved past it.
    let skip = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut reader = parser.for_slice(&input[skip..]);
    let mut places = Places::new(text, out);
    // The triples read at one position, which one tag gave.
    let (mut run, mut run_end) = (Vec::new(), 0);
    loop {
        match reader.next() {
            Some(Ok(t)) => {
                let end = reader.buffer_position() as usize + skip;
                if end != run_end {
                    if !places.place(run_end, &mut run) {
                        return;
                    }
                    run_end = end;
                }
                run.push(t);
            }
            Some(Err(e)) => {
                if !places.place(run_end, &mut run) {
                    return;
                }
                let span = places.tag_ending_at(reader.buffer_position() as usize + skip);
                let error = SyntaxError::new(span, e.to_string(), Severity::Error, XML_SYNTAX);
                places.out.errors.push(error);
                return;
            }
            None => {
                places.place(run_end, &mut run);
                return;
            }
        }
    }
}

/// How deep triple terms nest in `o` (they nest only as objects).
fn nesting(mut o: &Term) -> usize {
    let mut depth = 0;
    while let Term::Triple(t) = o {
        depth += 1;
        o = &t.object;
    }
    depth
}

struct Places<'a> {
    text: &'a str,
    out: &'a mut Parsed,
    /// `oxrdfxml`'s blank node labels, numbered as they first appear.
    blanks: FxHashMap<String, BlankId>,
    /// The subjects whose first triple is placed.
    described: FxHashSet<Iri>,
    /// Where the last tag ends: no later span starts before it.
    floor: usize,
    /// The datatypes a literal has without `rdf:datatype`.
    implicit: [Iri; 3],
}

impl<'a> Places<'a> {
    fn new(text: &'a str, out: &'a mut Parsed) -> Places<'a> {
        let rdf = |local: &str| Iri::intern(&format!("{RDF}{local}"));
        Places {
            text,
            out,
            blanks: FxHashMap::default(),
            described: FxHashSet::default(),
            floor: 0,
            implicit: [
                Iri::intern(&format!("{XSD}string")),
                rdf("langString"),
                rdf("dirLangString"),
            ],
        }
    }

    /// The span of the tag that ends at byte `end` of the text, from the
    /// last `<` before it; an empty span at `end` when no `>` ends there. It
    /// starts no earlier than the last tag ends.
    fn tag_ending_at(&mut self, end: usize) -> Span {
        let bytes = self.text.as_bytes();
        let mut end = end.clamp(self.floor, bytes.len());
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        let start = if end > self.floor && bytes[end - 1] == b'>' {
            bytes[self.floor..end - 1]
                .iter()
                .rposition(|&b| b == b'<')
                .map_or(end - 1, |i| self.floor + i)
        } else {
            end
        };
        self.floor = end;
        Span::new(start as u32, end as u32)
    }

    /// Place the triples of `run`, read at byte `end`, at the tag that ends
    /// there, in the order the tag gave them (`oxrdfxml` hands them out
    /// last first). False, with an error, at a triple term nested too deep:
    /// reading stops there.
    fn place(&mut self, end: usize, run: &mut Vec<oxrdf::Triple>) -> bool {
        if run.is_empty() {
            return true;
        }
        let span = self.tag_ending_at(end);
        let statement = self.out.statements.len() as u32;
        let mut subject: Option<Option<Iri>> = None;
        let mut placed = true;
        for t in run.drain(..).rev() {
            if nesting(&t.object) > MAX_NESTING as usize {
                let why = format!("Triple terms nested more than {MAX_NESTING} levels deep");
                let error = SyntaxError::new(span, why, Severity::Error, XML_SYNTAX);
                self.out.errors.push(error);
                placed = false;
                break;
            }
            if subject.is_none() {
                self.out.statements.push(Statement {
                    span,
                    subject: None,
                    graph: None,
                });
            }
            let s = self.subject(t.subject, span);
            let iri = match s {
                Node::Iri(i) => Some(i),
                _ => None,
            };
            subject = Some(match subject {
                None => iri,
                Some(same) => same.filter(|_| same == iri),
            });
            let p = Iri::intern(t.predicate.as_str());
            self.reference(span, p, Role::Predicate);
            let o = self.object(t.object, span, Role::Object);
            self.out.triples.push(Triple {
                s,
                p,
                o,
                p_span: span,
                o_span: span,
                statement,
                graph: None,
                unresolved: false,
            });
        }
        if let Some(st) = self.out.statements.get_mut(statement as usize) {
            st.subject = subject.flatten().map(|i| (i, span));
        }
        placed
    }

    fn reference(&mut self, span: Span, iri: Iri, role: Role) {
        self.out.refs.push(Ref {
            span,
            iri,
            role,
            form: Form::Anchor,
        });
    }

    fn blank(&mut self, label: &str) -> Node {
        let next = BlankId(self.blanks.len() as u32);
        let id = *self.blanks.entry(label.to_string()).or_insert(next);
        Node::Blank(id)
    }

    fn subject(&mut self, s: NamedOrBlankNode, span: Span) -> Node {
        match s {
            NamedOrBlankNode::NamedNode(n) => {
                let iri = Iri::intern(n.as_str());
                if self.described.insert(iri) {
                    self.reference(span, iri, Role::Subject);
                }
                Node::Iri(iri)
            }
            NamedOrBlankNode::BlankNode(b) => self.blank(b.as_str()),
        }
    }

    /// An object, or (with `role` [`Role::Quoted`]) a term of a triple term.
    fn object(&mut self, o: Term, span: Span, role: Role) -> Node {
        match o {
            Term::NamedNode(n) => {
                let iri = Iri::intern(n.as_str());
                self.reference(span, iri, role);
                Node::Iri(iri)
            }
            Term::BlankNode(b) => self.blank(b.as_str()),
            Term::Literal(l) => {
                let datatype = Iri::intern(l.datatype().as_str());
                if !self.implicit.contains(&datatype) {
                    self.reference(span, datatype, Role::Datatype);
                }
                let lang = l.language().map(|t| Iri::intern(&t.to_ascii_lowercase()));
                let dir = l.direction().map(|d| match d {
                    BaseDirection::Ltr => Direction::Ltr,
                    BaseDirection::Rtl => Direction::Rtl,
                });
                Node::Literal {
                    value: l.value().into(),
                    datatype,
                    lang,
                    dir: dir.filter(|_| lang.is_some()),
                }
            }
            Term::Triple(t) => {
                let t = *t;
                let s = match t.subject {
                    NamedOrBlankNode::NamedNode(n) => {
                        let iri = Iri::intern(n.as_str());
                        self.reference(span, iri, Role::Quoted);
                        Node::Iri(iri)
                    }
                    NamedOrBlankNode::BlankNode(b) => self.blank(b.as_str()),
                };
                let p = Iri::intern(t.predicate.as_str());
                self.reference(span, p, Role::Quoted);
                let o = self.object(t.object, span, Role::Quoted);
                Node::Triple(Box::new((s, p, o)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Syntax;

    const HEAD: &str = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n    xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n    xmlns:owl=\"http://www.w3.org/2002/07/owl#\"\n    xmlns:ex=\"http://example.org/\">\n";

    fn doc(body: &str) -> String {
        format!("{HEAD}{body}</rdf:RDF>\n")
    }

    fn read_text(text: &str) -> Parsed {
        let mut out = Parsed {
            syntax: Syntax::RdfXml,
            ..Parsed::default()
        };
        read(text, Some("http://example.org/doc"), &mut out);
        out
    }

    fn ex(local: &str) -> Node {
        Node::Iri(Iri::intern(&format!("http://example.org/{local}")))
    }

    /// The text of each triple's span, with its predicate's local name.
    fn placed<'t>(text: &'t str, p: &Parsed) -> Vec<(&'t str, &'static str)> {
        p.triples
            .iter()
            .map(|t| {
                let at = &text[t.p_span.start as usize..t.p_span.end as usize];
                let local = t.p.as_str().rsplit(['#', '/']).next().unwrap();
                (at, local)
            })
            .collect()
    }

    /// Statements are sorted, apart, and hold the triples that point at
    /// them, which are in statement order.
    fn well_placed(p: &Parsed) {
        for w in p.statements.windows(2) {
            assert!(w[0].span.end <= w[1].span.start, "{:?}", p.statements);
        }
        for w in p.triples.windows(2) {
            assert!(w[0].statement <= w[1].statement);
        }
        for t in &p.triples {
            let st = &p.statements[t.statement as usize];
            assert_eq!((t.p_span, t.o_span), (st.span, st.span));
        }
        for r in &p.refs {
            assert_eq!(r.form, Form::Anchor);
            assert!(p.statements.iter().any(|st| st.span == r.span), "{r:?}");
        }
    }

    #[test]
    fn a_flat_element_places_each_triple_at_its_own_tag() {
        let text = doc(concat!(
            "  <owl:Class rdf:about=\"http://example.org/Dog\">\n",
            "    <rdfs:subClassOf rdf:resource=\"http://example.org/Animal\"/>\n",
            "    <rdfs:label xml:lang=\"EN\">Dog</rdfs:label>\n",
            "    <ex:legs rdf:datatype=\"http://www.w3.org/2001/XMLSchema#integer\">4</ex:legs>\n",
            "  </owl:Class>\n",
        ));
        let p = read_text(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        well_placed(&p);
        assert_eq!(
            placed(&text, &p),
            [
                ("<owl:Class rdf:about=\"http://example.org/Dog\">", "type"),
                (
                    "<rdfs:subClassOf rdf:resource=\"http://example.org/Animal\"/>",
                    "subClassOf"
                ),
                ("</rdfs:label>", "label"),
                ("</ex:legs>", "legs"),
            ]
        );
        assert!(p.triples.iter().all(|t| t.s == ex("Dog")));
        assert_eq!(
            p.statements[0].subject,
            Some((Iri::intern("http://example.org/Dog"), p.triples[0].p_span))
        );
        // The subject is defined at its first triple, and only there.
        let dog = Iri::intern("http://example.org/Dog");
        let defs: Vec<_> = p.refs.iter().filter(|r| r.iri == dog).collect();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            (defs[0].role, defs[0].span),
            (Role::Subject, p.triples[0].p_span)
        );
        // Literals keep their language (lowercased) and datatype; only a
        // written datatype is a reference.
        let Node::Literal { lang, datatype, .. } = &p.triples[2].o else {
            panic!("{:?}", p.triples[2].o)
        };
        assert_eq!(lang.map(Iri::as_str), Some("en"));
        assert_eq!(datatype.as_str(), format!("{RDF}langString"));
        let datatypes: Vec<_> = p.refs.iter().filter(|r| r.role == Role::Datatype).collect();
        assert_eq!(datatypes.len(), 1);
        assert_eq!(datatypes[0].iri.as_str(), format!("{XSD}integer"));
        assert_eq!(datatypes[0].span, p.triples[3].p_span);
    }

    #[test]
    fn parse_type_resource_is_placed_at_its_start_tag_and_its_properties_at_theirs() {
        let text = doc(concat!(
            "  <rdf:Description rdf:about=\"http://example.org/a\">\n",
            "    <ex:address rdf:parseType=\"Resource\">\n",
            "      <ex:city>Paris</ex:city>\n",
            "    </ex:address>\n",
            "  </rdf:Description>\n",
        ));
        let p = read_text(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        well_placed(&p);
        assert_eq!(
            placed(&text, &p),
            [
                ("<ex:address rdf:parseType=\"Resource\">", "address"),
                ("</ex:city>", "city"),
            ]
        );
        assert_eq!(p.triples[0].o, Node::Blank(BlankId(0)));
        assert_eq!(p.triples[1].s, Node::Blank(BlankId(0)));
    }

    #[test]
    fn a_nested_blank_node_is_placed_at_the_end_tag_of_its_property() {
        let text = doc(concat!(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n",
            "    <rdfs:subClassOf>\n",
            "      <owl:Restriction>\n",
            "        <owl:onProperty rdf:resource=\"http://example.org/p\"/>\n",
            "        <owl:someValuesFrom rdf:resource=\"http://example.org/B\"/>\n",
            "      </owl:Restriction>\n",
            "    </rdfs:subClassOf>\n",
            "  </owl:Class>\n",
        ));
        let p = read_text(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        well_placed(&p);
        assert_eq!(
            placed(&text, &p),
            [
                ("<owl:Class rdf:about=\"http://example.org/A\">", "type"),
                ("<owl:Restriction>", "type"),
                (
                    "<owl:onProperty rdf:resource=\"http://example.org/p\"/>",
                    "onProperty"
                ),
                (
                    "<owl:someValuesFrom rdf:resource=\"http://example.org/B\"/>",
                    "someValuesFrom"
                ),
                ("</rdfs:subClassOf>", "subClassOf"),
            ]
        );
        assert_eq!(p.triples[4].s, ex("A"));
        assert_eq!(p.triples[4].o, Node::Blank(BlankId(0)));
        // A blank subject is no statement's subject.
        assert_eq!(p.statements[1].subject, None);
    }

    #[test]
    fn the_cells_of_a_collection_share_the_end_tag_of_their_property() {
        let text = doc(concat!(
            "  <owl:Class rdf:about=\"http://example.org/U\">\n",
            "    <owl:unionOf rdf:parseType=\"Collection\">\n",
            "      <rdf:Description rdf:about=\"http://example.org/X\"/>\n",
            "      <rdf:Description rdf:about=\"http://example.org/Y\"/>\n",
            "    </owl:unionOf>\n",
            "  </owl:Class>\n",
        ));
        let p = read_text(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        well_placed(&p);
        let at = placed(&text, &p);
        assert_eq!(
            at[0],
            ("<owl:Class rdf:about=\"http://example.org/U\">", "type")
        );
        assert_eq!(at.len(), 6, "{at:?}");
        assert!(
            at[1..].iter().all(|(tag, _)| *tag == "</owl:unionOf>"),
            "{at:?}"
        );
        // One statement for the five, whose subjects differ.
        assert_eq!(p.statements.len(), 2);
        assert_eq!(p.statements[1].subject, None);
        let mut locals: Vec<_> = at[1..].iter().map(|(_, l)| *l).collect();
        locals.sort();
        assert_eq!(locals, ["first", "first", "rest", "rest", "unionOf"]);
        // The list's cells, numbered as they first appear.
        let blanks: FxHashSet<_> = p
            .triples
            .iter()
            .flat_map(|t| [&t.s, &t.o])
            .filter_map(|n| match n {
                Node::Blank(b) => Some(b.0),
                _ => None,
            })
            .collect();
        assert_eq!(blanks, [0, 1].into_iter().collect());
    }

    #[test]
    fn two_reads_of_one_text_are_equal() {
        let head = HEAD.replace("<rdf:RDF ", "<rdf:RDF rdf:version=\"1.2\" ");
        let text = format!(
            "{head}{}</rdf:RDF>\n",
            concat!(
                "  <ex:Thing rdf:about=\"http://example.org/t\">\n",
                "    <ex:part><ex:Part><ex:name>x</ex:name></ex:Part></ex:part>\n",
                "    <ex:list rdf:parseType=\"Collection\"><ex:A/><ex:B rdf:nodeID=\"n\"/></ex:list>\n",
                "    <ex:p rdf:parseType=\"Triple\"><rdf:Description rdf:about=\"http://example.org/s\"><ex:q rdf:resource=\"http://example.org/o\"/></rdf:Description></ex:p>\n",
                "  </ex:Thing>\n",
            )
        );
        let first = read_text(&text);
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert!(first.triples.len() > 8);
        assert!(first.triples.iter().any(|t| matches!(t.o, Node::Triple(_))));
        assert!(first.refs.iter().any(|r| r.role == Role::Quoted));
        well_placed(&first);
        assert_eq!(first, read_text(&text));
    }

    #[test]
    fn an_error_keeps_the_triples_before_it() {
        let text = doc(concat!(
            "  <ex:A rdf:about=\"http://example.org/a\"/>\n",
            "  <ex:B rdf:about=\"http://example.org/b\"></ex:C>\n",
            "  <ex:D rdf:about=\"http://example.org/d\"/>\n",
        ));
        let p = read_text(&text);
        well_placed(&p);
        let subjects: Vec<_> = p.triples.iter().map(|t| t.s.clone()).collect();
        assert_eq!(subjects, [ex("a"), ex("b")]);
        assert_eq!(p.errors.len(), 1);
        let e = &p.errors[0];
        assert_eq!((e.code, e.severity), (XML_SYNTAX, Severity::Error));
        // At the end tag that closes nothing open.
        assert_eq!(&text[e.span.start as usize..e.span.end as usize], "</ex:C>");
        // An error before any triple, at the end of the text.
        let p = read_text("<rdf:RDF");
        assert!(p.triples.is_empty());
        assert_eq!(p.errors.len(), 1);
        assert_eq!(p.errors[0].span, Span::new(8, 8));
        // A text cut short with elements open is no error to `oxrdfxml`:
        // what it read stays.
        let cut = &text[..text.find("<ex:D").unwrap()];
        let p = read_text(cut.replace("</ex:C>", "").as_str());
        assert_eq!(p.triples.len(), 2);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
    }

    #[test]
    fn what_the_limits_refuse_is_not_read() {
        let text = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [<!ENTITY a \"&b;\">]>\n{}",
            doc("  <ex:A rdf:about=\"http://example.org/a\"/>\n")
        );
        let p = read_text(&text);
        assert!(p.triples.is_empty());
        let e = &p.errors[0];
        assert_eq!(e.code, XML_SYNTAX);
        assert_eq!(
            &text[e.span.start as usize..e.span.end as usize],
            "<!DOCTYPE"
        );
        assert!(e.message().ends_with("refused to avoid entity expansion"));
        // A single-quoted entity is read, at the offsets of the text.
        let text = format!(
            "<!DOCTYPE rdf:RDF [<!ENTITY e 'http://example.org/'>]>\n{}",
            doc("  <ex:A rdf:about=\"&e;a\"/>\n")
        );
        let p = read_text(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(placed(&text, &p), [("<ex:A rdf:about=\"&e;a\"/>", "type")]);
    }

    #[test]
    fn a_byte_order_mark_keeps_the_offsets_of_the_text() {
        for decl in ["", "<?xml version=\"1.0\"?>\n"] {
            let text = format!(
                "\u{feff}{decl}{}",
                doc("  <ex:A rdf:about=\"http://example.org/a\"/>\n")
            );
            let p = read_text(&text);
            assert!(p.errors.is_empty(), "{:?}", p.errors);
            assert_eq!(
                placed(&text, &p),
                [("<ex:A rdf:about=\"http://example.org/a\"/>", "type")]
            );
            let more = format!("{text}</oops>");
            let e = &read_text(&more).errors[0];
            assert_eq!(
                &more[e.span.start as usize..e.span.end as usize],
                "</oops>",
                "{decl:?}"
            );
        }
    }

    #[test]
    fn triple_terms_nested_too_deep_stop_the_reading_on_a_small_stack() {
        let nest = |n: usize| {
            let head = HEAD.replace("<rdf:RDF ", "<rdf:RDF rdf:version=\"1.2\" ");
            let open = "<ex:p rdf:parseType=\"Triple\"><rdf:Description rdf:about=\"http://example.org/s\">";
            let close = "</rdf:Description></ex:p>";
            format!(
                "{head}  <ex:A rdf:about=\"http://example.org/a\"/>\n  <rdf:Description rdf:about=\"http://example.org/b\">{}<ex:q rdf:resource=\"http://example.org/o\"/>{}</rdf:Description>\n</rdf:RDF>\n",
                open.repeat(n),
                close.repeat(n)
            )
        };
        let read = |text: String| {
            std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(move || {
                    let p = read_text(&text);
                    well_placed(&p);
                    let codes: Vec<_> = p.errors.iter().map(|e| e.code).collect();
                    (p.triples.len(), codes)
                })
                .unwrap()
                .join()
                .unwrap()
        };
        let most = MAX_NESTING as usize;
        assert_eq!(read(nest(most)), (2, vec![]));
        // The triple before it stays.
        assert_eq!(read(nest(most + 1)), (1, vec![XML_SYNTAX]));
        assert_eq!(read(nest(20_000)), (1, vec![XML_SYNTAX]));
    }

    #[test]
    fn a_base_direction_is_kept() {
        let text = concat!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n",
            "    xmlns:its=\"http://www.w3.org/2005/11/its\" xmlns:ex=\"http://example.org/\"\n",
            "    rdf:version=\"1.2\" its:version=\"2.0\">\n",
            "  <rdf:Description rdf:about=\"http://example.org/a\">\n",
            "    <ex:p xml:lang=\"ar\" its:dir=\"rtl\">x</ex:p>\n",
            "  </rdf:Description>\n",
            "</rdf:RDF>\n",
        );
        let p = read_text(text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let Node::Literal { lang, dir, .. } = &p.triples[0].o else {
            panic!("{:?}", p.triples)
        };
        assert_eq!(
            (lang.map(Iri::as_str), *dir),
            (Some("ar"), Some(Direction::Rtl))
        );
    }

    /// A Protégé-style text of `classes` classes, each with a label, a
    /// comment, a superclass and a restriction: about 1 MB for 2,150.
    fn protege(classes: usize) -> String {
        let mut text = String::from(concat!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n",
            "    <!ENTITY owl \"http://www.w3.org/2002/07/owl#\" >\n",
            "    <!ENTITY ex \"http://example.org/onto#\" >\n]>\n",
        ));
        text.push_str(&HEAD.replace(
            "<rdf:RDF ",
            "<rdf:RDF xml:base=\"http://example.org/onto\" ",
        ));
        for i in 0..classes {
            text.push_str(&format!(
                concat!(
                    "    <owl:Class rdf:about=\"&ex;C{i}\">\n",
                    "        <rdfs:subClassOf rdf:resource=\"&ex;C{up}\"/>\n",
                    "        <rdfs:subClassOf>\n",
                    "            <owl:Restriction>\n",
                    "                <owl:onProperty rdf:resource=\"&ex;partOf\"/>\n",
                    "                <owl:someValuesFrom rdf:resource=\"&ex;C{up}\"/>\n",
                    "            </owl:Restriction>\n",
                    "        </rdfs:subClassOf>\n",
                    "        <rdfs:label xml:lang=\"en\">class number {i}</rdfs:label>\n",
                    "        <rdfs:comment>A class made to measure how long reading takes.</rdfs:comment>\n",
                    "    </owl:Class>\n",
                ),
                i = i,
                up = i / 2,
            ));
        }
        text.push_str("</rdf:RDF>\n");
        text
    }

    #[test]
    fn timing_rdfxml_anchor_reads_a_megabyte_in_time() {
        let text = protege(2_150);
        assert!(
            (1_000_000..1_200_000).contains(&text.len()),
            "{}",
            text.len()
        );
        let start = std::time::Instant::now();
        let p = read_text(&text);
        let took = start.elapsed();
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.triples.len(), 2_150 * 8);
        well_placed(&p);
        // About 9.5 ms on the machine this was written on. The budget leaves
        // room for CI, which builds without fat LTO on slower runners.
        if cfg!(not(debug_assertions)) {
            assert!(took < std::time::Duration::from_millis(100), "{took:?}");
        }
    }
}
