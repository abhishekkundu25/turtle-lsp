//! Formatter properties over every fixture (and the private corpus when it is
//! present): the output is a fixed point, parses to the same quads as the
//! input (by our parser and by oxttl's Turtle, TriG and N-Triples parsers) and
//! keeps every comment.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use turtle_lsp::features::format::{Options, format};
use turtle_lsp::syntax::lexer::{Kind, tokenize};
use turtle_lsp::syntax::{PrefixMap, Severity, Syntax, parse};

include!("support/corpus.rs");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures")
}

/// The Turtle-family syntax of a fixture (JSON-LD has its own test below).
fn syntax_of(path: &Path) -> Option<Syntax> {
    Syntax::from_extension(&path.to_string_lossy()).filter(|s| *s != Syntax::JsonLd)
}

/// Every `.ttl`, `.trig`, `.nt` and `.nq` file under `dir`.
fn rdf_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rdf_files(&p, out);
        } else if syntax_of(&p).is_some() {
            out.push(p);
        }
    }
}

/// The `prefixes.ttl` that applies to a fragment, if any (none for N-Triples
/// and N-Quads, which have no directives).
fn header_for(path: &Path) -> String {
    if syntax_of(path).is_some_and(Syntax::is_line_based) {
        return String::new();
    }
    let root = fixtures();
    let mut dir = path.parent();
    while let Some(d) = dir {
        let h = d.join("prefixes.ttl");
        if h.exists() && h != path {
            return fs::read_to_string(h).unwrap();
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    String::new()
}

fn ours(src: &str, syntax: Syntax) -> Vec<String> {
    let mut v: Vec<String> = parse(src, &PrefixMap::default(), syntax)
        .triples
        .iter()
        .map(|t| format!("{:?} {:?} {:?} {:?}", t.s, t.p, t.o, t.graph))
        .collect();
    v.sort();
    v
}

/// N-Quads lines from oxttl with blank nodes renamed in order of appearance
/// (the formatter keeps token order, so both sides see them in the same order).
fn oxttl(src: &str, syntax: Syntax) -> Result<Vec<String>, String> {
    let e = |e: oxttl::TurtleSyntaxError| e.to_string();
    let quads: Vec<oxrdf::Quad> = match syntax {
        Syntax::Turtle => oxttl::TurtleParser::new()
            .for_slice(src)
            .map(|t| {
                t.map(|t| t.in_graph(oxrdf::GraphName::DefaultGraph))
                    .map_err(e)
            })
            .collect::<Result<_, _>>()?,
        Syntax::TriG => oxttl::TriGParser::new()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect::<Result<_, _>>()?,
        Syntax::NQuads => oxttl::NQuadsParser::new()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect::<Result<_, _>>()?,
        Syntax::NTriples => oxttl::NTriplesParser::new()
            .for_slice(src)
            .map(|t| {
                t.map(|t| t.in_graph(oxrdf::GraphName::DefaultGraph))
                    .map_err(e)
            })
            .collect::<Result<_, _>>()?,
        Syntax::JsonLd => unreachable!("oxttl reads no JSON-LD"),
    };
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for t in quads {
        let s = match &t.subject {
            oxrdf::NamedOrBlankNode::NamedNode(n) => n.to_string(),
            oxrdf::NamedOrBlankNode::BlankNode(b) => bnode(&mut ids, b.as_str()),
        };
        let o = term(&mut ids, &t.object);
        let g = match &t.graph_name {
            oxrdf::GraphName::DefaultGraph => String::new(),
            oxrdf::GraphName::NamedNode(n) => format!(" {n}"),
            oxrdf::GraphName::BlankNode(b) => format!(" {}", bnode(&mut ids, b.as_str())),
        };
        out.push(format!("{s} {} {o}{g} .", t.predicate));
    }
    out.sort();
    Ok(out)
}

fn bnode(ids: &mut HashMap<String, usize>, id: &str) -> String {
    let n = ids.len();
    format!("_:b{}", ids.entry(id.to_string()).or_insert(n))
}

fn term(ids: &mut HashMap<String, usize>, t: &oxrdf::Term) -> String {
    use oxrdf::Term;
    match t {
        Term::NamedNode(n) => n.to_string(),
        Term::BlankNode(b) => bnode(ids, b.as_str()),
        Term::Literal(l) => l.to_string(),
        Term::Triple(tr) => {
            let s = match &tr.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => n.to_string(),
                oxrdf::NamedOrBlankNode::BlankNode(b) => bnode(ids, b.as_str()),
            };
            format!("<<( {s} {} {} )>>", tr.predicate, term(ids, &tr.object))
        }
    }
}

fn comments(src: &str) -> Vec<String> {
    let mut v: Vec<String> = tokenize(src)
        .iter()
        .filter(|t| t.kind == Kind::Comment)
        .map(|t| t.text(src).trim_end().to_string())
        .collect();
    v.sort();
    v
}

fn fmt(src: &str, o: &Options) -> Option<String> {
    fmt_as(src, Syntax::Turtle, o)
}

fn fmt_as(src: &str, syntax: Syntax, o: &Options) -> Option<String> {
    format(
        src,
        &parse(src, &PrefixMap::default(), syntax),
        &PrefixMap::default(),
        o,
    )
}

fn option_sets() -> Vec<Options> {
    vec![
        Options::default(),
        Options {
            tab_size: 2,
            insert_spaces: false,
            line_width: 40,
        },
        Options {
            tab_size: 2,
            insert_spaces: true,
            line_width: 160,
        },
    ]
}

fn check_file(path: &Path) -> bool {
    let name = path.strip_prefix(fixtures()).unwrap().display().to_string();
    let src = fs::read_to_string(path).unwrap();
    let syntax = syntax_of(path).unwrap();
    check_text(&name, &src, syntax, &header_for(path))
}

fn check_text(name: &str, src: &str, syntax: Syntax, header: &str) -> bool {
    let parsed = parse(src, &PrefixMap::default(), syntax);
    if parsed.errors.iter().any(|e| e.severity == Severity::Error) {
        assert!(
            fmt_as(src, syntax, &Options::default()).is_none(),
            "{name}: formatted despite syntax errors"
        );
        return false;
    }
    let ox_before = oxttl(&(header.to_string() + src), syntax);
    if let Err(e) = &ox_before {
        eprintln!("{name}: oxttl rejects the input, comparing with our parser only: {e}");
    }
    for o in option_sets() {
        let out = fmt_as(src, syntax, &o)
            .unwrap_or_else(|| panic!("{name}: refused a valid file ({o:?})"));
        let again =
            fmt_as(&out, syntax, &o).unwrap_or_else(|| panic!("{name}: refused its own output"));
        assert!(
            again == out,
            "{name}: not idempotent ({o:?})\n--- first ---\n{out}\n--- second ---\n{again}"
        );
        assert_eq!(
            ours(src, syntax),
            ours(&out, syntax),
            "{name}: quads differ ({o:?})"
        );
        assert_eq!(comments(src), comments(&out), "{name}: comments differ");
        if let Ok(before) = &ox_before {
            let after = oxttl(&(header.to_string() + &out), syntax)
                .unwrap_or_else(|e| panic!("{name}: oxttl rejects the output: {e}\n{out}"));
            assert_eq!(before, &after, "{name}: oxttl quads differ ({o:?})");
        }
    }
    true
}

#[test]
fn every_fixture_formats_to_a_fixed_point_with_the_same_graph_and_comments() {
    let mut files = Vec::new();
    rdf_files(&fixtures(), &mut files);
    files.sort();
    let checked: Vec<&PathBuf> = files.iter().filter(|p| check_file(p)).collect();
    eprintln!(
        "{} of {} fixtures formatted and checked",
        checked.len(),
        files.len()
    );
    assert!(
        checked.len() >= 10,
        "only {} fixtures checked",
        checked.len()
    );
    for syntax in [Syntax::TriG, Syntax::NTriples] {
        let n = checked
            .iter()
            .filter(|p| syntax_of(p) == Some(syntax))
            .count();
        assert!(n >= 3, "only {n} {syntax:?} fixtures checked");
    }
}

#[test]
fn refuses_documents_with_syntax_errors() {
    let src = fs::read_to_string(fixtures().join("format/broken.ttl")).unwrap();
    assert!(fmt(&src, &Options::default()).is_none());
}

#[test]
fn messy_fixtures_match_the_golden_files() {
    let dir = fixtures().join("format");
    for (ext, syntax) in [
        ("ttl", Syntax::Turtle),
        ("trig", Syntax::TriG),
        ("nt", Syntax::NTriples),
    ] {
        let src = fs::read_to_string(dir.join(format!("messy.{ext}"))).unwrap();
        let got = fmt_as(&src, syntax, &Options::default()).unwrap();
        let want = fs::read_to_string(dir.join(format!("expected.{ext}"))).unwrap();
        assert_eq!(got, want, "messy.{ext}");
    }
}

#[test]
fn layout_rules() {
    let o = Options::default();
    let src = "@prefix ex: <http://example.com/> .\nPREFIX rdfs:  <http://www.w3.org/2000/01/rdf-schema#>\nex:a a ex:B;ex:p [ ex:q 1 ] , [ ex:q 2 ] .\nex:c ex:d ex:e.";
    assert_eq!(
        fmt(src, &o).unwrap(),
        "@prefix ex:  <http://example.com/> .\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         \n\
         ex:a a ex:B ;\n    ex:p [ ex:q 1 ] , [ ex:q 2 ] .\n\
         \n\
         ex:c ex:d ex:e .\n"
    );
    // Tabs, and a list too long for the line.
    let tabs = Options {
        tab_size: 4,
        insert_spaces: false,
        line_width: 30,
    };
    let src =
        "ex:a ex:p ex:one , ex:two , ex:three , ex:four ; ex:q ( ex:x ex:y ex:z ex:w ex:v ) .";
    assert_eq!(
        fmt(src, &tabs).unwrap(),
        "ex:a ex:p ex:one ,\n\t\tex:two ,\n\t\tex:three ,\n\t\tex:four ;\n\tex:q (\n\t\tex:x\n\t\tex:y\n\t\tex:z\n\t\tex:w\n\t\tex:v\n\t) .\n"
    );
}

#[test]
fn comments_keep_their_place() {
    let o = Options::default();
    let src = "# head\n\n# about a\nex:a ex:p ex:o , # first\n  ex:o2 ; # second\n  # own line\n  ex:q 1 . # end\n# tail\n";
    assert_eq!(
        fmt(src, &o).unwrap(),
        "# head\n\n# about a\nex:a ex:p ex:o , # first\n        ex:o2 ; # second\n    # own line\n    ex:q 1 . # end\n# tail\n"
    );
}

#[test]
fn late_prefix_moves_up_only_when_that_is_safe() {
    let o = Options::default();
    let src = "@prefix ex: <http://e/> .\nex:a ex:p ex:o .\n@prefix fresh: <http://f/> .\nfresh:a ex:p ex:o .\n";
    assert_eq!(
        fmt(src, &o).unwrap(),
        "@prefix ex:    <http://e/> .\n@prefix fresh: <http://f/> .\n\nex:a ex:p ex:o .\n\nfresh:a ex:p ex:o .\n"
    );
    // Redefining a prefix after a use: the directive stays where it is.
    let src = "@prefix ex: <http://e/> .\nex:a ex:p ex:o .\n@prefix ex: <http://other/> .\nex:b ex:p ex:o .\n";
    let out = fmt(src, &o).unwrap();
    assert!(out.find("<http://other/>").unwrap() > out.find("ex:a").unwrap());
    // BASE never moves.
    let src = "<a> <b> <c> .\n@base <http://b/> .\n<a> <b> <c> .\n";
    assert_eq!(
        fmt(src, &o).unwrap(),
        "<a> <b> <c> .\n\n@base <http://b/> .\n\n<a> <b> <c> .\n"
    );
}

#[test]
fn rdf12_constructs() {
    let o = Options::default();
    let src =
        "<<ex:a ex:b ex:c~ex:r>>ex:p<<(ex:x ex:y ex:z)>>.\nex:s ex:p ex:o~ex:r2{|ex:src ex:db|}.";
    assert_eq!(
        fmt(src, &o).unwrap(),
        "<< ex:a ex:b ex:c ~ ex:r >> ex:p <<( ex:x ex:y ex:z )>> .\n\nex:s ex:p ex:o ~ ex:r2 {| ex:src ex:db |} .\n"
    );
}

#[test]
fn crlf_documents_stay_crlf() {
    let src = "ex:a ex:p ex:o ;\r\n  ex:q ex:r .\r\n";
    assert_eq!(
        fmt(src, &Options::default()).unwrap(),
        "ex:a ex:p ex:o ;\r\n    ex:q ex:r .\r\n"
    );
}

/// Comments wedged between every pair of tokens: trailing after each token,
/// or on their own line before each one.
#[test]
fn comments_between_every_token_survive() {
    let mut files = Vec::new();
    rdf_files(&fixtures(), &mut files);
    for path in files {
        let syntax = syntax_of(&path).unwrap();
        // N-Triples and N-Quads allow no comment inside a statement.
        if syntax.is_line_based() {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap();
        if fmt_as(&src, syntax, &Options::default()).is_some() {
            wedge_comments(&src, syntax);
        }
    }
}

fn wedge_comments(src: &str, syntax: Syntax) {
    for own_line in [false, true] {
        let mut out = String::new();
        let mut prev = 0;
        for (i, t) in tokenize(src).iter().enumerate() {
            if t.kind == Kind::Comment {
                continue;
            }
            out.push_str(&src[prev..t.start as usize]);
            if own_line {
                out.push_str(&format!("\n# o{i}\n"));
            }
            out.push_str(t.text(src));
            if !own_line {
                out.push_str(&format!(" # t{i}\n"));
            }
            prev = t.end as usize;
        }
        out.push_str(&src[prev..]);
        for o in option_sets() {
            let f = fmt_as(&out, syntax, &o).expect("refused");
            let g = fmt_as(&f, syntax, &o).expect("refused its own output");
            if g != f {
                let dir = std::env::temp_dir();
                fs::write(dir.join("fmt-first.ttl"), &f).unwrap();
                fs::write(dir.join("fmt-second.ttl"), &g).unwrap();
                panic!(
                    "not idempotent ({o:?}); see {}",
                    dir.join("fmt-first.ttl").display()
                );
            }
            assert_eq!(comments(&out), comments(&f));
            assert_eq!(ours(&out, syntax), ours(&f, syntax));
        }
    }
}

#[test]
fn trig_graph_blocks() {
    let o = Options::default();
    let src = "@prefix ex: <http://e/> .\nex:d ex:p ex:o .\nex:g1{ex:a ex:p ex:o .ex:b ex:p 1, 2;ex:q ex:r}\ngraph ex:g2 { ex:c ex:p [ ex:q 1 ] . } {ex:e ex:p ex:o .}\n[] { ex:f ex:p ex:o }\nex:empty { }\n";
    assert_eq!(
        fmt_as(src, Syntax::TriG, &o).unwrap(),
        "@prefix ex: <http://e/> .\n\
         \n\
         ex:d ex:p ex:o .\n\
         \n\
         ex:g1 {\n    ex:a ex:p ex:o .\n\n    ex:b ex:p 1 , 2 ;\n        ex:q ex:r\n}\n\
         \n\
         graph ex:g2 {\n    ex:c ex:p [ ex:q 1 ] .\n}\n\
         \n\
         {\n    ex:e ex:p ex:o .\n}\n\
         \n\
         [] {\n    ex:f ex:p ex:o\n}\n\
         \n\
         ex:empty {\n}\n"
    );
}

#[test]
fn trig_comments_stay_inside_their_block() {
    let o = Options::default();
    let src =
        "ex:g { # about g\n  # first\n  ex:a ex:p ex:o . # end a\n\n  # last words\n} # after\n";
    assert_eq!(
        fmt_as(src, Syntax::TriG, &o).unwrap(),
        "ex:g { # about g\n    # first\n    ex:a ex:p ex:o . # end a\n\n    # last words\n} # after\n"
    );
}

#[test]
fn ntriples_one_triple_per_line() {
    let o = Options {
        tab_size: 2,
        insert_spaces: true,
        line_width: 20,
    };
    let src = "# head\n\t<http://e/a>\t<http://e/p>   \"x\"@en\t.  # t\n\n\n<http://e/a> <http://e/p> <<( _:b <http://e/q> \"y\"^^<http://e/dt> )>>.\n_:b <http://e/p> <http://e/o> .\n";
    assert_eq!(
        fmt_as(src, Syntax::NTriples, &o).unwrap(),
        "# head\n<http://e/a> <http://e/p> \"x\"@en . # t\n\n<http://e/a> <http://e/p> <<( _:b <http://e/q> \"y\"^^<http://e/dt> )>> .\n_:b <http://e/p> <http://e/o> .\n"
    );
}

/// The W3C TriG and N-Triples suites (RDF 1.1 and 1.2): every file our parser
/// accepts formats to a fixed point with the same quads and comments.
#[test]
fn w3c_trig_and_ntriples_files_format_safely() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/rdf-tests");
    let mut files = Vec::new();
    for dir in [
        "rdf11/rdf-trig",
        "rdf11/rdf-n-triples",
        "rdf12/rdf-trig",
        "rdf12/rdf-n-triples",
    ] {
        rdf_files(&root.join(dir), &mut files);
    }
    files.sort();
    let mut checked = 0;
    for p in &files {
        let syntax = syntax_of(p).unwrap();
        if syntax == Syntax::Turtle {
            continue;
        }
        let name = p.strip_prefix(&root).unwrap().display().to_string();
        let src = fs::read_to_string(p).unwrap();
        checked += usize::from(check_text(&name, &src, syntax, ""));
    }
    eprintln!(
        "{checked} of {} W3C files formatted and checked",
        files.len()
    );
    assert!(checked >= 100, "only {checked} W3C files checked");
}

#[test]
fn relative_iris_format_when_parsed_with_a_base() {
    let src = "<> a <Thing> .\n<a>   <b> <c> .\n";
    let parsed = turtle_lsp::syntax::parse_with_base(
        src,
        &PrefixMap::default(),
        Syntax::Turtle,
        Some("http://example.com/doc"),
    );
    assert_eq!(
        format(src, &parsed, &PrefixMap::default(), &Options::default()).as_deref(),
        Some("<> a <Thing> .\n\n<a> <b> <c> .\n")
    );
}

/// Every JSON-LD document of the private corpus that parses without JSON
/// errors formats to the same JSON value and formats again to itself.
#[test]
fn jsonld_corpus_round_trip() {
    let Some(dir) = corpus_path("jsonld") else {
        return;
    };
    let opts = Options {
        tab_size: 2,
        ..Options::default()
    };
    let (mut seen, mut formatted, mut unchanged) = (0, 0, 0);
    let mut files = Vec::new();
    collect_jsonld(&dir, &mut files);
    for p in files {
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let doc = turtle_lsp::jsonld::json::parse(&text);
        if !doc.errors.is_empty() || doc.root.is_none() {
            continue;
        }
        seen += 1;
        let once = turtle_lsp::jsonld::format::format(&text, &opts)
            .unwrap_or_else(|| panic!("{} was refused", p.display()));
        let a = turtle_lsp::jsonld::json::parse(&text).root.unwrap();
        let b = turtle_lsp::jsonld::json::parse(&once);
        assert!(b.errors.is_empty(), "{}", p.display());
        assert_eq!(strip(&a), strip(&b.root.unwrap()), "{}", p.display());
        let twice = turtle_lsp::jsonld::format::format(&once, &opts).unwrap();
        assert_eq!(once, twice, "not idempotent: {}", p.display());
        if once == text {
            unchanged += 1;
        } else {
            formatted += 1;
        }
    }
    eprintln!(
        "jsonld corpus: {seen} documents, {formatted} reformatted, {unchanged} already formatted"
    );
}

fn collect_jsonld(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_jsonld(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonld" || x == "json") {
            out.push(p);
        }
    }
}

