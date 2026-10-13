//! Formatter properties over every fixture (and the private corpus when it is
//! present): the output is a fixed point, parses to the same triples as the
//! input (by our parser and by oxttl) and keeps every comment.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use turtle_lsp::features::format::{Options, format};
use turtle_lsp::syntax::lexer::{Kind, tokenize};
use turtle_lsp::syntax::{PrefixMap, Severity, parse};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures")
}

fn ttl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            ttl_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "ttl") {
            out.push(p);
        }
    }
}

/// The `prefixes.ttl` that applies to a fragment, if any.
fn header_for(path: &Path) -> String {
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

fn ours(src: &str) -> Vec<String> {
    let mut v: Vec<String> = parse(src, &PrefixMap::default())
        .triples
        .iter()
        .map(|t| format!("{:?} {:?} {:?}", t.s, t.p, t.o))
        .collect();
    v.sort();
    v
}

/// N-Triples lines from oxttl with blank nodes renamed in order of appearance
/// (the formatter keeps token order, so both sides see them in the same order).
fn oxttl(src: &str) -> Result<Vec<String>, String> {
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for t in oxttl::TurtleParser::new().for_slice(src) {
        let t = t.map_err(|e| e.to_string())?;
        let s = match &t.subject {
            oxrdf::NamedOrBlankNode::NamedNode(n) => n.to_string(),
            oxrdf::NamedOrBlankNode::BlankNode(b) => bnode(&mut ids, b.as_str()),
        };
        let o = term(&mut ids, &t.object);
        out.push(format!("{s} {} {o} .", t.predicate));
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
    format(
        src,
        &parse(src, &PrefixMap::default()),
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
    let parsed = parse(&src, &PrefixMap::default());
    if parsed.errors.iter().any(|e| e.severity == Severity::Error) {
        assert!(
            fmt(&src, &Options::default()).is_none(),
            "{name}: formatted despite syntax errors"
        );
        return false;
    }
    let header = header_for(path);
    let ox_before = oxttl(&(header.clone() + &src));
    if let Err(e) = &ox_before {
        eprintln!("{name}: oxttl rejects the input, comparing with our parser only: {e}");
    }
    for o in option_sets() {
        let out = fmt(&src, &o).unwrap_or_else(|| panic!("{name}: refused a valid file ({o:?})"));
        let again = fmt(&out, &o).unwrap_or_else(|| panic!("{name}: refused its own output"));
        assert!(
            again == out,
            "{name}: not idempotent ({o:?})\n--- first ---\n{out}\n--- second ---\n{again}"
        );
        assert_eq!(ours(&src), ours(&out), "{name}: triples differ ({o:?})");
        assert_eq!(comments(&src), comments(&out), "{name}: comments differ");
        if let Ok(before) = &ox_before {
            let after = oxttl(&(header.clone() + &out))
                .unwrap_or_else(|e| panic!("{name}: oxttl rejects the output: {e}\n{out}"));
            assert_eq!(before, &after, "{name}: oxttl triples differ ({o:?})");
        }
    }
    true
}

#[test]
fn every_fixture_formats_to_a_fixed_point_with_the_same_graph_and_comments() {
    let mut files = Vec::new();
    ttl_files(&fixtures(), &mut files);
    files.sort();
    let checked = files.iter().filter(|p| check_file(p)).count();
    eprintln!(
        "{checked} of {} fixtures formatted and checked",
        files.len()
    );
    assert!(checked >= 10, "only {checked} fixtures checked");
}

#[test]
fn refuses_documents_with_syntax_errors() {
    let src = fs::read_to_string(fixtures().join("format/broken.ttl")).unwrap();
    assert!(fmt(&src, &Options::default()).is_none());
}

#[test]
fn messy_fixture_matches_the_golden_file() {
    let dir = fixtures().join("format");
    let src = fs::read_to_string(dir.join("messy.ttl")).unwrap();
    let want = fs::read_to_string(dir.join("expected.ttl")).unwrap();
    assert_eq!(fmt(&src, &Options::default()).unwrap(), want);
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
    ttl_files(&fixtures(), &mut files);
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        if fmt(&src, &Options::default()).is_some() {
            wedge_comments(&src);
        }
    }
}

fn wedge_comments(src: &str) {
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
            let f = fmt(&out, &o).expect("refused");
            let g = fmt(&f, &o).expect("refused its own output");
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
            assert_eq!(ours(&out), ours(&f));
        }
    }
}
