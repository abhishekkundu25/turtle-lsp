use super::facts::{Subj, subj};
use super::*;
use crate::shacl::vocab;
use crate::syntax::PrefixMap;
use crate::syntax::{self, Node, Syntax};
use std::path::Path;

#[test]
fn a_namespace_ends_at_the_last_delimiter_unless_a_vocabulary_owns_more() {
    assert_eq!(
        namespace_of("http://example.org/ns#a"),
        "http://example.org/ns#"
    );
    assert_eq!(namespace_of("urn:x:y"), "urn:x:");
    assert_eq!(namespace_of("plain"), "");
    // Open Graph's local names hold a colon.
    assert_eq!(
        namespace_of("http://ogp.me/ns#audio:album"),
        "http://ogp.me/ns#"
    );
    assert_eq!(namespace_of("http://ogp.me/ns#title"), "http://ogp.me/ns#");
}

fn analysis(uri: &str, text: &str, external: &Arc<PrefixMap>) -> Arc<Analysis> {
    Arc::new(Analysis::new(uri.into(), text, external.clone(), false))
}

fn fixtures() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "ttl") {
                out.push(std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    let mut out = Vec::new();
    walk(&root.join("workspace"), &mut out);
    walk(&root.join("project"), &mut out);
    assert!(out.len() >= 8, "fixtures missing");
    out
}

fn project_prefixes() -> Arc<PrefixMap> {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/project/prefixes.ttl"),
    )
    .unwrap();
    let mut m = PrefixMap::default();
    for p in syntax::parse(&text, &PrefixMap::default(), syntax::Syntax::Turtle).prefixes {
        m.insert(p.name, p.iri.to_string());
    }
    Arc::new(m)
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A fixture, sometimes cut short or with a renamed term, so documents
/// overlap partially and contain syntax errors.
fn variant(rng: &mut Rng, texts: &[String]) -> String {
    let t = &texts[rng.below(texts.len())];
    match rng.below(4) {
        0 => {
            let mut cut = rng.below(t.len() + 1);
            while !t.is_char_boundary(cut) {
                cut -= 1;
            }
            t[..cut].to_string()
        }
        1 => t
            .replacen("Asset", "Assett", 1)
            .replacen("alice", "alicia", 2),
        _ => t.clone(),
    }
}

#[test]
fn incremental_updates_match_a_rebuild() {
    let texts = fixtures();
    let ext = [Arc::new(PrefixMap::default()), project_prefixes()];
    for seed in 1..=8u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
        let mut k = Knowledge::default();
        // The model: what should be registered.
        let mut model: FxHashMap<(bool, String), (Source, Arc<Analysis>)> = FxHashMap::default();
        for step in 0..300 {
            let slot = rng.below(10);
            let external = slot >= 8;
            let uri = format!("file:///ws/d{slot}.ttl");
            let key = (external, uri.clone());
            if rng.below(4) == 0 {
                if external {
                    k.remove_external(&uri);
                } else {
                    k.remove(&uri);
                }
                model.remove(&key);
            } else {
                let text = variant(&mut rng, &texts);
                let a = analysis(&uri, &text, &ext[rng.below(2)]);
                if external {
                    k.add_external(&uri, a.clone());
                    model.insert(key, (Source::External, a));
                } else {
                    let src = if rng.below(2) == 0 {
                        Source::Open
                    } else {
                        Source::Disk
                    };
                    k.set(&uri, src, a.clone());
                    model.insert(key, (src, a));
                }
            }
            if step % 10 == 9 || step == 299 {
                let mut fresh = Knowledge::default();
                let mut keys: Vec<_> = model.keys().cloned().collect();
                keys.sort();
                for key in keys {
                    let (src, a) = &model[&key];
                    if key.0 {
                        fresh.add_external(&key.1, a.clone());
                    } else {
                        fresh.set(&key.1, *src, a.clone());
                    }
                }
                assert_eq!(k.snapshot(), fresh.snapshot(), "seed {seed} step {step}");
            }
        }
        // Removing everything leaves nothing behind.
        for (ext, uri) in model.keys() {
            if *ext {
                k.remove_external(uri);
            } else {
                k.remove(uri);
            }
        }
        assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
    }
}

fn project() -> Knowledge {
    let ext = project_prefixes();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/project");
    let mut k = Knowledge::default();
    for f in [
        "prefixes.ttl",
        "core/vocab.ttl",
        "shapes/asset-shapes.ttl",
        "data/instances.ttl",
        "data/observations.ttl",
    ] {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        let e = if f == "prefixes.ttl" {
            Arc::new(PrefixMap::default())
        } else {
            ext.clone()
        };
        k.set(
            &format!("file:///p/{f}"),
            Source::Disk,
            analysis(f, &text, &e),
        );
    }
    k
}

#[test]
fn facts_from_the_project_fixture() {
    let k = project();
    let sd = "https://ontology.example.com/plant/core/";
    let iri = |l: &str| format!("{sd}{l}");
    assert_eq!(k.label(&iri("Asset")), Some("Asset"));
    assert!(k.is_described(&iri("Asset")));
    assert!(!k.is_described(&iri("Assett")));
    assert_eq!(k.descriptions(&iri("Asset")).len(), 1);
    assert_eq!(
        k.supers(&iri("Asset")),
        [Iri::intern(&iri("PhysicalObject"))]
    );
    let mut subs = k.subs(&iri("PhysicalObject"));
    subs.sort();
    assert_eq!(
        subs,
        [
            Iri::intern(&iri("Asset")),
            Iri::intern(&iri("MeasurementPoint"))
        ]
    );
    assert_eq!(
        k.domains(&iri("locatedIn")),
        [Iri::intern(&iri("PhysicalObject"))]
    );
    assert_eq!(k.ranges(&iri("locatedIn")), [Iri::intern(&iri("Position"))]);
    assert!(k.used_as_predicate(&iri("designation")));
    // English label wins over German.
    assert_eq!(
        k.label("https://vocab.example.com/plant/function/Milling"),
        Some("Milling")
    );

    let shapes = k.shapes_targeting(&iri("Asset"));
    assert_eq!(shapes.len(), 1);
    let shape = shapes[0].1;
    assert_eq!(
        shape.node.map(|n| n.to_string()),
        Some("https://shape.example.com/plant/core/AssetShape".into())
    );
    let by_path = |l: &str| {
        shape
            .properties
            .iter()
            .find(|p| p.path.is_some_and(|x| x == iri(l).as_str()))
            .unwrap()
    };
    let d = by_path("designation");
    assert_eq!((d.min_count, d.max_count), (Some(1), Some(1)));
    assert_eq!(
        d.datatype.map(|x| x.to_string()).as_deref(),
        Some("http://www.w3.org/2001/XMLSchema#string")
    );
    let f = by_path("fillsPosition");
    assert_eq!(f.class, [Iri::intern(&iri("Position"))]);
    assert_eq!((f.min_count, f.max_count), (None, Some(1)));
    // A sequence path is not a simple predicate; its sh:in list is kept.
    let seq = shape.properties.iter().find(|p| p.path.is_none()).unwrap();
    assert_eq!(
        seq.in_values,
        Some(vec![Node::Iri(Iri::intern(
            "https://vocab.example.com/plant/function/Milling"
        ))])
    );

    // sd: is bound to the project namespace in prefixes.ttl and used by fragments.
    let b = k.bindings("sd");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].0, sd);
    assert!(b[0].1 >= 4, "{b:?}");
    assert!(k.is_own_namespace(sd));
    assert!(!k.is_own_namespace("http://www.w3.org/2002/07/owl#"));
    assert!(k.terms_in(sd).contains(&Iri::intern(&iri("hasFunction"))));
}