/// A span-free rendering of a JSON tree, for comparing values.
fn strip(j: &turtle_lsp::jsonld::json::Json) -> String {
    use turtle_lsp::jsonld::json::Value;
    match &j.value {
        Value::Array(a) => format!(
            "[{}]",
            a.items.iter().map(strip).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.members
                .iter()
                .map(|m| format!("{:?}:{}", m.key, strip(&m.value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        v => format!("{v:?}"),
    }
}

#[test]
fn a_redundant_semicolon_with_a_comment_is_dropped_in_one_pass() {
    let o = Options::default();
    let cases = [
        (
            Syntax::Turtle,
            "<a> <p> <o> ; # c\n.\n",
            "<a> <p> <o> . # c\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> <o> ;\n  # c\n  ;\n.\n",
            "<a> <p> <o>\n# c\n.\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> <o> # c1\n ; # c2\n .\n",
            "<a> <p> <o> # c1\n# c2\n.\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> [ <q> <r> ; # c\n ] .\n",
            "<a> <p> [\n        <q> <r> # c\n    ] .\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> <o> ; # c1\n ; # c2\n <q> <r> .\n",
            "<a> <p> <o> ; # c1\n    # c2\n    <q> <r> .\n",
        ),
        (
            Syntax::TriG,
            "<g> { <a> <p> <o> ; # c\n }\n",
            "<g> {\n    <a> <p> <o> # c\n}\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> <o> {| <q> <r> ; # c\n |} .\n",
            "<a> <p> <o> {|\n        <q> <r> # c\n    |} .\n",
        ),
        // No blank line the author did not write.
        (
            Syntax::Turtle,
            "<a> <p> <o>\n;\n# c\n.\n",
            "<a> <p> <o>\n# c\n.\n",
        ),
        // The comment is on the `;` line: the `.` does not go on a line alone.
        (
            Syntax::Turtle,
            "<a> <p> <o>\n ; # c\n .\n",
            "<a> <p> <o> . # c\n",
        ),
        (
            Syntax::Turtle,
            "<a> <p> <o> ; # c1\n.\n<b> <p> <o> ; # c2\n.\n",
            "<a> <p> <o> . # c1\n\n<b> <p> <o> . # c2\n",
        ),
    ];
    for (syntax, src, want) in cases {
        let once = fmt_as(src, syntax, &o).unwrap();
        assert_eq!(once, want, "{src:?}");
        assert_eq!(fmt_as(&once, syntax, &o).unwrap(), once, "{src:?}");
    }
}
