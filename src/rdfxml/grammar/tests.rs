use super::*;
use oxrdf::graph::CanonicalizationAlgorithm;
use oxrdf::{BaseDirection, BlankNode, Graph, Literal, NamedNode, NamedOrBlankNode, Term};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

include!("../../../tests/support/corpus.rs");

const BASE: &str = "http://example.org/base";

fn term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => BlankNode::new_unchecked(format!("b{}", b.0)).into(),
        Node::Literal {
            value,
            datatype,
            lang,
            dir,
        } => match (lang, dir) {
            (Some(l), Some(d)) => Literal::new_directional_language_tagged_literal_unchecked(
                &**value,
                l.as_str(),
                match d {
                    Direction::Ltr => BaseDirection::Ltr,
                    Direction::Rtl => BaseDirection::Rtl,
                },
            )
            .into(),
            (Some(l), None) => {
                Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into()
            }
            _ => Literal::new_typed_literal(&**value, NamedNode::new_unchecked(datatype.as_str()))
                .into(),
        },
        Node::Triple(t) => oxrdf::Triple::new(
            subject(&t.0),
            NamedNode::new_unchecked(t.1.as_str()),
            term(&t.2),
        )
        .into(),
    }
}

fn subject(n: &Node) -> NamedOrBlankNode {
    match term(n) {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        t => panic!("not a subject: {t}"),
    }
}

fn canonical(triples: impl IntoIterator<Item = oxrdf::Triple>) -> Graph {
    let mut g = Graph::new();
    for t in triples {
        g.insert(&t);
    }
    g.canonicalize(CanonicalizationAlgorithm::Unstable);
    g
}

fn graph(p: &Parsed) -> Graph {
    canonical(p.triples.iter().map(|t| {
        oxrdf::Triple::new(
            subject(&t.s),
            NamedNode::new_unchecked(t.p.as_str()),
            term(&t.o),
        )
    }))
}

/// `oxrdfxml`'s graph, or its first error.
fn theirs(text: &str, base: Option<&str>) -> Result<Graph, String> {
    let mut parser = oxrdfxml::RdfXmlParser::new();
    if let Some(b) = base {
        parser = parser.with_base_iri(b).map_err(|e| e.to_string())?;
    }
    let mut triples = Vec::new();
    for t in parser.for_slice(text) {
        triples.push(t.map_err(|e| e.to_string())?);
    }
    Ok(canonical(triples))
}

/// `None` when the graphs are equal, else the triples only one has.
fn difference(ours: &Graph, theirs: &Graph) -> Option<String> {
    if ours == theirs {
        return None;
    }
    let mut out = format!("ours {} triples, oxrdfxml {}\n", ours.len(), theirs.len());
    for (name, x, y) in [("ours", ours, theirs), ("oxrdfxml", theirs, ours)] {
        for t in x.iter().filter(|t| !y.contains(*t)).take(10) {
            writeln!(out, "  only {name}: {t}").unwrap();
        }
    }
    Some(out)
}

/// [`difference`], unless the graphs differ only where `oxrdfxml` keeps
/// the white space of an attribute value that XML 1.0 (3.3.3) makes
/// spaces: each triple only `oxrdfxml` has is, with its tabs and line
/// ends made spaces, a triple only ours has, each of ours used once,
/// and one of our triples with that subject (when an IRI), predicate
/// and value has as object the value of an attribute of `text` that
/// holds a tab or a line end (a character reference is not normalized).
fn difference_past_normalization(text: &str, p: &Parsed, theirs: &Graph) -> Option<String> {
    let ours = &graph(p);
    let iri = |n: &Node| match n {
        Node::Iri(i) => Some(i.as_str()),
        _ => None,
    };
    let normalized_values: FxHashSet<(Option<&str>, &str, &str)> = p
        .triples
        .iter()
        .filter_map(|t| {
            let Node::Literal { value, .. } = &t.o else {
                return None;
            };
            let (from, to) = (t.o_span.start as usize, t.o_span.end as usize);
            let quote = text[..from].chars().next_back()?;
            let quoted = matches!(quote, '"' | '\'') && text[to..].starts_with(quote);
            let raw = &text[from..to];
            let key = (iri(&t.s), t.p.as_str(), &**value);
            (quoted && raw.contains(['\t', '\n', '\r'])).then_some(key)
        })
        .collect();
    let mut only_ours: Vec<oxrdf::Triple> = ours
        .iter()
        .filter(|t| !theirs.contains(*t))
        .map(|t| t.into_owned())
        .collect();
    let normalized = |t: &oxrdf::TripleRef<'_>| -> Option<oxrdf::Triple> {
        let oxrdf::TermRef::Literal(l) = t.object else {
            return None;
        };
        let value = l
            .value()
            .replace("\r\n", " ")
            .replace(['\t', '\n', '\r'], " ");
        let s = match t.subject {
            oxrdf::NamedOrBlankNodeRef::NamedNode(n) => Some(n.as_str()),
            oxrdf::NamedOrBlankNodeRef::BlankNode(_) => None,
        };
        if !normalized_values.contains(&(s, t.predicate.as_str(), value.as_str())) {
            return None;
        }
        let o = match l.language() {
            Some(lang) => Literal::new_language_tagged_literal_unchecked(value, lang),
            None => Literal::new_typed_literal(value, l.datatype()),
        };
        Some(oxrdf::Triple::new(
            t.subject.into_owned(),
            t.predicate.into_owned(),
            o,
        ))
    };
    let explained = theirs.iter().filter(|t| !ours.contains(*t)).all(|t| {
        let at = normalized(&t).and_then(|n| only_ours.iter().position(|o| *o == n));
        at.map(|i| only_ours.swap_remove(i)).is_some()
    });
    if explained && only_ours.is_empty() {
        None
    } else {
        difference(ours, theirs)
    }
}