/// The prefix of an RDF/XML qualified name is an XML namespace's: it
/// binds no project prefix of that name, whatever namespace it holds.
#[test]
fn an_rdfxml_qualified_name_binds_no_project_prefix() {
    let external: PrefixMap = [("ex".to_string(), "http://project.org/".to_string())]
        .into_iter()
        .collect();
    let xml = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://ex.org/\">\n\
            \x20 <ex:C rdf:about=\"http://ex.org/a\"/>\n</rdf:RDF>\n";
    let ttl = "ex:a a ex:C .\n";
    for (uri, text, syntax, bound) in [
        ("file:///w/a.ttl", ttl, Syntax::Turtle, 1),
        ("file:///w/a.owl", xml, Syntax::RdfXml, 0),
    ] {
        let a = Analysis::with_syntax(uri.into(), text, Arc::new(external.clone()), false, syntax);
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        let mut k = Knowledge::default();
        k.set(uri, Source::Open, Arc::new(a));
        assert_eq!(k.bindings("ex").len(), bound, "{syntax:?}");
    }
}

#[test]
fn external_sources_describe_but_do_not_count_as_workspace() {
    let mut k = Knowledge::default();
    let none = Arc::new(PrefixMap::default());
    let text = "@prefix ex: <http://ex.org/> .\nex:a a ex:C .\nex:b a ex:C .\nex:c a ex:C .\n";
    k.add_external("cache:onto", analysis("cache:onto", text, &none));
    assert!(k.is_described("http://ex.org/a"));
    assert!(!k.is_own_namespace("http://ex.org/"));
    assert!(k.bindings("ex").is_empty());
    let g = k.generation();
    k.remove_external("cache:onto");
    assert!(!k.is_described("http://ex.org/a"));
    assert!(k.generation() > g);
    assert!(k.snapshot().is_empty());
}

#[test]
fn ontologies_describing_a_namespace_include_external_sources() {
    let mut k = Knowledge::default();
    let none = Arc::new(PrefixMap::default());
    let owl = "<http://www.w3.org/2002/07/owl#Ontology>";
    let ext = format!("<http://ex.org/x> a {owl} .\n<http://ex.org/x#A> a <urn:c> .\n");
    k.add_external("import:x", analysis("http://ex.org/x", &ext, &none));
    let ws = format!("<http://ex.org/w> a {owl} .\n<http://ex.org/w#B> a <urn:c> .\n");
    k.set(
        "file:///w.ttl",
        Source::Disk,
        analysis("file:///w.ttl", &ws, &none),
    );
    let data = "<http://ex.org/d#C> a <urn:c> .\n";
    k.set(
        "file:///d.ttl",
        Source::Disk,
        analysis("file:///d.ttl", data, &none),
    );
    let names = |ns: &str| -> Vec<String> {
        k.ontologies_describing(ns)
            .iter()
            .map(|(o, _)| o.to_string())
            .collect()
    };
    assert_eq!(names("http://ex.org/x#"), ["http://ex.org/x"]);
    assert_eq!(names("http://ex.org/w#"), ["http://ex.org/w"]);
    assert!(names("http://ex.org/d#").is_empty());
    assert!(names("http://xmlns.com/foaf/0.1/").is_empty());
}

#[test]
fn own_namespace_needs_a_share_of_described_terms() {
    let mut k = Knowledge::default();
    let none = Arc::new(PrefixMap::default());
    let mut text = String::from("@prefix ex: <http://ex.org/> .\n");
    for i in 0..3 {
        text.push_str(&format!("ex:t{i} a ex:C .\n"));
    }
    k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
    // 3 described of 4 referenced (t0..t2, C): below 80%.
    assert!(!k.is_own_namespace("http://ex.org/"));
    text.push_str("ex:C a ex:t0 .\n");
    k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
    // 4 of 4.
    assert!(k.is_own_namespace("http://ex.org/"));
    text.push_str("ex:t0 ex:t1 ex:x .\n");
    k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
    // 4 of 5: exactly 80%.
    assert!(k.is_own_namespace("http://ex.org/"));
    text.push_str("ex:t0 ex:t1 ex:y .\n");
    k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
    // 4 of 6.
    assert!(!k.is_own_namespace("http://ex.org/"));
}

#[test]
fn described_generation_moves_only_when_the_described_set_does() {
    let mut k = Knowledge::default();
    let none = Arc::new(PrefixMap::default());
    let header = "@prefix ex: <http://ex.org/> .\n";
    let set = |k: &mut Knowledge, body: &str| {
        let text = format!("{header}{body}");
        k.set("file:///a.ttl", Source::Open, analysis("a", &text, &none));
        k.described_generation()
    };
    let g = set(&mut k, "ex:a a ex:C .\n");
    // Same described set, different text, objects and whitespace.
    assert_eq!(set(&mut k, "ex:a  a ex:C .\n"), g);
    assert_eq!(set(&mut k, "ex:a a ex:D .\n"), g);
    assert_eq!(set(&mut k, "ex:a a ex:Dx ; ex:p ex:q .\n"), g);
    // A new subject.
    let g2 = set(&mut k, "ex:a a ex:C .\nex:b a ex:C .\n");
    assert!(g2 > g);
    // A new prefix binding.
    let g3 = set(
        &mut k,
        "@prefix f: <http://f.org/> .\nex:a a ex:C .\nex:b a ex:C .\n",
    );
    assert!(g3 > g2);
    // A namespace becoming own (3 of 3 described) without a 0<->1 change
    // in what other files see is still a change for them.
    let g4 = set(&mut k, "ex:a a ex:b .\nex:b a ex:c .\nex:c a ex:a .\n");
    assert!(g4 > g3);
    assert!(k.is_own_namespace("http://ex.org/"));
    k.remove("file:///a.ttl");
    assert!(k.described_generation() > g4);
}

const CURATION_HEADER: &str = "@prefix ex: <http://ex.org/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// Register (or replace) the Turtle document `name` with `body` after
/// [`CURATION_HEADER`].
fn put_ttl(k: &mut Knowledge, name: &str, body: &str) {
    let uri = format!("file:///{name}.ttl");
    let text = format!("{CURATION_HEADER}{body}");
    let none = Arc::new(PrefixMap::default());
    k.set(&uri, Source::Open, analysis(&uri, &text, &none));
}

fn names(v: Vec<Iri>) -> Vec<String> {
    v.iter()
        .map(|i| i.replace("http://ex.org/", "ex:"))
        .collect()
}

#[test]
fn the_term_rule() {
    let mut k = Knowledge::default();
    put_ttl(
        &mut k,
        "a",
        "ex:c1 a owl:Class .\nex:c2 a rdfs:Class .\nex:p1 a owl:ObjectProperty .\n\
             ex:p2 a owl:DatatypeProperty .\nex:p3 a owl:AnnotationProperty .\n\
             ex:p4 a rdf:Property .\nex:k a skos:Concept .\n\
             ex:sub rdfs:subClassOf [ a owl:Restriction ] .\nex:subp rdfs:subPropertyOf ex:p1 .\n\
             ex:alice a ex:Person ; rdfs:label \"Alice\" .\n",
    );
    for t in ["c1", "c2", "p1", "p2", "p3", "p4", "k", "sub", "subp"] {
        assert!(k.is_term(&format!("http://ex.org/{t}")), "{t}");
    }
    // Instance data, and the objects of the links, are not terms.
    for t in ["alice", "Person"] {
        assert!(!k.is_term(&format!("http://ex.org/{t}")), "{t}");
    }
    // An external source does not make terms.
    let none = Arc::new(PrefixMap::default());
    let ext = format!("{CURATION_HEADER}ex:e a owl:Class .\n");
    k.add_external("import:e", analysis("import:e", &ext, &none));
    assert!(!k.is_term("http://ex.org/e"));
    // Nor does it stop the workspace's: a term needs one workspace source.
    put_ttl(&mut k, "b", "ex:c1 a owl:Class .\n");
    k.remove("file:///a.ttl");
    assert!(k.is_term("http://ex.org/c1"));
    assert!(!k.is_term("http://ex.org/c2"));
}

#[test]
fn a_keystroke_in_a_comment_leaves_the_index_untouched() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "b", "ex:C a owl:Class ; rdfs:label \"Pump\" .\n");
    let body = |c: &str| {
        format!(
            "# {c}\nex:A a owl:Class ; rdfs:label \"Pump\" . # {c}\n\
                 ex:B a owl:Class ;\n  # {c}\n  rdfs:label \"Pump\" .\n"
        )
    };
    put_ttl(&mut k, "a", &body("Notes"));
    let (index, described) = (k.snapshot(), k.described_generation());
    for c in ["Notes.", "Notes. More", "Notes. More text"] {
        let g = k.generation();
        put_ttl(&mut k, "a", &body(c));
        assert_eq!(k.snapshot(), index, "{c}");
        assert_eq!(k.described_generation(), described, "{c}");
        assert_eq!(k.changes(), &Changes::default(), "{c}");
        // The analyses it hands out are new.
        assert!(k.generation() > g, "{c}");
    }
    assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B", "ex:C"]);
}

/// The `T` lines of the index (the sources under each text key) that
/// are in one of `x` and `y` only.
fn differing_texts(x: &[String], y: &[String]) -> Vec<String> {
    let only = |a: &[String], b: &[String]| -> Vec<String> {
        a.iter().filter(|l| !b.contains(l)).cloned().collect()
    };
    let mut out = only(x, y);
    out.extend(only(y, x));
    assert!(out.iter().all(|l| l.starts_with("T ")), "{out:?}");
    out.sort();
    out
}

#[test]
fn a_label_edit_changes_only_its_keys() {
    let mut k = Knowledge::default();
    put_ttl(
        &mut k,
        "b",
        "ex:B a owl:Class ; rdfs:label \"Pump\" .\nex:C a owl:Class ; rdfs:label \"Valve\" .\n",
    );
    let body = |l: &str, d: &str| {
        format!(
            "ex:A a owl:Class ; rdfs:label \"{l}\" ; rdfs:comment \"A machine.\" .\n\
                 ex:D a owl:Class ; rdfs:label \"{d}\" .\n"
        )
    };
    let key = |t: &str| text_key(t, None);
    let (a, b) = ("Open:file:///a.ttl", "Open:file:///b.ttl");
    put_ttl(&mut k, "a", &body("Pump", "Drum"));
    let before = k.snapshot();
    // A's label leaves B's and joins C's.
    put_ttl(&mut k, "a", &body("Valve", "Drum"));
    let after = k.snapshot();
    let (pump, valve) = (key("pump"), key("valve"));
    let mut want = vec![
        format!("T Label {pump} [{a},{b}]"),
        format!("T Label {pump} [{b}]"),
        format!("T Label {valve} [{a},{b}]"),
        format!("T Label {valve} [{b}]"),
    ];
    want.sort();
    assert_eq!(differing_texts(&before, &after), want);
    let mut texts = vec![(Text::Label, pump), (Text::Label, valve)];
    texts.sort();
    let want = Changes {
        texts,
        ..Changes::default()
    };
    assert_eq!(k.changes(), &want);
    // D's label shares with nothing, before or after.
    put_ttl(&mut k, "a", &body("Valve", "Drums"));
    let (drum, drums) = (key("drum"), key("drums"));
    let mut want = vec![
        format!("T Label {drum} [{a}]"),
        format!("T Label {drums} [{a}]"),
    ];
    want.sort();
    assert_eq!(differing_texts(&after, &k.snapshot()), want);
    assert_eq!(k.changes(), &Changes::default());
}

#[test]
fn a_ref_added_and_removed_in_one_swap_nets_to_nothing() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "a", "ex:a ex:p ex:x .\nex:b ex:q ex:y .\n");
    let (index, described) = (k.snapshot(), k.described_generation());
    // Each of ex:p, ex:q, ex:x and ex:y goes from one statement and
    // comes in the other.
    for body in [
        "ex:a ex:q ex:y .\nex:b ex:p ex:x .\n",
        // Two references to ex:x, then one.
        "ex:a ex:q ex:y , ex:x .\nex:b ex:p ex:x .\n",
        "ex:a ex:p ex:x .\nex:b ex:q ex:y .\n",
    ] {
        put_ttl(&mut k, "a", body);
        assert_eq!(k.snapshot(), index, "{body}");
        assert_eq!(k.changes(), &Changes::default(), "{body}");
    }
    assert_eq!(k.described_generation(), described);
}

#[test]
fn a_literal_is_found_where_its_statement_is_split_by_another() {
    // JSON-LD gives the triples of a nested node between those of the
    // node it is nested in: A's label comes before B's triples, its
    // `skos:prefLabel` after them.
    let none = Arc::new(PrefixMap::default());
    let json = r#"{"@id": "http://ex.org/A", "@type": "http://www.w3.org/2002/07/owl#Class",
          "http://www.w3.org/2000/01/rdf-schema#label": "pump ",
          "http://www.w3.org/2000/01/rdf-schema#seeAlso": {"@id": "http://ex.org/B",
            "@type": "http://www.w3.org/2002/07/owl#Class",
            "http://www.w3.org/2000/01/rdf-schema#label": "Pump"},
          "http://www.w3.org/2004/02/skos/core#prefLabel": "Pumpe"}"#;
    let mut k = Knowledge::default();
    let uri = "file:///j.jsonld";
    let a = analysis(uri, json, &none);
    let statements: Vec<u32> = a.parsed.triples.iter().map(|t| t.statement).collect();
    assert!(!statements.is_sorted(), "{statements:?}");
    k.set(uri, Source::Open, a);
    assert_eq!(names(k.labelled("PUMP", None)), ["ex:A", "ex:B"]);
    assert_eq!(names(k.labelled("pumpe", None)), ["ex:A"]);
}

#[test]
fn every_described_or_typed_subject_counts_as_a_subject() {
    // The subjects are read from the curation summaries, which cover
    // these: a lookup by subject must find each.
    let none = Arc::new(PrefixMap::default());
    for (i, text) in fixtures().iter().enumerate() {
        let a = analysis(&format!("file:///f{i}.ttl"), text, &none);
        let f = DocFacts::new(&a);
        let has = |iri: &Iri| {
            f.subjects
                .binary_search_by_key(&iri.id(), |c| c.0.id())
                .is_ok()
        };
        for iri in a.defs.keys().chain(a.types.keys()).chain(a.supers.keys()) {
            assert!(has(iri), "fixture {i}: {iri}");
        }
    }
}

/// What other documents' checks read of `k` about each item a change
/// set can name: per IRI, per text key, per namespace, per prefix name.
#[derive(Debug)]
struct Read {
    iris: Vec<(Iri, IriRead)>,
    texts: Vec<((Text, u64), Collisions)>,
    namespaces: Vec<(Iri, bool)>,
    prefixes: Vec<(Iri, Vec<Iri>)>,
}

/// Of an IRI: described, a term, deprecated here, a term with a
/// `skos:definition`, its [`TermState`].
type IriRead = (bool, bool, bool, bool, TermState);

/// Every item a change of the versions `fs` of a source could name.
fn items(fs: &[&DocFacts]) -> Changes {
    let (mut iris, mut texts, mut names) = (Vec::new(), Vec::new(), Vec::new());
    for f in fs {
        let counted = [
            &f.subjects,
            &f.objects,
            &f.refs,
            &f.defs,
            &f.terms,
            &f.defined,
        ];
        iris.extend(counted.into_iter().flatten().map(|c| c.0));
        iris.extend(f.deprecated.iter().map(|d| d.0));
        iris.extend(f.curated.iter().map(|c| c.0));
        for index in Index::ALL {
            texts.extend(index.of(f).iter().map(|k| (index.text(), k.key)));
        }
        names.extend(f.bindings.iter().map(|b| b.0.0));
    }
    let mut namespaces: Vec<Iri> = iris
        .iter()
        .filter_map(|i| Iri::get(namespace_of(i)))
        .collect();
    for v in [&mut iris, &mut namespaces, &mut names] {
        v.sort_by_key(|i| i.id());
        v.dedup();
    }
    texts.sort();
    texts.dedup();
    Changes {
        iris,
        texts,
        namespaces,
        prefixes: names,
    }
}