/// The oracle accepts a difference only where attribute-value
/// normalization explains it: not in element text, where a line end
/// stays, and each triple of ours explains one of `oxrdfxml`'s.
#[test]
fn only_attribute_normalization_explains_a_difference() {
    let doc = |body: &str| {
        format!(r#"{HEAD}<rdf:Description rdf:about="urn:ex:s"{body}</rdf:Description></rdf:RDF>"#)
    };
    let lit = |p: &str, v: &str| {
        oxrdf::Triple::new(
            NamedNode::new_unchecked("urn:ex:s"),
            NamedNode::new_unchecked(format!("urn:ex:{p}")),
            Literal::new_simple_literal(v),
        )
    };
    // An attribute value with a line end: explained.
    let text = doc(" e:p=\"a\nb\">");
    let p = parse(&text, None);
    let want = theirs(&text, None).unwrap();
    assert!(want.contains(&lit("p", "a\nb")));
    assert_eq!(difference_past_normalization(&text, &p, &want), None);
    // Element text keeps its line end: a space in ours is no
    // normalization.
    let text = doc("><e:p>a b</e:p>");
    let p = parse(&text, None);
    let want = canonical([lit("p", "a\nb")]);
    assert!(difference_past_normalization(&text, &p, &want).is_some());
    // Two of theirs, one of ours.
    let text = doc(" e:p=\"a\nb\" e:q=\"x\">");
    let p = parse(&text, None);
    let want = canonical([lit("p", "a\nb"), lit("p", "a\tb")]);
    assert!(difference_past_normalization(&text, &p, &want).is_some());
}

/// Our parse of a text `oxrdfxml` reads without error: no error, and
/// the same graph.
#[track_caller]
fn agrees(text: &str, base: Option<&str>) -> Parsed {
    let p = parse(text, base);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let want = theirs(text, base).expect("oxrdfxml reads it");
    if let Some(d) = difference(&graph(&p), &want) {
        panic!("{d}");
    }
    p
}

fn at(text: &str, s: Span) -> &str {
    &text[s.start as usize..s.end as usize]
}

/// Every span is inside the text and on character boundaries; refs and
/// statements are sorted and disjoint; `statement` never decreases.
#[track_caller]
fn spans_hold(text: &str, p: &Parsed) {
    let inside = |s: Span| {
        s.start <= s.end
            && (s.end as usize) <= text.len()
            && text.is_char_boundary(s.start as usize)
            && text.is_char_boundary(s.end as usize)
    };
    for t in &p.triples {
        assert!(inside(t.p_span) && inside(t.o_span), "{t:?}");
        assert!((t.statement as usize) < p.statements.len(), "{t:?}");
    }
    assert!(
        p.triples
            .windows(2)
            .all(|w| w[0].statement <= w[1].statement)
    );
    for e in &p.errors {
        assert!(inside(e.span), "{e:?}");
    }
    for r in &p.refs {
        assert!(inside(r.span), "{r:?}");
    }
    assert!(p.refs.windows(2).all(|w| w[0].span.end <= w[1].span.start));
    for s in &p.statements {
        assert!(inside(s.span), "{s:?}");
    }
    assert!(
        p.statements
            .windows(2)
            .all(|w| w[0].span.end <= w[1].span.start)
    );
}

#[test]
fn a_document_gives_its_triples_refs_and_statements() {
    let text = r##"<?xml version="1.0"?>
<!DOCTYPE rdf:RDF [
  <!ENTITY ex "http://example.org/">
]>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:ex="http://example.org/"
         xmlns="http://example.org/d#"
         xml:base="http://example.org/base">
  <!-- a class -->
  <rdfs:Class rdf:about="http://example.org/Dog" rdfs:label="Dog">
    <rdfs:subClassOf rdf:resource="&ex;Animal"/>
    <rdfs:comment xml:lang="en">A dog</rdfs:comment>
    <ex:size rdf:datatype="http://www.w3.org/2001/XMLSchema#integer">3</ex:size>
    <rdfs:seeAlso>
      <ex:Page rdf:about="#page"><ex:title>t</ex:title></ex:Page>
    </rdfs:seeAlso>
    <ex:list rdf:parseType="Collection"><rdf:Description rdf:about="a"/><ex:B/></ex:list>
    <ex:knows rdf:parseType="Resource"><ex:name>n</ex:name></ex:knows>
    <ex:ref rdf:ID="r1" rdf:nodeID="b1"/>
    <local>x</local>
  </rdfs:Class>
  <rdf:Seq rdf:ID="s"><rdf:li>a</rdf:li><rdf:li rdf:resource="#x"/></rdf:Seq>
</rdf:RDF>
"##;
    let p = agrees(text, None);
    spans_hold(text, &p);
    assert_eq!(p.syntax, Syntax::RdfXml);
    assert_eq!(p.document_base, None);
    assert_eq!(parse(text, Some(BASE)).document_base.as_deref(), Some(BASE));
    assert_eq!(p.statements.len(), 2);
    assert_eq!(
        p.statements[0]
            .subject
            .map(|(i, s)| (i.as_str(), at(text, s))),
        Some(("http://example.org/Dog", "http://example.org/Dog"))
    );
    assert!(at(text, p.statements[0].span).starts_with("<rdfs:Class"));
    assert!(at(text, p.statements[0].span).ends_with("</rdfs:Class>"));
    assert!(at(text, p.statements[1].span).starts_with("<rdf:Seq"));
    // Each triple's predicate and object, as written.
    let written: Vec<(&str, &str)> = p
        .triples
        .iter()
        .map(|t| (at(text, t.p_span), at(text, t.o_span)))
        .collect();
    for pair in [
        ("rdfs:label", "Dog"),
        ("rdfs:Class", "rdfs:Class"),
        ("rdfs:subClassOf", "&ex;Animal"),
        ("rdfs:comment", "A dog"),
        ("ex:size", "3"),
        ("ex:title", "t"),
        ("ex:name", "n"),
        ("local", "x"),
        ("rdf:li", "a"),
        ("rdf:li", "#x"),
    ] {
        assert!(written.contains(&pair), "{pair:?} not in {written:?}");
    }
    assert!(written.iter().any(|(p, o)| *p == "rdfs:seeAlso"
        && o.starts_with("<ex:Page")
        && o.ends_with("</ex:Page>")));
    assert!(
        written
            .iter()
            .any(|(p, o)| *p == "ex:knows" && *o == "<ex:name>n</ex:name>")
    );
    // Every reference slices its IRI or name; an anchor is near it.
    let base = oxiri::Iri::parse(BASE.to_string()).unwrap();
    for r in &p.refs {
        let s = at(text, r.span);
        match r.form {
            Form::Iri => assert_eq!(resolve(&base, s).unwrap().as_str(), r.iri.as_str()),
            Form::PName { prefix_len } => {
                assert_eq!(s.as_bytes()[prefix_len as usize], b':', "{s}");
                // An end tag's prefix is its start tag's use.
                if !text[..r.span.start as usize].ends_with("</") {
                    let use_ = p.prefix_uses.iter().find(|u| u.span == r.span).unwrap();
                    assert_eq!(use_.name.as_str(), &s[..prefix_len as usize]);
                }
                assert!(
                    r.iri.as_str().ends_with(&s[prefix_len as usize + 1..]),
                    "{s}"
                );
            }
            Form::Anchor => assert!(
                s.contains('&') || matches!(s, "local" | "rdf:li" | "r1" | "s"),
                "{s}"
            ),
            Form::A => panic!("no `a` in RDF/XML"),
        }
    }
    let roles = |iri: &str| -> Vec<Role> {
        p.refs
            .iter()
            .filter(|r| r.iri.as_str() == iri)
            .map(|r| r.role)
            .collect()
    };
    assert_eq!(roles("http://example.org/Dog"), [Role::Subject]);
    assert_eq!(roles("http://example.org/Animal"), [Role::Object]);
    assert_eq!(
        roles("http://www.w3.org/2001/XMLSchema#integer"),
        [Role::Datatype]
    );
    assert_eq!(
        roles("http://www.w3.org/2000/01/rdf-schema#subClassOf"),
        [Role::Predicate]
    );
    assert_eq!(roles(&format!("{RDF}_2")), [Role::Predicate]);
    assert!(p.prefixes.is_empty());
    assert_eq!(p.comments.len(), 1);
    // The same text with single-quoted entities reads the same.
    let single = text.replace(r#""http://example.org/">"#, "'http://example.org/'>");
    assert_ne!(single, text);
    let q = parse(&single, None);
    assert!(q.errors.is_empty(), "{:?}", q.errors);
    assert_eq!(graph(&q), graph(&p));
    // Blank nodes are numbered in the order they appear: two parses agree.
    assert_eq!(parse(text, None), p);
}

/// The name in an element's end tag is a reference too, with the start
/// tag's IRI, role and form: a rename edits it, so Find References and
/// highlights show it. An empty element, an end implied by an error, a
/// name from an entity and a name inside an XML literal give none.
#[test]
fn an_end_tag_names_its_element_iri() {
    let text = format!(
        "<!DOCTYPE r [<!ENTITY m \"<ex:q>1</ex:q>\">]>\n\
         <rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n\
         <ex:C rdf:about=\"http://example.org/s\"><ex:p>1</ex:p><ex:e/>&m;\
         <ex:x rdf:parseType=\"Literal\"><ex:in>t</ex:in></ex:x><ex:li rdf:parseType=\"Collection\"><ex:D/></ex:li></ex:C>\n\
         </rdf:RDF>\n"
    );
    let p = parse(&text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    spans_hold(&text, &p);
    let refs = |local: &str| -> Vec<(String, Role, Form)> {
        p.refs
            .iter()
            .filter(|r| r.iri.as_str() == format!("http://example.org/{local}"))
            .map(|r| (at(&text, r.span).to_string(), r.role, r.form))
            .collect()
    };
    let pname = Form::PName { prefix_len: 2 };
    assert_eq!(
        refs("C"),
        [
            ("ex:C".into(), Role::Object, pname),
            ("ex:C".into(), Role::Object, pname)
        ]
    );
    assert_eq!(
        refs("p"),
        [
            ("ex:p".into(), Role::Predicate, pname),
            ("ex:p".into(), Role::Predicate, pname)
        ]
    );
    assert_eq!(refs("li").len(), 2);
    assert_eq!(refs("e"), [("ex:e".into(), Role::Predicate, pname)]);
    assert_eq!(refs("q"), [("&m;".into(), Role::Predicate, Form::Anchor)]);
    assert_eq!(refs("in"), []);
    assert_eq!(refs("D"), [("ex:D".into(), Role::Object, pname)]);
    // The end tag's reference is the second, after its content's.
    let c: Vec<u32> = p
        .refs
        .iter()
        .filter(|r| r.iri.as_str() == "http://example.org/C")
        .map(|r| r.span.start)
        .collect();
    assert_eq!(c[1] as usize, text.rfind("</ex:C>").unwrap() + 2);
    // An end that an error implies: no end-tag reference.
    let broken = format!(
        "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n<ex:C rdf:about=\"http://example.org/s\">\n</rdf:RDF>\n"
    );
    let p = parse(&broken, None);
    assert!(!p.errors.is_empty());
    assert_eq!(
        p.refs
            .iter()
            .filter(|r| r.iri.as_str() == "http://example.org/C")
            .count(),
        1
    );
}

/// A nested node element that only names an IRI (a collection item, or
/// a property's object) is a use of it, not a description.
#[test]
fn a_nested_element_that_only_names_an_iri_is_an_object() {
    let text = concat!(
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" ",
        "xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:ex=\"http://example.org/\">\n",
        "  <owl:Class rdf:about=\"http://example.org/A\">\n",
        "    <owl:unionOf rdf:parseType=\"Collection\">\n",
        "      <rdf:Description rdf:about=\"http://example.org/B\"/>\n",
        "      <rdf:Description rdf:about=\"http://example.org/C\" ex:n=\"1\"/>\n",
        "      <ex:K rdf:about=\"http://example.org/D\"/>\n",
        "    </owl:unionOf>\n",
        "    <ex:p><rdf:Description rdf:about=\"http://example.org/E\"></rdf:Description></ex:p>\n",
        "    <ex:p><rdf:Description rdf:about=\"http://example.org/F\"><ex:q>x</ex:q></rdf:Description></ex:p>\n",
        "  </owl:Class>\n",
        "  <rdf:Description rdf:about=\"http://example.org/G\"/>\n",
        "</rdf:RDF>\n",
    );
    let p = agrees(text, None);
    let role = |iri: &str| {
        let roles: Vec<Role> = p
            .refs
            .iter()
            .filter(|r| r.iri.as_str() == format!("http://example.org/{iri}"))
            .map(|r| r.role)
            .collect();
        roles
    };
    for (iri, want) in [
        ("A", Role::Subject),
        ("B", Role::Object),
        ("C", Role::Subject),
        ("D", Role::Subject),
        ("E", Role::Object),
        ("F", Role::Subject),
        ("G", Role::Subject),
    ] {
        assert_eq!(role(iri), [want], "{iri}");
    }
}

#[test]
fn rdf_12_terms_read_as_oxrdfxml_reads_them() {
    let head = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:its="http://www.w3.org/2005/11/its" xmlns:ex="http://example.org/""#;
    for body in [
        // its:dir: on text, on property attributes, inherited by them.
        r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a" xml:lang="ar" its:dir="rtl" ex:q="x"><ex:p its:dir="ltr">t</ex:p><ex:r>u</ex:r><ex:s ex:t="v"/></rdf:Description></rdf:RDF>"#,
        // Without rdf:version 1.2, its:dir is not read.
        r#"><rdf:Description rdf:about="http://example.org/a" xml:lang="en" its:dir="rtl" ex:q="x"><ex:p its:dir="ltr">t</ex:p></rdf:Description></rdf:RDF>"#,
        r#" rdf:version="1.2-basic"><rdf:Description rdf:about="http://example.org/a" xml:lang="en" its:dir="ltr"><ex:p its:dir="rtl">t</ex:p></rdf:Description></rdf:RDF>"#,
        // Triple terms, annotated and not.
        r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a"><ex:says rdf:parseType="Triple" rdf:annotation="http://example.org/r"><rdf:Description rdf:about="http://example.org/b"><ex:p rdf:resource="http://example.org/c"/></rdf:Description></ex:says><ex:hears rdf:parseType="Triple" rdf:annotationNodeID="n"><rdf:Description rdf:about="http://example.org/b"><ex:p rdf:resource="http://example.org/c"/></rdf:Description></ex:hears></rdf:Description></rdf:RDF>"#,
        r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a"><ex:says rdf:parseType="Triple"><rdf:Description rdf:about="http://example.org/b"><ex:p>x</ex:p></rdf:Description></ex:says><ex:q>y</ex:q></rdf:Description></rdf:RDF>"#,
        r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a"><ex:p rdf:annotationNodeID="n" rdf:ID="i" xml:lang="en">x</ex:p><ex:q rdf:annotation="http://example.org/r2" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#,
    ] {
        let text = format!("{head}{body}");
        let p = agrees(&text, Some(BASE));
        spans_hold(&text, &p);
    }
}

#[test]
fn rdf_11_forms_read_as_oxrdfxml_reads_them() {
    for text in [
        // A typed node element as the root, without rdf:RDF.
        r#"<ex:Thing xmlns:ex="http://example.org/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" rdf:about="a" ex:p="1"><ex:r rdf:type="http://example.org/T" ex:s="2"/></ex:Thing>"#,
        // Nested xml:base and xml:lang, rdf:li, containers, empty and
        // nested collections, rdf:nodeID shared, reification.
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/" xml:base="http://example.org/one/" xml:lang="en">
  <rdf:Bag rdf:about="bag" xml:base="http://example.org/two/"><rdf:li rdf:resource="x"/><rdf:li xml:lang="fr">deux</rdf:li><rdf:_7>seven</rdf:_7><rdf:li>three</rdf:li></rdf:Bag>
  <rdf:Description rdf:nodeID="n1"><ex:p rdf:nodeID="n1"/><ex:e rdf:parseType="Collection"/><ex:l rdf:parseType="Collection"><ex:T/><rdf:Description rdf:nodeID="n1"/></ex:l></rdf:Description>
  <rdf:Description rdf:ID="d"><ex:p rdf:ID="st">v</ex:p><ex:q rdf:parseType="Resource" rdf:ID="st2"><ex:r>w</ex:r></ex:q></rdf:Description>
  <rdf:Description><ex:empty/><ex:space>  </ex:space><ex:entity>a &amp; b &#x263A;</ex:entity><ex:cdata><![CDATA[<x>]]></ex:cdata></rdf:Description>
</rdf:RDF>"#,
    ] {
        let p = agrees(text, Some(BASE));
        spans_hold(text, &p);
    }
}

#[test]
fn errors_are_reported_and_the_rest_is_read() {
    let text = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/a" rdf:nodeID="x">
    <ex:p>one</ex:p>
    <nope:q>two</nope:q>
    <ex:r>three<ex:s/></ex:r>
    <rdf:Description/>
    <ex:t rdf:resource="http://example.org/b" rdf:nodeID="y"/>
    <ex:u>unclosed
  </rdf:Description>
  <rdf:Description rdf:about="http://example.org/c"><ex:v>ok</ex:v></rdf:Description>
</rdf:RDF>"#;
    let p = parse(text, None);
    spans_hold(text, &p);
    let found: Vec<(&str, &str)> = p
        .errors
        .iter()
        .map(|e| (e.code, at(text, e.span)))
        .collect();
    assert_eq!(
        found,
        [
            ("rdfxml-syntax", "rdf:nodeID"),
            ("rdfxml-syntax", "nope:q"),
            ("rdfxml-syntax", "ex:s"),
            ("rdfxml-syntax", "rdf:Description"),
            ("rdfxml-syntax", "rdf:nodeID"),
            ("xml-syntax", "ex:u"),
        ]
    );
    let objects: Vec<String> = p.triples.iter().map(|t| term(&t.o).to_string()).collect();
    for o in ["\"one\"", "\"three\"", "<http://example.org/b>", "\"ok\""] {
        assert!(objects.iter().any(|x| x == o), "{o} not in {objects:?}");
    }
    assert!(
        !objects
            .iter()
            .any(|x| x.contains("unclosed") || x == "\"two\"")
    );
}

#[test]
fn xml_literals_are_exclusive_canonical_xml() {
    let text = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/" xmlns="http://www.w3.org/1999/xhtml" xmlns:h="http://example.org/h">
<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Literal"><h:b z="1" a="&lt;&quot;"><i>x &amp; y</i><!-- c --><br/></h:b></ex:p><ex:e rdf:parseType="Literal"></ex:e></rdf:Description></rdf:RDF>"#;
    let p = parse(text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let values: Vec<&str> = p
        .triples
        .iter()
        .filter_map(|t| match &t.o {
            Node::Literal { value, .. } => Some(&**value),
            _ => None,
        })
        .collect();
    assert_eq!(
        values,
        [
            r#"<h:b xmlns:h="http://example.org/h" a="&lt;&quot;" z="1"><i xmlns="http://www.w3.org/1999/xhtml">x &amp; y</i><br xmlns="http://www.w3.org/1999/xhtml"></br></h:b>"#,
            ""
        ]
    );
}

/// Texts `oxrdfxml` reads into another graph than this parser, on
/// purpose (see the module doc): (name, text). Each must still be read
/// by `oxrdfxml` and still differ, so the list cannot go stale.
#[test]
fn oxrdfxml_departures() {
    let head = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/">"#;
    let cases = [
        (
            "XML literal namespaces",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Literal"><b/></ex:p></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "whitespace, CDATA, text",
            format!(
                "{head}<rdf:Description rdf:about=\"http://example.org/a\"><ex:p>\n<![CDATA[x]]>\n</ex:p></rdf:Description></rdf:RDF>"
            ),
        ),
        (
            "an entity declared twice",
            format!(
                r#"<!DOCTYPE r [<!ENTITY e "http://example.org/1"><!ENTITY e "http://example.org/2">]>{head}<rdf:Description rdf:about="&e;"><ex:p>x</ex:p></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "a parameter entity",
            format!(
                r#"<!DOCTYPE r [<!ENTITY % e "http://example.org/1">]>{head}<rdf:Description rdf:about="&e;"><ex:p>x</ex:p></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "`<` in an attribute value",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a" ex:q="a<b"/></rdf:RDF>"#
            ),
        ),
        (
            "rdf:type alone on an empty property element",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:type="http://example.org/T"/></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "a line end in an attribute value",
            format!(
                "{head}<rdf:Description rdf:about=\"http://example.org/a\" ex:q=\"a\nb\"/></rdf:RDF>"
            ),
        ),
        (
            "a tab in an attribute value",
            format!(
                "{head}<rdf:Description rdf:about=\"http://example.org/a\" ex:q=\"a\tb\"/></rdf:RDF>"
            ),
        ),
        (
            "a CR LF line end in text",
            format!(
                "{head}<rdf:Description rdf:about=\"http://example.org/a\"><ex:p>a\r\nb</ex:p></rdf:Description></rdf:RDF>"
            ),
        ),
        (
            "an unknown rdf:parseType",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Other"><ex:b>x</ex:b></ex:p></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "rdf:parseType=\"Triple\" without rdf:version",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:says rdf:parseType="Triple"><rdf:Description rdf:about="http://example.org/s"><ex:p rdf:resource="http://example.org/o"/></rdf:Description></ex:says></rdf:Description></rdf:RDF>"#
            ),
        ),
        (
            "rdf:parseType=\"Triple\" under rdf:version=\"1.2-basic\"",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/a" rdf:version="1.2-basic"><ex:says rdf:parseType="Triple"><rdf:Description rdf:about="http://example.org/s"><ex:p rdf:resource="http://example.org/o"/></rdf:Description></ex:says></rdf:Description></rdf:RDF>"#
            ),
        ),
    ];
    for (name, text) in cases {
        let want = theirs(&text, None).unwrap_or_else(|e| panic!("{name}: oxrdfxml: {e}"));
        let p = parse(&text, None);
        spans_hold(&text, &p);
        assert!(
            difference(&graph(&p), &want).is_some(),
            "{name}: the graphs agree now"
        );
    }
}

/// Texts `oxrdfxml` reads without error where this parser reports one
/// (see the module doc): (name, text, the errors as code and text at
/// the span). Each must still be read by `oxrdfxml`, so the list
/// cannot go stale.
#[test]
fn oxrdfxml_reads_what_this_parser_reports() {
    let head = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/">"#;
    let v12 = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/" rdf:version="1.2">"#;
    let cases = [
        (
            "rdf:bagID on a node element (RDF/XML 6.2.4)",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/s" rdf:bagID="b"><ex:p rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#
            ),
            vec![("rdfxml-syntax", "rdf:bagID")],
        ),
        (
            "rdf:bagID on a property element (RDF/XML 6.2.4)",
            format!(
                r#"{head}<rdf:Description rdf:about="http://example.org/s"><ex:p rdf:bagID="b" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#
            ),
            vec![("rdfxml-syntax", "rdf:bagID")],
        ),
        (
            "rdf:annotation and rdf:annotationNodeID on one element (RDF 1.2 XML 6.2.22)",
            format!(
                r#"{v12}<rdf:Description rdf:about="http://example.org/s"><ex:q rdf:annotation="http://example.org/a" rdf:annotationNodeID="b1" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#
            ),
            vec![("rdfxml-syntax", "rdf:annotationNodeID")],
        ),
        (
            "rdf:annotation on parseType=\"Collection\" (RDF 1.2 XML 6.2.20)",
            format!(
                r#"{v12}<rdf:Description rdf:about="http://example.org/s"><ex:d rdf:annotation="http://example.org/r" rdf:parseType="Collection"><rdf:Description rdf:about="http://example.org/i"/></ex:d></rdf:Description></rdf:RDF>"#
            ),
            vec![("rdfxml-syntax", "rdf:annotation")],
        ),
        (
            "an empty prefixed namespace in XML 1.0 (Namespaces in XML 1.0, 3)",
            format!(
                r#"<?xml version="1.0"?>{head}<rdf:Description rdf:about="http://example.org/s" xmlns:ex=""><rdf:value>v</rdf:value></rdf:Description></rdf:RDF>"#
            ),
            vec![("xml-syntax", "xmlns:ex")],
        ),
        (
            "an XML declaration after white space (XML 1.0, 2.8)",
            format!(
                r#"


<?xml version="1.0"?>{head}<rdf:Description rdf:about="http://example.org/s"><ex:p>v</ex:p></rdf:Description></rdf:RDF>"#
            ),
            vec![("xml-syntax", "<?xml")],
        ),
    ];
    for (name, text, errors) in cases {
        theirs(&text, None).unwrap_or_else(|e| panic!("{name}: oxrdfxml: {e}"));
        let p = parse(&text, None);
        spans_hold(&text, &p);
        assert_eq!(errors_at(&text, &p), errors, "{name}");
    }
}

/// The W3C RDF/XML suite (RDF 1.1), vendored in tests/w3c.
const W3C: &str = "https://w3c.github.io/rdf-tests/rdf/";

/// Tests where this parser departs from the suite: (test IRI, why).
/// A listed test that passes fails the run.
const W3C_KNOWN: &[(&str, &str)] = &[];

/// Evaluation tests `oxrdfxml` reads into another graph than the
/// suite's (and ours): their XML literals are not in exclusive
/// canonical form. Any other difference from `oxrdfxml` fails the run.
const W3C_NOT_OXRDFXML: &[&str] = &["xml-canon-test001", "xml-canon-test002"];

fn w3c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/rdf-tests")
}

fn w3c_local(url: &str) -> PathBuf {
    w3c_dir().join(url.strip_prefix(W3C).expect("a suite URL"))
}

/// (test IRI, evaluation or not, action URL, result URL).
fn w3c_tests() -> Vec<(String, bool, String, Option<String>)> {
    use oxrdf::vocab::rdf;
    let url = format!("{W3C}rdf11/rdf-xml/manifest.ttl");
    let src = std::fs::read(w3c_local(&url)).unwrap();
    let mut g = Graph::new();
    for t in oxttl::TurtleParser::new()
        .with_base_iri(&url)
        .unwrap()
        .for_slice(&src)
    {
        g.insert(&t.unwrap());
    }
    let mf = |l: &str| {
        NamedNode::new_unchecked(format!(
            "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#{l}"
        ))
    };
    let manifest: NamedOrBlankNode = NamedNode::new_unchecked(&url).into();
    let mut list = g
        .object_for_subject_predicate(&manifest, &mf("entries"))
        .unwrap()
        .into_owned();
    let mut tests = Vec::new();
    while list != Term::NamedNode(rdf::NIL.into_owned()) {
        let node: NamedOrBlankNode = match &list {
            Term::BlankNode(b) => b.clone().into(),
            Term::NamedNode(n) => n.clone().into(),
            _ => panic!("bad list"),
        };
        let entry = g
            .object_for_subject_predicate(&node, rdf::FIRST)
            .unwrap()
            .into_owned();
        list = g
            .object_for_subject_predicate(&node, rdf::REST)
            .unwrap()
            .into_owned();
        let Term::NamedNode(entry) = entry else {
            panic!("an entry is an IRI")
        };
        let iri = |p: &str| match g.object_for_subject_predicate(&entry, &mf(p)) {
            Some(oxrdf::TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        };
        let ty = match g.object_for_subject_predicate(&entry, rdf::TYPE) {
            Some(oxrdf::TermRef::NamedNode(n)) => n.as_str().to_string(),
            _ => panic!("{entry} has no type"),
        };
        let eval = match ty.strip_prefix("http://www.w3.org/ns/rdftest#") {
            Some("TestXMLEval") => true,
            Some("TestXMLNegativeSyntax") => false,
            _ => panic!("unknown test type {ty}"),
        };
        tests.push((
            entry.as_str().to_string(),
            eval,
            iri("action").unwrap(),
            iri("result"),
        ));
    }
    tests
}

/// `Ok` when the test passes, else why not.
fn w3c_run(eval: bool, action: &str, result: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(w3c_local(action)).map_err(|e| e.to_string())?;
    let p = parse(&text, Some(action));
    spans_hold(&text, &p);
    if !eval {
        return if p.errors.is_empty() {
            Err("no error reported".into())
        } else {
            Ok(())
        };
    }
    if let Some(e) = p.errors.first() {
        return Err(format!("{} at {}: {}", e.code, e.span.start, e.message()));
    }
    let nt = std::fs::read(w3c_local(result.ok_or("no result")?)).map_err(|e| e.to_string())?;
    let want = canonical(
        oxttl::NTriplesParser::new()
            .for_slice(&nt)
            .map(|t| t.expect("the expected N-Triples parse")),
    );
    let got = graph(&p);
    if let Some(d) = difference(&got, &want) {
        return Err(d);
    }
    let name = action.rsplit_once('/').map_or(action, |x| x.0);
    let id = format!(
        "{}-{}",
        name.rsplit('/').next().unwrap_or(""),
        action
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rdf")
    );
    let listed = W3C_NOT_OXRDFXML.contains(&id.as_str());
    match theirs(&text, Some(action)) {
        Ok(g) if (g != got) != listed => Err(format!(
            "oxrdfxml {} this test (W3C_NOT_OXRDFXML lists {id}: {listed})",
            if listed {
                "now agrees on"
            } else {
                "gives another graph for"
            }
        )),
        _ => Ok(()),
    }
}

#[test]
fn rdfxml_w3c_suite() {
    let tests = w3c_tests();
    let (mut passed, mut failures, mut stale) = (0, Vec::new(), Vec::new());
    for (iri, eval, action, result) in &tests {
        let known = W3C_KNOWN.iter().find(|k| k.0 == iri);
        match (w3c_run(*eval, action, result.as_deref()), known) {
            (Ok(()), None) => passed += 1,
            (Ok(()), Some(_)) => stale.push(iri.clone()),
            (Err(_), Some(_)) => {}
            (Err(e), None) => failures.push(format!("{iri}: {e}")),
        }
    }
    eprintln!(
        "W3C rdf11/rdf-xml: {passed}/{} pass, {} known",
        tests.len(),
        W3C_KNOWN.len()
    );
    assert!(tests.len() >= 160, "only {} tests", tests.len());
    assert!(stale.is_empty(), "known failures now pass: {stale:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Corpus files where our graph departs from `oxrdfxml`'s on purpose:
/// (file, why). Only attribute-value normalization may explain it
/// ([`difference_past_normalization`]).
const CORPUS_KNOWN: &[(&str, &str)] = &[(
    "foaf.rdf",
    "two rdfs:comment attribute values hold a line end, which XML 1.0 (3.3.3) makes a space; oxrdfxml keeps it",
)];
const CORPUS: [&str; 4] = ["foaf.rdf", "obo-bfo.rdf", "obo-ro.rdf", "obo-pato.rdf"];

fn corpus(name: &str) -> Option<(String, String)> {
    let path = corpus_path(name)?;
    let text = std::fs::read_to_string(path).unwrap();
    Some((format!("http://example.org/corpus/{name}"), text))
}

#[test]
fn corpus_graphs_are_oxrdfxml_graphs() {
    for name in CORPUS {
        let Some((base, text)) = corpus(name) else {
            continue;
        };
        let p = parse(&text, Some(&base));
        assert!(
            p.errors.is_empty(),
            "{name}: {:?}",
            &p.errors[..p.errors.len().min(5)]
        );
        spans_hold(&text, &p);
        let want = theirs(&text, Some(&base)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ours = graph(&p);
        if let Some(d) = difference_past_normalization(&text, &p, &want) {
            panic!("{name}: {d}");
        }
        let known = CORPUS_KNOWN.iter().find(|k| k.0 == name);
        match (ours == want, known) {
            (true, None) => eprintln!("{name}: ok, {} triples", p.triples.len()),
            (false, Some((_, why))) => eprintln!("{name}: known difference: {why}"),
            (true, Some(_)) => panic!("{name} now agrees: remove it from CORPUS_KNOWN"),
            (false, None) => panic!(
                "{name}: attribute values normalized: add it to CORPUS_KNOWN\n{}",
                difference(&ours, &want).unwrap_or_default()
            ),
        }
    }
}

/// xorshift64: the same inputs on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The start and end of every tag (not comments, PIs or declarations).
fn tags(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(o) = b[i..].iter().position(|&c| c == b'<') {
        let s = i + o;
        let Some(e) = b[s..].iter().position(|&c| c == b'>').map(|e| s + e + 1) else {
            break;
        };
        if !matches!(b.get(s + 1), Some(b'!' | b'?')) {
            out.push((s, e));
        }
        i = s + 1;
    }
    out
}

/// Parse a mutated text: no panic, spans inside it, in bounded time,
/// and `oxrdfxml`'s graph whenever it reads the text without error.
/// `true` when `oxrdfxml` read it.
#[track_caller]
fn survives(what: &str, text: &str, budget: Duration) -> bool {
    let started = Instant::now();
    let p = parse(text, Some(BASE));
    let took = started.elapsed();
    assert!(took < budget, "{what}: {took:?}");
    spans_hold(text, &p);
    let Ok(want) = theirs(text, Some(BASE)) else {
        return false;
    };
    if let Some(d) = difference_past_normalization(text, &p, &want) {
        panic!("{what}: oxrdfxml reads it, and the graphs differ: {d}");
    }
    true
}

fn fuzz(name: &str, text: &str, cuts: usize, edits: usize, seed: u64) {
    let mut rng = Rng(seed);
    let budget = Duration::from_millis(200) + Duration::from_micros(text.len() as u64 / 10);
    let mut accepted = 0;
    for i in 0..cuts {
        let mut at = rng.below(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        accepted += usize::from(survives(
            &format!("{name} cut {i} at {at}"),
            &text[..at],
            budget,
        ));
    }
    let tags = tags(text);
    for i in 0..edits {
        let (s, e) = tags[rng.below(tags.len())];
        let mutated = if i % 2 == 0 {
            format!("{}{}", &text[..s], &text[e..])
        } else {
            format!("{}{}", &text[..e], &text[s..])
        };
        let what = format!(
            "{name} {} {:?}",
            if i % 2 == 0 { "deleted" } else { "doubled" },
            &text[s..e]
        );
        accepted += usize::from(survives(&what, &mutated, budget));
    }
    eprintln!(
        "{name}: {} mutations, {accepted} read by oxrdfxml",
        cuts + edits
    );
}

/// Cut and tag-edited corpus files: each survived, and read as
/// `oxrdfxml` reads it when it reads it. `CHAOS_ITERS=n` makes it n
/// mutations of FOAF and of BFO and n / 8 of RO (1.2 MB), two thirds of
/// them cuts; `CHAOS_SEED` changes the seed.
#[test]
fn mutated_corpus_files_are_survived() {
    let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let seed = env("CHAOS_SEED").unwrap_or(0x9E37_79B9_7F4A_7C15);
    let n = env("CHAOS_ITERS").map_or(300, |n| n as usize);
    for (name, count) in [
        ("foaf.rdf", n),
        ("obo-bfo.rdf", n),
        ("obo-ro.rdf", if n == 300 { n } else { n / 8 }),
        ("obo-pato.rdf", 5),
    ] {
        let Some((_, text)) = corpus(name) else {
            continue;
        };
        fuzz(name, &text, count * 2 / 3, count - count * 2 / 3, seed);
    }
}

/// Inputs that would take quadratic time in a careless scanner: a tag
/// with 100,000 attributes, a long run of `%` in a DOCTYPE, 20,000
/// namespace declarations used by 100,000 names, and an XML literal
/// whose element uses 20,000 namespaces. Each parses well under a
/// second (a few tens of milliseconds measured).
#[test]
fn timing_long_inputs_parse_in_linear_time() {
    let rdf = r#"xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#""#;
    let mut attrs = String::new();
    for i in 0..100_000 {
        write!(attrs, " ex:a{i}=\"v\"").unwrap();
    }
    let many_attrs = format!(
        r#"<rdf:RDF {rdf} xmlns:ex="http://example.org/"><rdf:Description rdf:about="http://example.org/s"{attrs}/></rdf:RDF>"#
    );
    let percent = format!("<!DOCTYPE r [{} x;]><r/>", "%".repeat(200_000));
    let mut decls = String::new();
    for i in 0..20_000 {
        write!(decls, " xmlns:p{i}=\"http://example.org/{i}/\"").unwrap();
    }
    let mut names = String::new();
    for i in 0..100_000 {
        write!(names, "<p{}:q>x</p{}:q>", i % 20_000, i % 20_000).unwrap();
    }
    let many_names = format!(
        r#"<rdf:RDF {rdf}{decls}><rdf:Description rdf:about="http://example.org/s">{names}</rdf:Description></rdf:RDF>"#
    );
    let mut used = String::new();
    for i in 0..20_000 {
        write!(used, " p{i}:a=\"v\"").unwrap();
    }
    let literal = format!(
        r#"<rdf:RDF {rdf}{decls}><rdf:Description rdf:about="http://example.org/s"><p0:q rdf:parseType="Literal"><x{used}><y{used}/></x></p0:q></rdf:Description></rdf:RDF>"#
    );
    for (name, text, triples) in [
        ("attributes", &many_attrs, 100_000),
        ("percent", &percent, 0),
        ("names", &many_names, 100_000),
        ("literal", &literal, 1),
    ] {
        let started = Instant::now();
        let p = parse(text, None);
        let took = started.elapsed();
        eprintln!("{name}: {} bytes in {took:?}", text.len());
        assert_eq!(p.triples.len(), triples, "{name}: {:?}", p.errors.first());
        assert!(took < Duration::from_secs(1), "{name}: {took:?}");
    }
}

const HEAD: &str =
    r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:e="urn:ex:">"#;

/// The triples of `p` as N-Triples lines, sorted.
fn lines_of(p: &Parsed) -> Vec<String> {
    let mut v: Vec<String> = p
        .triples
        .iter()
        .map(|t| {
            oxrdf::Triple::new(
                subject(&t.s),
                NamedNode::new_unchecked(t.p.as_str()),
                term(&t.o),
            )
            .to_string()
        })
        .collect();
    v.sort();
    v
}

/// RDF/XML 7.2.21: an empty property element with only property
/// attributes, `rdf:type` among them, has a blank node object that
/// takes them.
#[test]
fn an_empty_property_element_with_rdf_type_has_a_blank_node_object() {
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="urn:s"><e:p rdf:type="urn:T"/><e:q rdf:type="urn:T" e:r="v"/></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    spans_hold(&text, &p);
    assert_eq!(
        lines_of(&p),
        [
            "<urn:s> <urn:ex:p> _:b0",
            "<urn:s> <urn:ex:q> _:b1",
            "_:b0 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T>",
            "_:b1 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T>",
            "_:b1 <urn:ex:r> \"v\"",
        ]
    );
}

/// XML 1.0 2.11 and 3.3.3: line ends are `\n`; in an attribute value,
/// white space is a space, but a character reference stays the
/// character. Spans stay over the text as written.
#[test]
fn line_ends_and_attribute_values_are_normalized() {
    let text = format!(
        "{HEAD}<rdf:Description rdf:about=\"urn:s\" e:a=\"a\nb\" e:b=\"a\r\nb\" e:c=\"a\tb&#10;c&#9;\"><e:t>a\r\nb\rc&#13;</e:t><e:d><![CDATA[x\r\ny]]></e:d></rdf:Description></rdf:RDF>"
    );
    let p = parse(&text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    spans_hold(&text, &p);
    let value = |iri: &str| {
        let t = p.triples.iter().find(|t| t.p.as_str() == iri).unwrap();
        let Node::Literal { value, .. } = &t.o else {
            panic!()
        };
        (value.to_string(), at(&text, t.o_span))
    };
    assert_eq!(value("urn:ex:a"), ("a b".into(), "a\nb"));
    assert_eq!(value("urn:ex:b"), ("a b".into(), "a\r\nb"));
    assert_eq!(value("urn:ex:c"), ("a b\nc\t".into(), "a\tb&#10;c&#9;"));
    assert_eq!(value("urn:ex:t"), ("a\nb\nc\r".into(), "a\r\nb\rc&#13;"));
    assert_eq!(value("urn:ex:d").0, "x\ny");
}

/// A triple whose subject, predicate or object does not resolve to an
/// absolute IRI is dropped, whatever the path; the error stays.
#[test]
fn a_triple_with_an_iri_that_does_not_resolve_is_dropped() {
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="bad subject"><e:p>text</e:p></rdf:Description><rdf:Description rdf:about="urn:s"><e:p rdf:resource="bad object"/><e:q rdf:datatype="bad type">1</e:q><e:ok>x</e:ok></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, Some("http://example.org/"));
    spans_hold(&text, &p);
    assert_eq!(lines_of(&p), ["<urn:s> <urn:ex:ok> \"x\""]);
    let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
    assert_eq!(errors, ["bad subject", "bad object", "bad type"]);
    // rdf:ID with no base: no IRI, no triple.
    let text =
        format!(r#"{HEAD}<rdf:Description rdf:ID="a"><e:p>x</e:p></rdf:Description></rdf:RDF>"#);
    let p = parse(&text, None);
    assert!(p.triples.is_empty(), "{:?}", p.triples);
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
}

/// RDF/XML 7.2.20: any other `rdf:parseType` reads as "Literal".
#[test]
fn an_unknown_parse_type_is_an_xml_literal() {
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="urn:s"><e:p rdf:parseType="Other"><e:b>x</e:b></e:p></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(
        lines_of(&p),
        [
            r#"<urn:s> <urn:ex:p> "<e:b xmlns:e=\"urn:ex:\">x</e:b>"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral>"#
        ]
    );
}

/// XML 1.0 4.4.2 and 4.5: an entity reference in a replacement text
/// expands where the entity is used, and a replacement text with
/// markup in content is read as markup. Everything read from it has
/// the span of the reference.
#[test]
fn entities_nest_and_their_markup_is_read() {
    let text = format!(
        r#"<!DOCTYPE r [<!ENTITY a "&b;"><!ENTITY b "value"><!ENTITY l "<e:label>hello</e:label>"><!ENTITY two "&l;<e:n>&a;</e:n>"><!ENTITY ltx "&lt;e:x/>"><!ENTITY sixty "&#60;e:y/>">]>{HEAD}<rdf:Description rdf:about="urn:s" e:p="&a;">&l;</rdf:Description><rdf:Description rdf:about="urn:t">&two;<e:q>&lt;</e:q></rdf:Description><rdf:Description rdf:about="urn:u"><e:text>&ltx;</e:text>&sixty;</rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    spans_hold(&text, &p);
    assert_eq!(
        lines_of(&p),
        [
            "<urn:s> <urn:ex:label> \"hello\"",
            "<urn:s> <urn:ex:p> \"value\"",
            "<urn:t> <urn:ex:label> \"hello\"",
            "<urn:t> <urn:ex:n> \"value\"",
            "<urn:t> <urn:ex:q> \"<\"",
            "<urn:u> <urn:ex:text> \"<e:x/>\"",
            "<urn:u> <urn:ex:y> \"\"",
        ]
    );
    for t in &p.triples {
        let (pred, obj) = (at(&text, t.p_span), at(&text, t.o_span));
        match t.p.as_str() {
            "urn:ex:label" => assert!(pred == "&l;" || pred == "&two;", "{pred}"),
            "urn:ex:n" => assert_eq!((pred, obj), ("&two;", "&two;")),
            "urn:ex:y" => assert_eq!(pred, "&sixty;"),
            _ => {}
        }
    }
    // References inside an entity are anchors at its reference.
    for r in &p.refs {
        let s = at(&text, r.span);
        if s.starts_with('&') {
            assert_eq!(r.form, Form::Anchor, "{s}");
        }
    }
}

/// The entity limits hold with references that nest: a recursive
/// entity is refused, markup that does not close is closed at the end
/// of its replacement text, and a markup entity in an attribute value
/// is reported.
#[test]
fn nested_entities_keep_their_limits() {
    for (body, error) in [
        ("<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">", "refers to itself"),
        ("<!ENTITY a \"<e:p>x\">", "is not closed in the entity &a;"),
        (
            "<!ENTITY a \"</rdf:Description>\">",
            "closes no open element",
        ),
        (
            "<!ENTITY a \"<e:p>x</e:p>\">",
            "puts `<` in an attribute value",
        ),
    ] {
        let text = format!(
            r#"<!DOCTYPE r [{body}]>{HEAD}<rdf:Description rdf:about="urn:s" e:v="&a;">&a;</rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        assert!(
            p.errors.iter().any(|e| e.message().contains(error)),
            "{body}: {:?}",
            p.errors
        );
        for e in &p.errors {
            assert_ne!(e.span.start, e.span.end, "{body}: {e:?}");
        }
    }
    // An element the entity leaves open ends at the reference (found by
    // the chaos test: a collection item's span ended inside the
    // replacement text).
    let text = format!(
        r#"<!DOCTYPE r [<!ENTITY a "x<e:N>">]>{HEAD}<rdf:Description rdf:about="urn:s"><e:list rdf:parseType="Collection">&a;</e:list></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    assert!(!p.triples.is_empty());
    // A billion laughs in markup: refused past 64 KiB, and the text read
    // stays within the budget (`&a11;` is under the declaration limit:
    // the budget stops it).
    let mut decl = String::from("<!DOCTYPE r [<!ENTITY a0 \"<e:p>x</e:p>\">");
    for i in 1..30 {
        write!(decl, "<!ENTITY a{i} \"&a{0};&a{0};\">", i - 1).unwrap();
    }
    let text = format!(
        r#"{decl}]>{HEAD}<rdf:Description rdf:about="urn:s">{}</rdf:Description></rdf:RDF>"#,
        "&a11;&a12;&a29;".repeat(50)
    );
    let started = Instant::now();
    let p = parse(&text, None);
    assert!(started.elapsed() < Duration::from_secs(1));
    spans_hold(&text, &p);
    assert!(
        p.errors.iter().any(|e| e.message().contains("KiB")),
        "{:?}",
        p.errors
    );
    assert!(
        p.errors
            .iter()
            .any(|e| e.message().contains("expansion is over its limit")),
        "{:?}",
        p.errors
    );
    assert!(p.triples.len() >= 2048, "{}", p.triples.len());
    assert!(p.triples.len() <= text.len() * xml::EXPANSION_PER_BYTE / 10 + 10_000);
}

/// The work of expanding entities is bounded, not only the text they
/// expand to: a ladder of entities over an empty one expands to
/// nothing in 2^n steps (28 levels took 5.6 s), in an attribute value,
/// in element text and in markup. Past the budget one error is
/// reported, the rest of the references are kept as written, and the
/// document is read on.
#[test]
fn timing_entity_expansion_work_is_bounded() {
    let mut ladder = String::from("<!ENTITY e0 \"\">");
    for i in 1..200 {
        write!(ladder, "<!ENTITY e{i} \"&e{0};&e{0};\">", i - 1).unwrap();
    }
    ladder += "<!ENTITY m \"<e:p>&e199;</e:p>\">";
    for body in [
        r#"<rdf:Description rdf:about="urn:s" e:p="a&e199;b"/>"#,
        r#"<rdf:Description rdf:about="urn:s"><e:p>a&e199;b</e:p></rdf:Description>"#,
        r#"<rdf:Description rdf:about="urn:s">&m;</rdf:Description>"#,
    ] {
        let text = format!(
            r#"<!DOCTYPE r [{ladder}]>{HEAD}{body}<rdf:Description rdf:about="urn:t" e:q="after"/></rdf:RDF>"#
        );
        let started = Instant::now();
        let p = parse(&text, None);
        let took = started.elapsed();
        spans_hold(&text, &p);
        let budget: Vec<&str> = p
            .errors
            .iter()
            .map(|e| e.message())
            .filter(|m| m.contains("expansion"))
            .collect();
        assert_eq!(budget.len(), 1, "{body}: {:?}", p.errors);
        assert!(
            lines_of(&p).contains(&"<urn:t> <urn:ex:q> \"after\"".to_string()),
            "{body}"
        );
        assert!(took < Duration::from_secs(1), "{body}: {took:?}");
    }
    // Entities within the declaration limit, used past the budget: the
    // first uses expand, the rest are kept as written.
    let mut decl = String::from("<!DOCTYPE r [<!ENTITY a0 \"<e:p>x</e:p>\">");
    for i in 1..12 {
        write!(decl, "<!ENTITY a{i} \"&a{0};&a{0};\">", i - 1).unwrap();
    }
    let text = format!(
        r#"{decl}]>{HEAD}<rdf:Description rdf:about="urn:s">{}</rdf:Description><rdf:Description rdf:about="urn:t" e:q="after"/></rdf:RDF>"#,
        "&a11;".repeat(50)
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    let m: Vec<&str> = p.errors.iter().map(|e| e.message()).collect();
    assert!(!m.iter().any(|m| m.contains("KiB")), "{m:?}");
    assert_eq!(
        m.iter().filter(|m| m.contains("expansion")).count(),
        1,
        "{m:?}"
    );
    assert!(p.triples.len() > 2048, "{}", p.triples.len());
    assert!(p.triples.len() <= text.len() * xml::EXPANSION_PER_BYTE / 10 + 10_000);
    assert!(lines_of(&p).contains(&"<urn:t> <urn:ex:q> \"after\"".to_string()));
}

/// A reference inside a CDATA section of a replacement text is text
/// when the entity is read as markup: no reference to itself.
#[test]
fn cdata_in_a_replacement_text_keeps_its_references_as_text() {
    let text = format!(
        r#"<!DOCTYPE r [<!ENTITY note "<e:p><![CDATA[&note;]]></e:p>">]>{HEAD}<rdf:Description rdf:about="urn:s">&note;</rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(lines_of(&p), ["<urn:s> <urn:ex:p> \"&note;\""]);
}

/// Every span made inside or after an entity reference is over the
/// text, start before end: the white space an entity expands to is not
/// in the text (12 spaces from `&pad;`).
#[test]
fn spans_after_an_entity_are_in_the_text() {
    let text = format!(
        r#"<!DOCTYPE r [<!ENTITY pad "            ">]>{HEAD}&pad;x<rdf:Description rdf:about="urn:s"><e:p>&pad;y</e:p>&pad;z</rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
    assert_eq!(errors, ["&pad;x", "&pad;z"], "{:?}", p.errors);
}

/// Namespaces in XML 6.3: two attributes with one namespace and local
/// name are an error; the tag is read on.
#[test]
fn two_attributes_with_one_expanded_name_are_an_error() {
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="urn:s" xmlns:z="urn:ex:" e:p="1" z:p="2"><e:q>x</e:q></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    let errors: Vec<(&str, &str)> = p
        .errors
        .iter()
        .map(|e| (e.code, at(&text, e.span)))
        .collect();
    assert_eq!(errors, [("xml-syntax", "z:p")]);
    assert!(lines_of(&p).contains(&"<urn:s> <urn:ex:q> \"x\"".to_string()));
    // Many attributes: the same, through the map.
    let many: String = (0..40).map(|i| format!(" e:a{i}=\"v\"")).collect();
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="urn:s" xmlns:z="urn:ex:"{many} z:a3="w"/></rdf:RDF>"#
    );
    let p = parse(&text, None);
    let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
    assert_eq!(errors, ["z:a3"]);
}

/// XML 1.0 2.1: a document has a root element.
#[test]
fn a_document_without_a_root_element_is_an_error() {
    for text in [
        "<?xml version=\"1.0\"?>",
        "<?xml version=\"1.0\"?>\n<!-- c -->\n",
        "",
        "  \n",
    ] {
        let p = parse(text, None);
        spans_hold(text, &p);
        let errors: Vec<(&str, &str)> = p.errors.iter().map(|e| (e.code, e.message())).collect();
        assert_eq!(
            errors,
            [("xml-syntax", "the document has no root element")],
            "{text:?}"
        );
    }
}

/// The IRIs one parse makes are bounded by its text: a long namespace
/// with many short local names does not make the interner keep a copy
/// of the namespace per name past [`IRI_BYTES_PER_BYTE`].
#[test]
fn the_iris_a_parse_makes_are_bounded() {
    let unique = format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let ns = format!("http://example.org/{unique}/{}/", "n".repeat(8 * 1024));
    let names = 20_000;
    let mut body = String::new();
    for i in 0..names {
        write!(body, "<x:p{i}>v</x:p{i}>").unwrap();
    }
    let text = format!(
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:x="{ns}"><rdf:Description rdf:about="urn:s">{body}</rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    let iris: Vec<String> = (0..names).map(|i| format!("{ns}p{i}")).collect();
    let kept: Vec<&String> = iris.iter().filter(|i| Iri::get(i).is_some()).collect();
    let kept_bytes: usize = kept.iter().map(|i| i.len()).sum();
    let unbounded: usize = iris.iter().map(String::len).sum();
    let kept = kept.len();
    let budget = text.len() * IRI_BYTES_PER_BYTE + MIN_IRI_BYTES;
    eprintln!(
        "text {} bytes; {kept} of {names} IRIs kept, {kept_bytes} bytes (budget {budget}, unbounded {unbounded})",
        text.len()
    );
    assert!(kept_bytes <= budget, "{kept_bytes} > {budget}");
    assert!(kept < names);
    assert_eq!(p.triples.len(), kept);
    assert_eq!(
        p.errors.len(),
        1,
        "{:?}",
        &p.errors[..p.errors.len().min(3)]
    );
    assert!(p.errors[0].message().contains("bytes per byte of text"));
    // A second parse makes no more.
    let again = parse(&text, None);
    assert_eq!(again, p);
}

/// A property attribute whose name is refused (past the IRI budget, or
/// in no namespace) still makes its element an empty property element
/// with attributes: the object is a blank node, never the empty literal
/// the text does not say.
#[test]
fn a_refused_property_attribute_keeps_the_production() {
    let unique = format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let ns = format!("http://example.org/{unique}/{}/", "n".repeat(8 * 1024));
    let mut body = String::new();
    for i in 0..20_000 {
        write!(body, "<x:p{i}>v</x:p{i}>").unwrap();
    }
    let text = format!(
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:x="{ns}"><rdf:Description rdf:about="urn:ex:s">{body}</rdf:Description><rdf:Description rdf:about="urn:ex:s"><x:p0 x:fresh="v"/></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    // `urn:ex:s` and `x:p0` were made before the budget was spent.
    assert!(Iri::get(&format!("{ns}fresh")).is_none());
    let p0 = format!("{ns}p0");
    let objects: Vec<&Node> = p
        .triples
        .iter()
        .filter(|t| t.p == p0.as_str())
        .map(|t| &t.o)
        .collect();
    assert_eq!(objects.len(), 2, "{objects:?}");
    assert!(matches!(objects[1], Node::Blank(_)), "{objects:?}");
    // A name in no namespace: reported, and the same production.
    let text = format!(
        r#"{HEAD}<rdf:Description rdf:about="urn:ex:t"><e:p plain="v"/></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
    assert_eq!(errors, ["plain"]);
    assert_eq!(p.triples.len(), 1, "{:?}", p.triples);
    assert!(matches!(p.triples[0].o, Node::Blank(_)), "{:?}", p.triples);
}

/// The release-build parse of PATO (21 MB, 257k triples): 71 ms
/// measured (2026-11-19, Apple silicon, load average 3.4). The budget
/// leaves room for CI, which builds without fat LTO on slower runners,
/// as the JSON-LD frontend's does.
/// A triple that names an IRI that is not absolute is dropped, as Turtle
/// and oxrdfxml drop it; the error stays.
#[test]
fn a_triple_with_an_invalid_iri_is_dropped_and_reported() {
    let text = concat!(
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n",
        "  <rdf:Description rdf:about=\"http://example.org/a b\">\n",
        "    <ex:p rdf:resource=\"http://example.org/c\"/>\n",
        "  </rdf:Description>\n",
        "  <rdf:Description rdf:about=\"http://example.org/d\">\n",
        "    <ex:p rdf:resource=\"http://example.org/c\"/>\n",
        "  </rdf:Description>\n",
        "</rdf:RDF>\n",
    );
    let p = parse(text, Some("http://example.org/"));
    assert_eq!(p.triples.len(), 1, "{:?}", p.triples);
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    assert!(p.errors[0].message.contains("not an absolute IRI"));
}

/// The errors of a parse: their code and the text at their span.
fn errors_at<'t>(text: &'t str, p: &Parsed) -> Vec<(&'static str, &'t str)> {
    p.errors
        .iter()
        .map(|e| (e.code, at(text, e.span)))
        .collect()
}

const EX_HEAD: &str = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/""#;

/// RDF 1.2 XML Syntax 6.2.19: `rdf:parseType="Triple"` gives a triple
/// term whatever `rdf:version` says, as `rdf:annotation` gives one; a
/// version announcement only lets a parser warn early (RDF 1.2
/// Concepts 1.10).
#[test]
fn a_triple_term_is_read_whatever_the_version() {
    for version in [
        "",
        r#" rdf:version="1.1""#,
        r#" rdf:version="1.2-basic""#,
        r#" rdf:version="1.2""#,
    ] {
        let text = format!(
            r#"{EX_HEAD}{version}><rdf:Description rdf:about="http://example.org/a"><ex:q>sibling</ex:q><ex:says rdf:parseType="Triple"><rdf:Description rdf:about="http://example.org/s"><ex:p rdf:resource="http://example.org/o"/></rdf:Description></ex:says><ex:r rdf:annotation="http://example.org/n">x</ex:r></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        assert_eq!(errors_at(&text, &p), [], "{version}");
        assert_eq!(
            lines_of(&p),
            [
                r#"<http://example.org/a> <http://example.org/q> "sibling""#,
                r#"<http://example.org/a> <http://example.org/r> "x""#,
                "<http://example.org/a> <http://example.org/says> <<( <http://example.org/s> <http://example.org/p> <http://example.org/o> )>>",
                r#"<http://example.org/n> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <http://example.org/a> <http://example.org/r> "x" )>>"#,
            ],
            "{version}"
        );
    }
}

/// RDF/XML 6.2.4 (`oldTerms`): `rdf:bagID`, `rdf:aboutEach` and
/// `rdf:aboutEachPrefix` were withdrawn; none is an attribute, on a node
/// or a property element. The element is read without it.
#[test]
fn withdrawn_rdf_attributes_are_errors() {
    for (attr, value) in [
        ("bagID", "bag1"),
        ("aboutEach", "http://example.org/b"),
        ("aboutEachPrefix", "http://example.org/"),
    ] {
        let text = format!(
            r#"{EX_HEAD}><rdf:Description rdf:about="http://example.org/s" rdf:{attr}="{value}"><ex:p rdf:{attr}="{value}" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        let name = format!("rdf:{attr}");
        assert_eq!(
            errors_at(&text, &p),
            [("rdfxml-syntax", &*name), ("rdfxml-syntax", &*name)],
            "{attr}"
        );
        assert_eq!(
            lines_of(&p),
            ["<http://example.org/s> <http://example.org/p> <http://example.org/o>"],
            "{attr}"
        );
    }
}

/// RDF 1.2 XML Syntax 6.2.15 to 6.2.22: a property element takes
/// `rdf:annotation` or `rdf:annotationNodeID`, not both
/// (`(annotationAttr | annotationNodeIDAttr)?`). The first is kept.
#[test]
fn both_annotation_attributes_are_an_error() {
    let text = format!(
        r#"{EX_HEAD} rdf:version="1.2"><rdf:Description rdf:about="http://example.org/s"><ex:q rdf:annotation="http://example.org/a" rdf:annotationNodeID="b1" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#
    );
    let p = parse(&text, None);
    spans_hold(&text, &p);
    assert_eq!(
        errors_at(&text, &p),
        [("rdfxml-syntax", "rdf:annotationNodeID")]
    );
    assert_eq!(
        lines_of(&p),
        [
            "<http://example.org/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <http://example.org/s> <http://example.org/q> <http://example.org/o> )>>",
            "<http://example.org/s> <http://example.org/q> <http://example.org/o>",
        ]
    );
}

/// RDF 1.2 XML Syntax 6.2.20: `parseTypeCollectionPropertyElt` is
/// `set(idAttr?, parseCollection)`: no annotation. The list is read
/// without it. `Literal`, `Resource`, an unknown parse type (6.2.17,
/// 6.2.18, 6.2.21) and `Triple` (6.1) take one.
#[test]
fn a_collection_takes_no_annotation() {
    for attr in [
        r#"rdf:annotation="http://example.org/r""#,
        r#"rdf:annotationNodeID="r""#,
    ] {
        let text = format!(
            r#"{EX_HEAD} rdf:version="1.2"><rdf:Description rdf:about="http://example.org/s"><ex:d {attr} rdf:parseType="Collection"><rdf:Description rdf:about="http://example.org/i"/></ex:d></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        let name = attr.split('=').next().unwrap();
        assert_eq!(errors_at(&text, &p), [("rdfxml-syntax", name)], "{attr}");
        let lines = lines_of(&p);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines.iter().all(|l| !l.contains("reifies")), "{lines:?}");
    }
    let node =
        r#"<rdf:Description rdf:about="http://example.org/i"><ex:p>v</ex:p></rdf:Description>"#;
    for (parse_type, content) in [
        ("Literal", node),
        ("Resource", "<ex:p>v</ex:p>"),
        ("Other", node),
        ("Triple", node),
    ] {
        let text = format!(
            r#"{EX_HEAD} rdf:version="1.2"><rdf:Description rdf:about="http://example.org/s"><ex:d rdf:annotation="http://example.org/r" rdf:parseType="{parse_type}">{content}</ex:d></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        assert_eq!(errors_at(&text, &p), [], "{parse_type}");
        assert!(
            lines_of(&p).iter().any(|l| l.contains("reifies")),
            "{parse_type}"
        );
    }
}

#[test]
fn timing_rdfxml_parse() {
    let Some((base, text)) = corpus("obo-pato.rdf") else {
        return;
    };
    let started = Instant::now();
    let p = parse(&text, Some(&base));
    let took = started.elapsed();
    eprintln!("obo-pato.rdf: {} triples in {took:?}", p.triples.len());
    assert!(p.errors.is_empty());
    assert!(took < Duration::from_millis(500), "{took:?}");
}