fn read(k: &Knowledge, items: &Changes) -> Read {
    let state = |i: Iri| {
        let defined = k.marks.get(&i).is_some_and(|m| m.defined > 0);
        (
            k.is_described(&i),
            k.is_term(&i),
            k.deprecated_here(&i),
            k.is_term(&i) && defined,
            k.term_state(i),
        )
    };
    let bound = |name: Iri| {
        let mut ns: Vec<Iri> = k.bindings(&name).into_iter().map(|b| b.0).collect();
        ns.sort();
        ns
    };
    Read {
        iris: items.iris.iter().map(|&i| (i, state(i))).collect(),
        texts: items
            .texts
            .iter()
            .map(|&t| (t, k.collisions(t.0, t.1)))
            .collect(),
        namespaces: items
            .namespaces
            .iter()
            .map(|&n| (n, k.is_own_namespace(&n)))
            .collect(),
        prefixes: items.prefixes.iter().map(|&n| (n, bound(n))).collect(),
    }
}

/// The items whose reads differ between `x` and `y`.
fn moved<T: Copy, S: PartialEq>(x: &[(T, S)], y: &[(T, S)]) -> Vec<T> {
    x.iter()
        .zip(y)
        .filter(|(a, b)| a.1 != b.1)
        .map(|(a, _)| a.0)
        .collect()
}

#[test]
fn the_change_set_names_what_changed_for_other_documents() {
    let texts = fixtures();
    let ext = [Arc::new(PrefixMap::default()), project_prefixes()];
    let mut seen = [0usize; 4];
    for seed in 1..=6u64 {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D ^ seed);
        let mut k = Knowledge::default();
        for step in 0..120 {
            let slot = rng.below(6);
            let external = slot >= 5;
            let uri = format!("file:///ws/d{slot}.ttl");
            let key = if external {
                Key::External(uri.clone())
            } else {
                Key::Workspace(uri.clone())
            };
            let a = (rng.below(4) != 0).then(|| {
                let text = variant(&mut rng, &texts);
                analysis(&uri, &text, &ext[rng.below(2)])
            });
            let new = a.as_ref().map(|a| DocFacts::new(a));
            let old = k.by_key.get(&key).map(|&id| &k.slot(id).facts);
            let all = items(&old.into_iter().chain(new.as_ref()).collect::<Vec<_>>());
            let before = read(&k, &all);
            let (generation, last) = (k.generation(), k.changes().iris.clone());
            match (a, external) {
                (Some(a), true) => k.add_external(&uri, a),
                (Some(a), false) => k.set(&uri, Source::Disk, a),
                (None, true) => k.remove_external(&uri),
                (None, false) => k.remove(&uri),
            }
            let after = read(&k, &all);
            let c = k.changes();
            let at = format!("seed {seed} step {step}");
            if k.generation() == generation {
                // Nothing changed: the set stays the last change's.
                assert!(moved(&before.iris, &after.iris).is_empty(), "{at}");
                assert_eq!(c.iris, last, "{at}");
                continue;
            }
            assert_eq!(c.iris, moved(&before.iris, &after.iris), "{at}");
            assert_eq!(
                c.namespaces,
                moved(&before.namespaces, &after.namespaces),
                "{at}"
            );
            assert_eq!(c.prefixes, moved(&before.prefixes, &after.prefixes), "{at}");
            let texts = moved(&before.texts, &after.texts);
            for t in &c.texts {
                assert!(texts.contains(t), "{at}: {t:?} listed, unchanged");
            }
            for (was, now) in before.texts.iter().zip(&after.texts) {
                if was.1 != now.1 && !c.texts.contains(&was.0) {
                    let t = was.0;
                    // Then an IRI under it is listed.
                    let named = was.1.iter().chain(&now.1).any(|x| c.iris.contains(&x.2));
                    assert!(named, "{at}: {t:?} changed, not named");
                }
            }
            for (n, v) in seen.iter_mut().zip([
                c.iris.len(),
                c.texts.len(),
                c.namespaces.len(),
                c.prefixes.len(),
            ]) {
                *n += v;
            }
        }
    }
    // Each kind of item was named somewhere.
    assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
}

#[test]
fn labelled_folds_case_and_trims_and_keeps_languages_apart() {
    let mut k = Knowledge::default();
    put_ttl(
        &mut k,
        "a",
        "ex:A a owl:Class ; rdfs:label \" Pump\\t\"@en, \"Pumpe\"@de .\n\
             ex:B a owl:Class ; skos:prefLabel \"PUMP\"@EN .\n\
             ex:C a owl:Class ; rdfs:label \"pump\" .\n\
             ex:alice rdfs:label \"Pump\"@en .\n\
             ex:D a owl:Class ; rdfs:comment \"Pump\"@en .\n",
    );
    put_ttl(
        &mut k,
        "b",
        "ex:E rdfs:subClassOf ex:A ; rdfs:label \"pump \"@en .\n",
    );
    let en = names(k.labelled("PUMP", Some("En")));
    assert_eq!(en, ["ex:A", "ex:B", "ex:E"]);
    assert_eq!(names(k.labelled(" pump\n", None)), ["ex:C"]);
    assert_eq!(names(k.labelled("pumpe", Some("de"))), ["ex:A"]);
    assert!(k.labelled("pumpe", Some("en")).is_empty());
    assert!(k.labelled("Pump", Some("xx-never-seen")).is_empty());
    // A term that repeats its label is listed once.
    put_ttl(&mut k, "c", "ex:A rdfs:label \"pump\"@en .\n");
    assert_eq!(
        names(k.labelled("Pump", Some("en"))),
        ["ex:A", "ex:B", "ex:E"]
    );
}

#[test]
fn labels_subtract_exactly_on_replace_and_remove() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "a", "ex:A a owl:Class ; rdfs:label \"Pump\" .\n");
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Pump\" .\n");
    assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B"]);
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
    assert_eq!(names(k.labelled("pump", None)), ["ex:A"]);
    assert_eq!(names(k.labelled("valve", None)), ["ex:B"]);
    // The label stays while the type moves to another document.
    put_ttl(&mut k, "c", "ex:A a owl:Class .\n");
    put_ttl(&mut k, "a", "ex:A rdfs:label \"Pump\" .\n");
    assert_eq!(names(k.labelled("pump", None)), ["ex:A"]);
    k.remove("file:///c.ttl");
    assert!(k.labelled("pump", None).is_empty());
    for d in ["a", "b"] {
        k.remove(&format!("file:///{d}.ttl"));
    }
    assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
}

#[test]
fn a_label_a_document_gives_twice_stays_until_both_go() {
    let mut k = Knowledge::default();
    let a = "ex:A a owl:Class ; rdfs:label \"Pump\" .\n";
    let b = "ex:B a owl:Class ; rdfs:label \"Pump\" .\n";
    let x = "ex:x rdfs:label \"Other\" .\n";
    put_ttl(&mut k, "a", &format!("{a}{x}{b}"));
    assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B"]);
    // Changes at the start, in the middle and at the end.
    let changes: [(String, &[&str]); 3] = [
        (format!("{x}{b}"), &["ex:B"]),
        (format!("{a}{x}{a}{b}"), &["ex:A", "ex:B"]),
        (format!("{a}{x}"), &["ex:A"]),
    ];
    for (body, want) in changes {
        put_ttl(&mut k, "a", &body);
        assert_eq!(names(k.labelled("pump", None)), want, "{body}");
    }
    put_ttl(&mut k, "a", x);
    assert!(k.labelled("pump", None).is_empty());
    k.remove("file:///a.ttl");
    assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
}

/// 400 documents of instance data that share their comments: adding
/// one compares no collisions, as no comment is about a term.
#[cfg(not(debug_assertions))]
#[test]
fn timing_shared_comments_are_not_quadratic() {
    let none = Arc::new(PrefixMap::default());
    let docs: Vec<(String, Arc<Analysis>)> = (0..400)
        .map(|f| {
            let mut text = String::from(CURATION_HEADER);
            for i in 0..100 {
                text += &format!("ex:s{f}_{i} rdfs:comment \"Shared {i}\" .\n");
            }
            let uri = format!("file:///w/{f}.ttl");
            let a = analysis(&uri, &text, &none);
            (uri, a)
        })
        .collect();
    let mut k = Knowledge::default();
    let start = std::time::Instant::now();
    for (uri, a) in docs {
        k.set(&uri, Source::Disk, a);
    }
    let took = start.elapsed();
    assert!(took.as_millis() < 200, "took {took:?}");
}

/// A vocabulary of `n` classes, four triples each (the shape of the
/// `big-vocab` benchmark), with `edit` appended to the label of class
/// `n / 2`.
fn vocabulary(n: usize, edit: &str) -> String {
    let mut out = String::from(CURATION_HEADER);
    for i in 0..n {
        let e = if i == n / 2 { edit } else { "" };
        out += &format!(
            "\nex:C{i} a owl:Class ;\n    rdfs:label \"Class {i}{e}\"@en ;\n    \
                 skos:definition \"A class numbered {i}.\"@en ;\n    rdfs:subClassOf ex:C{} .\n",
            (i + 1) % n
        );
    }
    out
}

/// Typing in a label of a 17,000-class vocabulary. Swapping the new
/// facts in walks each of the source's sorted lists once beside the old
/// one (about ten lists of 17,000 to 34,000 entries, compared without a
/// lookup) and looks up only the two text keys that changed; removing
/// the source and adding it again looks every IRI and key up, and
/// checks every key's collisions, twice. On an M-series laptop: about
/// 0.12 ms against 4.8 + 1.4 ms (2%); swapping by removing and adding
/// cost 1.75 ms before. What limits the swap is the memory those walks
/// read; the analyses stay alive in `versions`, so no drop is timed.
#[cfg(not(debug_assertions))]
#[test]
fn timing_a_label_edit_swaps_in_a_fraction_of_a_rebuild() {
    let none = Arc::new(PrefixMap::default());
    let uri = "file:///vocab.ttl";
    let n = 17_000;
    let versions: Vec<Arc<Analysis>> = (0..9)
        .map(|i| analysis(uri, &vocabulary(n, &"x".repeat(i)), &none))
        .collect();
    let mut k = Knowledge::default();
    k.set(uri, Source::Open, versions[0].clone());
    let (mut swap, mut full) = (Vec::new(), Vec::new());
    for a in &versions[1..] {
        let facts = DocFacts::new(a);
        let t = std::time::Instant::now();
        k.set_prepared(uri, Source::Open, a.clone(), facts);
        swap.push(t.elapsed());
        let facts = DocFacts::new(a);
        let t = std::time::Instant::now();
        k.remove(uri);
        k.set_prepared(uri, Source::Open, a.clone(), facts);
        full.push(t.elapsed());
    }
    // The swaps did their work: the index is the one a rebuild makes.
    let edited = format!("Class {}xxxxxxxx", n / 2);
    assert_eq!(
        names(k.labelled(&edited, Some("en"))),
        [format!("ex:C{}", n / 2)]
    );
    let mut fresh = Knowledge::default();
    fresh.set(uri, Source::Open, versions[8].clone());
    assert_eq!(k.snapshot(), fresh.snapshot());
    swap.sort();
    full.sort();
    let (swap, full) = (swap[swap.len() / 2], full[full.len() / 2]);
    assert!(swap * 10 < full, "swap {swap:?}, remove and add {full:?}");
    assert!(swap.as_micros() < 1_000, "swap {swap:?}");
}

#[test]
fn defined_as_falls_back_to_the_comment() {
    let mut k = Knowledge::default();
    put_ttl(
        &mut k,
        "a",
        "ex:A a owl:Class ; skos:definition \"A machine.\"@en ; rdfs:comment \"Moves water.\"@en .\n\
             ex:B a owl:Class ; rdfs:comment \"a machine. \"@en .\n\
             ex:C a owl:Class ; rdfs:comment \"Moves water.\"@en .\n\
             ex:x rdfs:comment \"Moves water.\"@en .\n",
    );
    assert_eq!(
        names(k.defined_as("A MACHINE.", Some("en"))),
        ["ex:A", "ex:B"]
    );
    // A's comment is not its definition: it has a skos:definition.
    assert_eq!(names(k.defined_as("moves water.", Some("en"))), ["ex:C"]);
    // Once another document gives C a skos:definition, its comment stops counting.
    put_ttl(&mut k, "b", "ex:C skos:definition \"Other.\"@en .\n");
    assert!(k.defined_as("moves water.", Some("en")).is_empty());
    assert_eq!(names(k.defined_as("other.", Some("en"))), ["ex:C"]);
    k.remove("file:///b.ttl");
    assert_eq!(names(k.defined_as("moves water.", Some("en"))), ["ex:C"]);
}

#[test]
fn deprecated_here_reads_boolean_true_and_false_wins() {
    let mut k = Knowledge::default();
    put_ttl(
        &mut k,
        "a",
        "ex:a owl:deprecated true .\nex:b owl:deprecated \"true\"^^xsd:boolean .\n\
             ex:c owl:deprecated false .\nex:d owl:deprecated \"true\" .\nex:e owl:deprecated true .\n",
    );
    let none = Arc::new(PrefixMap::default());
    let json = r#"{"@id": "http://ex.org/j", "http://www.w3.org/2002/07/owl#deprecated": true}"#;
    k.set(
        "file:///j.jsonld",
        Source::Disk,
        analysis("file:///j.jsonld", json, &none),
    );
    let dep = |k: &Knowledge, l: &str| k.deprecated_here(&format!("http://ex.org/{l}"));
    for l in ["a", "b", "j", "e"] {
        assert!(dep(&k, l), "{l}");
    }
    // False, and a plain string, are not deprecations.
    assert!(!dep(&k, "c") && !dep(&k, "d"));
    // A false anywhere in the workspace wins.
    put_ttl(&mut k, "b", "ex:e owl:deprecated false .\n");
    assert!(!dep(&k, "e"));
    k.remove("file:///b.ttl");
    assert!(dep(&k, "e"));
    // An external source's deprecation is not the workspace's.
    let ext = format!("{CURATION_HEADER}ex:x owl:deprecated true .\n");
    k.add_external("import:x", analysis("import:x", &ext, &none));
    assert!(!dep(&k, "x"));
}

#[test]
fn described_generation_moves_on_label_collisions_and_curation_facts() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
    let set = |k: &mut Knowledge, body: &str| {
        put_ttl(k, "a", body);
        k.described_generation()
    };
    let x = "ex:x rdfs:label \"Other\" .\n";
    let g = set(
        &mut k,
        &format!("{x}ex:A a owl:Class ; rdfs:label \"Pu\" .\n"),
    );
    // Typing inside a label that collides with nothing.
    for body in ["\"Pum\" .\n", "\"Pump\" .\n", "\"Pump\" . \n"] {
        let g2 = set(&mut k, &format!("{x}ex:A a owl:Class ; rdfs:label {body}"));
        assert_eq!(g2, g, "{body}");
    }
    // Instance data whose label collides is not a term.
    let body = "ex:x rdfs:label \"valve\" .\nex:A a owl:Class ; rdfs:label \"Pump\" .\n";
    assert_eq!(set(&mut k, body), g);
    // A new collision, and its end.
    let g2 = set(&mut k, "ex:A a owl:Class ; rdfs:label \"Valve\" .\n");
    assert!(g2 > g);
    // Editing a collision's case keeps it the same.
    assert_eq!(
        set(&mut k, "ex:A a owl:Class ; rdfs:label \"VALVE\" .\n"),
        g2
    );
    let g3 = set(&mut k, "ex:A a owl:Class ; rdfs:label \"Valv\" .\n");
    assert!(g3 > g2);
    // A new term, and a new own deprecation.
    let g4 = set(
        &mut k,
        "ex:A a owl:Class ; rdfs:label \"Valv\" .\nex:C a owl:Class .\n",
    );
    assert!(g4 > g3);
    let g5 = set(
        &mut k,
        "ex:A a owl:Class ; rdfs:label \"Valv\" .\nex:C a owl:Class ; owl:deprecated true .\n",
    );
    assert!(g5 > g4);
}

#[test]
fn described_generation_moves_on_a_definition_colliding_with_a_comment() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:comment \"Same.\" .\n");
    put_ttl(
        &mut k,
        "a",
        "ex:A a owl:Class ; skos:definition \"Different.\" .\n",
    );
    let g = k.described_generation();
    // Only the definition's text changes: B's comment is its definition.
    put_ttl(
        &mut k,
        "a",
        "ex:A a owl:Class ; skos:definition \"same.\" .\n",
    );
    assert!(k.described_generation() > g);
}

#[test]
fn described_generation_moves_when_a_colliding_label_goes_with_its_document() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
    // A is typed, and labelled in another language, in c.ttl; d.ttl
    // only gives it the colliding label.
    put_ttl(
        &mut k,
        "c",
        "ex:A a owl:Class ; rdfs:label \"Ventil\"@de .\n",
    );
    put_ttl(&mut k, "d", "ex:A rdfs:label \"Valve\" .\n");
    let g = k.described_generation();
    k.remove("file:///d.ttl");
    assert!(k.described_generation() > g);
}

#[test]
fn described_generation_moves_when_a_term_changes_kind() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Pump\" .\n");
    put_ttl(&mut k, "a", "ex:A a owl:Class ; rdfs:label \"Pump\" .\n");
    let g = k.described_generation();
    put_ttl(
        &mut k,
        "a",
        "ex:A a owl:ObjectProperty ; rdfs:label \"Pump\" .\n",
    );
    let g2 = k.described_generation();
    assert!(g2 > g);
    assert_eq!(k.term_kind("http://ex.org/A"), TERM_PROPERTY);
    // A sub-property statement in another document adds to the kind.
    put_ttl(&mut k, "c", "ex:B rdfs:subPropertyOf ex:p .\n");
    assert!(k.described_generation() > g2);
    assert_eq!(k.term_kind("http://ex.org/B"), TERM_CLASS | TERM_PROPERTY);
    // An external source says nothing about kinds.
    let none = Arc::new(PrefixMap::default());
    let ext = format!("{CURATION_HEADER}ex:A a skos:Concept .\n");
    k.add_external("import:e", analysis("import:e", &ext, &none));
    assert_eq!(k.term_kind("http://ex.org/A"), TERM_PROPERTY);
}

#[test]
fn described_generation_moves_when_another_document_labels_or_defines_a_term() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "z", "ex:A a owl:Class .\n");
    put_ttl(&mut k, "b", "ex:A rdfs:seeAlso ex:x .\n");
    let set = |k: &mut Knowledge, body: &str| {
        put_ttl(k, "b", &format!("ex:A rdfs:seeAlso ex:x{body} .\n"));
        k.described_generation()
    };
    let g = k.described_generation();
    let g1 = set(&mut k, " ; rdfs:label \"A\"");
    assert!(g1 > g);
    // Typing inside the label.
    assert_eq!(set(&mut k, " ; rdfs:label \"Ab\""), g1);
    assert_eq!(set(&mut k, " ; rdfs:label \"Abc\"@en"), g1);
    let g2 = set(&mut k, " ; rdfs:label \"Abc\" ; rdfs:comment \"A\"");
    assert!(g2 > g1);
    // Typing inside the comment.
    assert_eq!(
        set(&mut k, " ; rdfs:label \"Abc\" ; rdfs:comment \"An a\""),
        g2
    );
    // The comment goes.
    assert!(set(&mut k, " ; rdfs:label \"Abc\"") > g2);
}

#[test]
fn described_generation_moves_when_the_reporting_document_changes() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "z", "ex:D a owl:Class .\n");
    put_ttl(&mut k, "a", "ex:D rdfs:seeAlso ex:x .\n");
    let g = k.described_generation();
    // a.ttl, before z.ttl by URI, starts declaring D: it reports now.
    put_ttl(&mut k, "a", "ex:D rdfs:subClassOf ex:x .\n");
    assert!(k.described_generation() > g);
}

#[test]
fn described_generation_moves_when_an_ontology_header_changes_elsewhere() {
    let mut k = Knowledge::default();
    put_ttl(&mut k, "o", "ex:onto a owl:Ontology .\n");
    put_ttl(&mut k, "h", "ex:onto rdfs:seeAlso ex:x .\n");
    let set = |k: &mut Knowledge, body: &str| {
        put_ttl(k, "h", &format!("ex:onto rdfs:seeAlso ex:x{body} .\n"));
        k.described_generation()
    };
    let g = k.described_generation();
    let license = " ; <http://purl.org/dc/terms/license> <http://l>";
    let g1 = set(&mut k, license);
    assert!(g1 > g);
    // Another licence changes nothing the checks read.
    assert_eq!(set(&mut k, &format!("{license}, <http://m>")), g1);
    let g2 = set(&mut k, &format!("{license} ; rdfs:label \"O\""));
    assert!(g2 > g1);
    assert_eq!(set(&mut k, &format!("{license} ; rdfs:label \"Onto\"")), g2);
    // An ontology typed in a document before o.ttl by URI moves the report.
    put_ttl(&mut k, "a", "ex:onto a owl:Ontology .\n");
    assert!(k.described_generation() > g2);
}

/// A shape as lines of text: its node, statement and targets, then one
/// line per property constraint, with spans as the text they cover.
fn describe(text: &str, s: &Shape) -> Vec<String> {
    let at = |sp: Span| &text[sp.start as usize..sp.end as usize];
    let short = |i: &Iri| i.replace("http://example.com/", "ex:");
    let mut out = vec![format!(
        "{} {:?} targets={:?}",
        s.node.as_ref().map_or("[]".into(), short),
        at(s.span).lines().next().unwrap_or(""),
        s.targets.iter().map(short).collect::<Vec<_>>(),
    )];
    for c in &s.properties {
        out.push(format!(
            "{:?} path={:?} min={:?} max={:?} class={:?} datatype={:?} in={:?}",
            at(c.span),
            c.path.as_ref().map(short),
            c.min_count,
            c.max_count,
            c.class.iter().map(short).collect::<Vec<_>>(),
            c.datatype.as_ref().map(short),
            c.in_values,
        ));
    }
    out
}

/// `sh:property [ ]` on a class-targeting node shape is an empty
/// constraint of it; a node shape without a target targets nothing, nor
/// do its property shapes. Pinned before 33c.
#[test]
fn property_shapes_of_targeting_and_untargeted_shapes() {
    let none = Arc::new(PrefixMap::default());
    let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.com/> .
ex:C a rdfs:Class .
ex:S a sh:NodeShape ; sh:targetClass ex:C ;
  sh:property [ sh:path ex:p ; sh:minCount 1 ; sh:class ex:K ] , [ ] .
ex:U a sh:NodeShape ;
  sh:property [ sh:path ex:q ; sh:minCount 1 ] .
ex:V sh:property [ sh:path ex:r ; sh:maxCount 2 ] .
";
    let mut k = Knowledge::default();
    k.set("file:///s.ttl", Source::Open, analysis("s", text, &none));
    let found: Vec<String> = k
        .shapes_targeting("http://example.com/C")
        .iter()
        .flat_map(|(_, s)| describe(text, s))
        .collect();
    assert_eq!(
        found,
        [
            r#"ex:S "ex:S a sh:NodeShape ; sh:targetClass ex:C ;" targets=["ex:C"]"#,
            r#""[ sh:path ex:p ; sh:minCount 1 ; sh:class ex:K ]" path=Some("ex:p") min=Some(1) max=None class=["ex:K"] datatype=None in=None"#,
            r#""[ ]" path=None min=None max=None class=[] datatype=None in=None"#,
        ]
    );
    for class in ["U", "V", "p", "q", "r", "K", "S"] {
        let iri = format!("http://example.com/{class}");
        assert!(k.shapes_targeting(&iri).is_empty(), "{class}");
    }
    assert_eq!(k.slot(0).facts.shapes.len(), 1);
}

/// The [`Shape`]s of `a` as the SHACL model has them: the shapes of
/// [`model::Shapes`] over a graph of the document's triples alone,
/// narrowed by the definition on [`Shape`], in the order the document
/// first names each one's target class or types it `sh:NodeShape`. The
/// builder must find exactly these; it reads the parse because this
/// costs more per edit (owner decision 8). Returns the shapes and the
/// shape indexes by target class.
fn shapes_from_the_model(a: &Analysis) -> (Vec<Shape>, IriMap<Vec<u32>>) {
    use crate::rdf::graph::{Graph, Id, Term};
    use crate::shacl::model;
    use crate::shacl::vocab::Param;
    use crate::syntax::Triple;
    fn is_node(g: &Graph, id: Id) -> bool {
        matches!(g.term(id), Term::Iri(_) | Term::Blank(_))
    }
    // Every rdf:first of a cell, the last rdf:rest, at most 10,000 cells.
    fn in_list<'t>(g: &Graph, doc: &dyn Fn(u32) -> &'t Triple, head: Id) -> Vec<Node> {
        let v = vocab::get();
        let mut out = Vec::new();
        let mut cur = head;
        for _ in 0..10_000 {
            if !is_node(g, cur) || g.as_iri(cur) == Some(v.rdf_nil) {
                break;
            }
            let mut next = None;
            for (i, t) in g.out(cur) {
                if t.p == v.rdf_first {
                    out.push(doc(i).o.clone());
                } else if t.p == v.rdf_rest {
                    next = Some(t.o);
                }
            }
            match next {
                Some(n) => cur = n,
                None => break,
            }
        }
        out
    }
    let v = vocab::get();
    let triples = &a.parsed.triples;
    // As the lint's model: a triple with an undeclared prefix counts.
    let all: std::borrow::Cow<[Triple]> = if triples.iter().any(|t| t.unresolved) {
        let cleared = |t: &Triple| Triple {
            unresolved: false,
            ..t.clone()
        };
        triples.iter().map(cleared).collect()
    } else {
        triples.into()
    };
    let g = Graph::new(&[&*all]);
    let shapes = model::Shapes::new(&g);
    let doc = |n: u32| &triples[g.origin(n).1 as usize];
    let constraint = |shape: Id, span: Span| {
        let mut c = PropertyConstraint {
            path: None,
            min_count: None,
            max_count: None,
            class: Vec::new(),
            datatype: None,
            in_values: None,
            span,
        };
        for &i in shapes.get(shape).map_or(&[][..], |s| s.params.as_slice()) {
            let t = doc(i);
            let count = || match &t.o {
                Node::Literal { value, .. } => value.trim().parse::<u32>().ok(),
                _ => None,
            };
            if t.p == v.path {
                if let Node::Iri(o) = &t.o {
                    c.path = Some(*o);
                }
                continue;
            }
            match (v.param(t.p), &t.o) {
                (Some(Param::Class), Node::Iri(o)) => c.class.push(*o),
                (Some(Param::Datatype), Node::Iri(o)) => c.datatype = Some(*o),
                (Some(Param::MinCount), _) => c.min_count = count(),
                (Some(Param::MaxCount), _) => c.max_count = count(),
                (Some(Param::In), _) => c.in_values = Some(in_list(&g, &doc, g.triple_at(i).o)),
                _ => {}
            }
        }
        c
    };
    let mut kept: Vec<(u32, Shape)> = Vec::new();
    for s in &shapes.shapes {
        let iri = g.as_iri(s.node);
        // The first triple that names a target class, or the type
        // sh:NodeShape of an IRI.
        let mut first = u32::MAX;
        let mut targets = Vec::new();
        for &i in &s.targets {
            let t = g.triple_at(i);
            if t.p == v.target_class {
                first = first.min(i);
                targets.extend(g.as_iri(t.o));
            }
        }
        if let Some(iri) = iri
            && s.typed_node_shape
        {
            let mut is_class = false;
            for (i, t) in g.out(s.node).filter(|(_, t)| t.p == v.rdf_type) {
                match g.as_iri(t.o) {
                    Some(o) if o == v.node_shape => first = first.min(i),
                    Some(o) if o == v.rdfs_class || o == v.owl_class => is_class = true,
                    _ => {}
                }
            }
            if is_class {
                targets.push(iri);
            }
        }
        if targets.is_empty() {
            continue;
        }
        targets.sort_unstable();
        targets.dedup();
        let properties = s
            .params
            .iter()
            .map(|&i| (i, g.triple_at(i)))
            .filter(|(_, t)| t.p == v.property && is_node(&g, t.o))
            .map(|(i, t)| constraint(t.o, doc(i).o_span))
            .collect();
        let span = g
            .out(s.node)
            .next()
            .and_then(|(i, _)| a.parsed.statements.get(doc(i).statement as usize))
            .map_or_else(Span::default, |st| st.span);
        let shape = Shape {
            node: iri,
            span,
            targets,
            properties,
        };
        kept.push((first, shape));
    }
    kept.sort_unstable_by_key(|k| k.0);
    let mut by_target = IriMap::<Vec<u32>>::default();
    for (index, (_, shape)) in kept.iter().enumerate() {
        for &t in &shape.targets {
            by_target.entry(t).or_default().push(index as u32);
        }
    }
    (kept.into_iter().map(|k| k.1).collect(), by_target)
}

/// The builder's shapes of `text` are the model's.
fn same_shapes_as_the_model(name: &str, text: &str, external: &Arc<PrefixMap>) -> usize {
    let a = Analysis::new(name.into(), text, external.clone(), false);
    let f = DocFacts::new(&a);
    let (shapes, by_target) = shapes_from_the_model(&a);
    assert_eq!(f.shapes, shapes, "{name}");
    assert_eq!(*f.shapes_by_target, *by_target, "{name}");
    shapes.len()
}

/// Shapes the narrower definition keeps or drops, and the corners of
/// the builder's reading: blank and property shapes with `sh:targetClass`,
/// literal targets, implicit class targets, repeated parameters,
/// malformed and cyclic `sh:in` lists, an `sh:property` IRI with no
/// triples, an undeclared prefix, and shapes that the model finds
/// earlier than their target (by `sh:targetNode`, `sh:node`, `sh:path`).
const CORNERS: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.com/> .
ex:Late sh:targetNode ex:x .
ex:Ref sh:node ex:Late2 .
ex:Late2 sh:class ex:K .
ex:PathFirst sh:path ex:p .
ex:A a sh:NodeShape ; sh:targetClass ex:C , ex:C , ex:D , \"lit\" ;
  sh:property [ sh:path ex:p ; sh:path ex:p2 ; sh:path ( ex:s1 ex:s2 ) ;
                sh:minCount 1 ; sh:minCount \"x\" ; sh:maxCount \" 3 \" ;
                sh:class ex:K1 , ex:K2 ; sh:datatype xsd:string ; sh:datatype \"no\" ;
                sh:in ( ex:a \"b\" 1 ) ; sh:in ( ex:c ) ] ,
              [ ] , ex:Empty , ex:Named , \"literal\" ;
  sh:property [ sh:in _:cycle ] , [ sh:in _:forked ] , [ sh:in \"head\" ] , [ sh:in rdf:nil ] .
_:cycle rdf:first ex:loop ; rdf:rest _:cycle .
_:forked rdf:first ex:f1 , ex:f2 ; rdf:rest _:gap , rdf:nil .
_:gap rdf:rest ( ex:after ) .
ex:Named sh:path ex:named ; sh:maxCount 1 .
ex:Implicit a sh:NodeShape , rdfs:Class ; sh:property [ sh:path ex:i ] .
ex:OwlImplicit a owl:Class , sh:NodeShape .
ex:NotAClass a sh:NodeShape ; sh:property [ sh:path ex:n ] .
[ a sh:NodeShape , rdfs:Class ; sh:targetClass ex:B ; sh:property [ sh:path ex:b ] ] .
[ a sh:NodeShape , rdfs:Class ] .
[ sh:path ex:pp ; sh:targetClass ex:C ] .
ex:LiteralOnly sh:targetClass \"C\" .
ex:Late sh:targetClass ex:L ; sh:property ex:Named .
ex:Late2 sh:targetClass ex:L .
ex:PathFirst sh:targetClass ex:L .
ex:Undeclared sh:targetClass nope:C ; sh:property [ sh:path nope:p ; sh:class nope:K ] .
ex:Typed sh:targetClass ex:T .
ex:Typed a sh:NodeShape .
ex:Shared sh:targetClass ex:S1 ; sh:property _:shared .
ex:Shared2 sh:targetClass ex:S2 ; sh:property _:shared .
_:shared sh:path ex:sh ; sh:minCount 0 .
";

#[test]
fn the_builder_finds_the_shapes_of_the_model_that_target_classes() {
    let none = Arc::new(PrefixMap::default());
    let project = project_prefixes();
    assert_eq!(same_shapes_as_the_model("corners.ttl", CORNERS, &none), 12);
    // Every fixture, cut short and renamed as the incremental test does.
    let texts = fixtures();
    let mut found = 0;
    for (i, t) in texts.iter().enumerate() {
        for ext in [&none, &project] {
            found += same_shapes_as_the_model(&format!("f{i}.ttl"), t, ext);
        }
    }
    assert!(found > 0);
    let mut rng = Rng(0x5EED_33C0);
    let mut corners = vec![CORNERS.to_string()];
    corners.extend(texts.iter().cloned());
    for i in 0..300 {
        let t = variant(&mut rng, &corners);
        same_shapes_as_the_model(&format!("v{i}.ttl"), &t, &project);
    }
    // The other shape fixtures, in Turtle and JSON-LD.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    for f in [
        "shacl/vocab.ttl",
        "nested-prefixes/shapes/person-shape.ttl",
        "chaos/select-shapes.ttl",
        "chaos/shapes.jsonld",
    ] {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        same_shapes_as_the_model(f, &text, &none);
    }
    // The public corpus, when it is downloaded (tools/corpus/fetch.sh). In
    // CI (`CI=true`) a missing file fails the test, as in
    // tests/support/corpus.rs, so a run that skipped it is not `ok`.
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
    let ci = std::env::var("CI").is_ok_and(|v| v == "true");
    for f in ["brick.ttl", "dcat-ap-shacl.ttl", "w3c-shacl.ttl"] {
        match std::fs::read_to_string(corpus.join(f)) {
            Ok(text) => {
                same_shapes_as_the_model(f, &text, &none);
            }
            Err(e) => assert!(
                !ci,
                "test/corpus/{f}: {e} (run tools/corpus/fetch.sh, or npm run corpus); \
                     CI=true does not skip the corpus"
            ),
        }
    }
}

/// A shapes document of `n` node shapes with three property shapes each.
fn shapes_document(n: usize) -> String {
    let mut text = String::from(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n",
    );
    for i in 0..n {
        text.push_str(&format!(
            "ex:S{i} a sh:NodeShape ; sh:targetClass ex:C{i} ;\n\
                 \x20 sh:property [ sh:path ex:p{i} ; sh:minCount 1 ; sh:datatype ex:d ] ,\n\
                 \x20   [ sh:path ex:q{i} ; sh:class ex:C ; sh:maxCount 2 ] ,\n\
                 \x20   [ sh:path ex:r{i} ; sh:in ( ex:a ex:b \"c\" ) ] .\n\
                 ex:x{i} a ex:C{i} ; ex:p{i} \"v\" ; ex:q{i} ex:x0 .\n"
        ));
    }
    text
}

/// Per edit, the shapes cost one pass over the parse and no graph; the
/// model indexes every triple of the document into one.
#[test]
fn the_shapes_build_no_graph_per_edit() {
    use crate::rdf::graph::INDEXED;
    let none = Arc::new(PrefixMap::default());
    let a = Analysis::new("s.ttl".into(), &shapes_document(50), none, false);
    let indexed = || INDEXED.with(|c| c.get());
    let before = indexed();
    assert_eq!(DocFacts::new(&a).shapes.len(), 50);
    assert_eq!(indexed() - before, 0);
    assert_eq!(shapes_from_the_model(&a).0.len(), 50);
    assert_eq!(indexed() - before, a.parsed.triples.len() as u64);
}

/// The measurement behind owner decision 8: the builder against a graph
/// of the document plus its model, the best of seven tries each. On the
/// generated 2,000-shape document (44,000 triples) the model cost 1.5
/// to 2.3 times the builder, in the shipped profile and in CI's (LTO
/// off, 16 codegen units), on a loaded M-series laptop: builder 0.80 to
/// 1.23 ms, model 1.81 to 2.00 ms. That document is asserted. At
/// 33c Brick (53,646 triples) cost 0.52 ms against 1.26 ms, so the
/// builder stays. The corpus files, when present, are printed only.
#[cfg(not(debug_assertions))]
#[test]
fn timing_the_shape_builder_costs_less_than_the_model() {
    use crate::rdf::graph::Graph;
    let none = Arc::new(PrefixMap::default());
    let mut texts = vec![("generated".to_string(), shapes_document(2_000))];
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
    for f in ["brick.ttl", "dcat-ap-shacl.ttl"] {
        if let Ok(text) = std::fs::read_to_string(corpus.join(f)) {
            texts.push((f.to_string(), text));
        }
    }
    let best = |f: &dyn Fn()| {
        (0..7)
            .map(|_| {
                let t0 = std::time::Instant::now();
                f();
                t0.elapsed()
            })
            .min()
            .unwrap()
    };
    let v = vocab::get();
    for (name, text) in &texts {
        let a = Analysis::new(name.clone(), text, none.clone(), false);
        let triples = &a.parsed.triples;
        let nodes: Vec<Subj> = triples
            .iter()
            .filter_map(|t| match (&t.s, &t.o) {
                (s, _) if t.p == v.target_class => subj(s),
                (Node::Iri(s), Node::Iri(o)) if t.p == v.rdf_type && *o == v.node_shape => {
                    Some(Subj::Iri(*s))
                }
                _ => None,
            })
            .collect();
        let builder = best(&|| {
            let mut f = DocFacts::default();
            f.shapes(&a, nodes.clone());
            std::hint::black_box(f);
        });
        let graph = best(&|| {
            std::hint::black_box(Graph::new(&[triples]));
        });
        let model = best(&|| {
            std::hint::black_box(shapes_from_the_model(&a));
        });
        let analysis = best(&|| {
            std::hint::black_box(Analysis::new(name.clone(), text, none.clone(), false));
        });
        eprintln!(
            "{name}: {} triples; builder {builder:?}; model {model:?} (graph {graph:?}); Analysis::new {analysis:?}",
            triples.len()
        );
        if name == "generated" {
            assert!(builder < model, "builder {builder:?}, model {model:?}");
        }
    }
}
