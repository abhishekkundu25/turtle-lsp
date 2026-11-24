//! The incremental oracle: a document edited through the workspace, as a
//! client edits it, must give what a fresh workspace opened on the final
//! text gives. Every incremental path (the kept edits, the statement splice,
//! the diagnostics cache) is checked here, against the full rebuild.
//!
//! [`check`] opens a document, applies batches of LSP content changes with
//! [`Workspace::change_doc`] (asking for the analysis and the diagnostics on
//! open and after the batches that say so, as a client asks for diagnostics
//! after some keystrokes), then compares, field by field, with a fresh
//! workspace opened on the text the changes leave when applied the plain way:
//! the parse (errors included), the analysis maps, line conversions on a
//! sample of offsets, the text, the knowledge index
//! ([`crate::knowledge::Knowledge::snapshot`]) and the composed diagnostics
//! ([`diagnose::document`]), the edited document's as the server composes
//! them, with the findings it keeps between edits
//! ([`Workspace::diagnostics`]). Beside it in both workspaces is a document
//! whose terms share the labels the edited one starts with, diagnosed after
//! each analysis of the edited one: its label checks follow the edits
//! through the index's change set, and are compared too. The first
//! difference is the error, named by field. The edited workspace also
//! checks each spliced analysis against a full one as it goes
//! ([`Workspace::set_check_incremental`]), and the report says how each
//! analysis was parsed. The chaos tests run it on every iteration of their
//! edit mode.

use crate::analysis::{
    Analysis, Encoding, LineIndex, ParsePath, offset_in, position_in, same_analysis, same_list,
};
use crate::diagnose::{self, DiagnosticInputs};
use crate::settings::Settings;
use crate::syntax::{Node, PrefixMap, Syntax};
use crate::workspace::Workspace;
use lsp_types::{Diagnostic, Range, TextDocumentContentChangeEvent};

/// The changes a client sends in one `didChange`, each in the text the ones
/// before it left.
#[derive(Clone, Debug)]
pub struct Batch {
    pub changes: Vec<TextDocumentContentChangeEvent>,
    /// Ask for the analysis and the diagnostics after this batch. After the
    /// last batch they are always asked for.
    pub analyse: bool,
}

/// What an edit session that matched the full rebuild left.
#[derive(Debug)]
pub struct Report {
    /// The document's text after the last batch.
    pub text: String,
    /// How each analysis of the edited document was parsed, in order, the
    /// one on open first.
    pub paths: Vec<ParsePath>,
}

/// Apply `batches` to a `syntax` document opened on `start`, positions in
/// `enc`, and compare the result with a fresh workspace opened on the text
/// they leave. `Err` names the first field that differs. An RDF/XML
/// document is an `.owl` file: its syntax is what its text gives
/// ([`Syntax::of`]), at each change and in the fresh workspace, so an edit
/// can turn it into Turtle or JSON-LD and back.
pub fn check(
    syntax: Syntax,
    start: &str,
    batches: Vec<Batch>,
    enc: Encoding,
) -> Result<Report, String> {
    check_against(syntax, start, batches, enc, None)
}

/// [`check`], with the fresh workspace opened on `fresh` instead of the
/// edited text when it is given (the oracle's own tests).
fn check_against(
    syntax: Syntax,
    start: &str,
    batches: Vec<Batch>,
    enc: Encoding,
    fresh: Option<&str>,
) -> Result<Report, String> {
    let uri = format!("file:///oracle/doc.{}", extension(syntax));
    let other = companion(syntax, start);
    let settings = settings();
    let mut ws = Workspace::new(Vec::new(), PrefixMap::default());
    ws.set_check_incremental(true);
    open(&mut ws, &uri, start.to_string(), syntax);
    ws.open_doc_as(OTHER.into(), other.clone(), 0, Syntax::Turtle);
    // As a client asks on open: the edits then have an analysis to go stale.
    kept_diagnostics(&mut ws, &uri, enc, &settings);
    kept_diagnostics(&mut ws, OTHER, enc, &settings);
    let mut text = start.to_string();
    for (i, batch) in batches.into_iter().enumerate() {
        for ch in &batch.changes {
            apply(&mut text, ch, enc);
        }
        ws.change_doc(&uri, i as i32 + 1, batch.changes, enc);
        if batch.analyse {
            kept_diagnostics(&mut ws, &uri, enc, &settings);
            kept_diagnostics(&mut ws, OTHER, enc, &settings);
        }
    }
    let got_diags = kept_diagnostics(&mut ws, &uri, enc, &settings);
    let got_other = kept_diagnostics(&mut ws, OTHER, enc, &settings);
    let mut fresh_ws = Workspace::new(Vec::new(), PrefixMap::default());
    let want_text = fresh.unwrap_or(&text).to_string();
    open(&mut fresh_ws, &uri, want_text, syntax);
    fresh_ws.open_doc_as(OTHER.into(), other, 0, Syntax::Turtle);
    let want_diags = diagnostics(&mut fresh_ws, &uri, enc, &settings);
    let want_other = diagnostics(&mut fresh_ws, OTHER, enc, &settings);

    let (Some(got), Some(want)) = (ws.analysis(&uri), fresh_ws.analysis(&uri)) else {
        return Err("analysis: missing".into());
    };
    same_analysis(&got, &want)?;
    same_list(
        "knowledge",
        &ws.knowledge().snapshot(),
        &fresh_ws.knowledge().snapshot(),
    )?;
    same_list("diagnostics", &got_diags, &want_diags)?;
    same_list("other diagnostics", &got_other, &want_other)?;
    let mut paths = Vec::new();
    let log = ws.take_parse_log().into_iter().filter(|r| r.uri == uri);
    for (i, record) in log.enumerate() {
        if let Some(diff) = record.difference {
            return Err(format!("analysis {i}, spliced: {diff}"));
        }
        paths.push(record.path);
    }
    Ok(Report { text, paths })
}

/// Open the document at `uri`: of `syntax`, or, for RDF/XML, of the syntax
/// its text gives (its name is an `.owl` file's), decided again at each
/// change, as a client's `.owl` is.
fn open(ws: &mut Workspace, uri: &str, text: String, syntax: Syntax) {
    if syntax == Syntax::RdfXml {
        ws.open_doc(uri.to_string(), text, 0);
    } else {
        ws.open_doc_as(uri.to_string(), text, 0, syntax);
    }
}

/// Apply `ch` to `text` the plain way, finding the lines again for each
/// change: the reference for the workspace's own edits, which move the line
/// starts along. A position that converts to no offset skips the change, as
/// the workspace does.
fn apply(text: &mut String, ch: &TextDocumentContentChangeEvent, enc: Encoding) {
    let Some(r) = ch.range else {
        *text = ch.text.clone();
        return;
    };
    let lines = LineIndex::new(text);
    let (Some(s), Some(e)) = (
        offset_in(&lines, text, enc, r.start),
        offset_in(&lines, text, enc, r.end),
    ) else {
        return;
    };
    text.replace_range(s.min(e) as usize..e.max(s) as usize, &ch.text);
}

/// The LSP change that replaces the bytes `range` of `text` with `new`. The
/// range must lie on character boundaries. An end between the `\r` and the
/// `\n` of a line break, where no LSP position points, takes the whole break
/// in, and `new` puts back its half of it.
pub fn change(
    text: &str,
    range: std::ops::Range<usize>,
    new: &str,
    enc: Encoding,
) -> TextDocumentContentChangeEvent {
    let b = text.as_bytes();
    let splits = |x: usize| x > 0 && b.get(x - 1) == Some(&b'\r') && b.get(x) == Some(&b'\n');
    let (mut s, mut e) = (range.start, range.end);
    let mut new = new.to_string();
    if splits(s) {
        s -= 1;
        new.insert(0, '\r');
    }
    if splits(e) {
        e += 1;
        new.push('\n');
    }
    let lines = LineIndex::new(text);
    let at = |x: usize| position_in(&lines, text, enc, x as u32);
    TextDocumentContentChangeEvent {
        range: Some(Range::new(at(s), at(e))),
        range_length: None,
        text: new,
    }
}

/// Every check on, the opt-in ones included, so each one is compared.
fn settings() -> Settings {
    Settings {
        lint_enable: crate::features::curation::OPT_IN
            .iter()
            .map(|c| c.to_string())
            .collect(),
        ..Settings::default()
    }
}

/// The document's diagnostics in full.
fn diagnostics(ws: &mut Workspace, uri: &str, enc: Encoding, s: &Settings) -> Vec<Diagnostic> {
    let Some(a) = ws.analysis(uri) else {
        return Vec::new();
    };
    diagnose::document(
        diagnose::Doc::Rdf(&a),
        ws.knowledge(),
        enc,
        &DiagnosticInputs::new(s),
    )
}

/// The open document's diagnostics as the server composes them, with what
/// it keeps between edits.
fn kept_diagnostics(ws: &mut Workspace, uri: &str, enc: Encoding, s: &Settings) -> Vec<Diagnostic> {
    let Some(a) = ws.analysis(uri) else {
        return Vec::new();
    };
    ws.diagnostics(uri, &a, enc, &DiagnosticInputs::new(s))
}

/// Whether `tag` reads as a Turtle language tag.
fn is_language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or_default();
    !first.is_empty()
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The document beside the edited one.
const OTHER: &str = "file:///oracle/other.ttl";

/// A Turtle document with a term for each of the first labels of `text`, a
/// `syntax` document, labelled the same: where the edited document keeps
/// one of those labels on a term, the two documents report each other's
/// term (`duplicate-label`). Typed as a class and a property, so its kind
/// collides with any.
fn companion(syntax: Syntax, text: &str) -> String {
    const LABELS: [&str; 2] = [
        "http://www.w3.org/2000/01/rdf-schema#label",
        "http://www.w3.org/2004/02/skos/core#prefLabel",
    ];
    let a = Analysis::with_syntax(
        "file:///oracle/doc".into(),
        text,
        Default::default(),
        false,
        syntax,
    );
    let mut out = String::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n",
    );
    let labels = a.parsed.triples.iter().filter_map(|t| match &t.o {
        Node::Literal { value, lang, .. } if LABELS.contains(&t.p.as_str()) => Some((value, *lang)),
        _ => None,
    });
    for (i, (value, lang)) in labels.take(64).enumerate() {
        let lang = match lang {
            Some(l) if is_language_tag(&l) => format!("@{}", &*l),
            Some(_) => continue,
            None => String::new(),
        };
        let mut quoted = String::new();
        for c in value.chars() {
            match c {
                '\\' => quoted.push_str("\\\\"),
                '"' => quoted.push_str("\\\""),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                c => quoted.push(c),
            }
        }
        out += &format!(
            "<http://oracle.example/other/{i}> a owl:Class , owl:ObjectProperty ; rdfs:label \"{quoted}\"{lang} .\n"
        );
    }
    out
}

fn extension(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Turtle => "ttl",
        Syntax::TriG => "trig",
        Syntax::NTriples => "nt",
        Syntax::NQuads => "nq",
        Syntax::JsonLd => "jsonld",
        Syntax::RdfXml => "owl",
        Syntax::OwlFunctional => "ofn",
        Syntax::OwlXml => "owx",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One batch of the given changes, analysed after.
    fn batch(changes: Vec<TextDocumentContentChangeEvent>) -> Batch {
        Batch {
            changes,
            analyse: true,
        }
    }

    fn full(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.into(),
        }
    }

    const TTL: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        ex:a a ex:C ; rdfs:label \"A\" .\n\
        ex:b rdfs:subClassOf ex:a ; rdfs:comment \"B\" .\n";

    /// Replace the first `old` in `text` with `new`, as a change.
    fn edit(text: &str, old: &str, new: &str, enc: Encoding) -> TextDocumentContentChangeEvent {
        let at = text.find(old).unwrap();
        change(text, at..at + old.len(), new, enc)
    }

    const XML: &str = "<?xml version=\"1.0\"?>\n\
        <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
        \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\">\n\
        \x20 <rdfs:Class rdf:about=\"http://example.org/A\">\n\
        \x20   <rdfs:label>A</rdfs:label>\n\
        \x20   <rdfs:subClassOf><rdf:Description/></rdfs:subClassOf>\n\
        \x20 </rdfs:Class>\n\
        </rdf:RDF>\n";

    #[test]
    fn an_rdf_xml_document_whose_first_bytes_change_syntax_matches_a_fresh_open() {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            // Into Turtle (a broken one), then back to RDF/XML; into JSON-LD.
            let prefix = "@prefix ex: <http://example.org/> .\n";
            let turtle = change(XML, 0..0, prefix, enc);
            let after = format!("{prefix}{XML}");
            let back = change(&after, 0..prefix.len(), "", enc);
            let r = check(
                Syntax::RdfXml,
                XML,
                vec![batch(vec![turtle.clone()]), batch(vec![back])],
                enc,
            )
            .unwrap();
            assert_eq!(r.text, XML);
            let r = check(Syntax::RdfXml, XML, vec![batch(vec![turtle])], enc).unwrap();
            assert_eq!(r.text, after);
            let json = check(
                Syntax::RdfXml,
                XML,
                vec![batch(vec![full("{\"@id\": \"x\"}")])],
                enc,
            );
            assert_eq!(json.unwrap().text, "{\"@id\": \"x\"}");
            // A cut in the middle of the XML, analysed between keystrokes.
            let cut = edit(XML, "</rdfs:label>", "</rdfs:lab", enc);
            check(Syntax::RdfXml, XML, vec![batch(vec![cut])], enc).unwrap();
        }
    }

    #[test]
    fn a_ranged_edit_matches_a_fresh_open() {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            let r = check(
                Syntax::Turtle,
                TTL,
                vec![batch(vec![edit(TTL, "ex:C", "ex:D", enc)])],
                enc,
            )
            .unwrap();
            assert_eq!(r.text, TTL.replace("ex:C", "ex:D"));
        }
    }

    #[test]
    fn a_two_change_batch_applies_each_in_the_text_the_first_left() {
        let enc = Encoding::Utf16;
        let first = edit(TTL, "\"A\"", "\"Apple\"", enc);
        let after = TTL.replace("\"A\"", "\"Apple\"");
        // On the same line as the first, so a position read in the old text
        // would land elsewhere.
        let second = edit(&after, " .\nex:b", " ; ex:p ex:q .\nex:b", enc);
        let r = check(
            Syntax::Turtle,
            TTL,
            vec![
                Batch {
                    changes: vec![first, second],
                    analyse: false,
                },
                batch(vec![]),
            ],
            enc,
        )
        .unwrap();
        assert_eq!(
            r.text,
            after.replace(" .\nex:b", " ; ex:p ex:q .\nex:b"),
            "{:?}",
            r.text
        );
    }

    #[test]
    fn a_full_text_change_replaces_the_text() {
        let to = "ex:x ex:y ex:z .\n";
        let r = check(
            Syntax::TriG,
            TTL,
            vec![batch(vec![full("{ }")]), batch(vec![full(to)])],
            Encoding::Utf16,
        )
        .unwrap();
        assert_eq!(r.text, to);
    }

    #[test]
    fn crlf_text_and_an_edit_inside_its_line_break() {
        let text = TTL.replace('\n', "\r\n");
        let enc = Encoding::Utf16;
        // Between `\r` and `\n`: the change takes the whole break.
        let at = text.find("\r\nex:b").unwrap() + 1;
        let c = change(&text, at..at, " # note", enc);
        let r = check(Syntax::Turtle, &text, vec![batch(vec![c])], enc).unwrap();
        let mut want = text.clone();
        want.insert_str(at, " # note");
        assert_eq!(r.text, want);
    }

    #[test]
    fn multi_byte_text_matches_in_both_encodings() {
        let text =
            "@prefix ex: <http://example.org/> .\nex:é ex:p \"日本😀\" .\nex:b ex:p ex:é .\n";
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            let c = edit(text, "\"日本😀\"", "\"😀😀\" , ex:ü", enc);
            let later = text.replace("\"日本😀\"", "\"😀😀\" , ex:ü");
            let c2 = edit(&later, "ex:b", "ex:ß", enc);
            let r = check(Syntax::Turtle, text, vec![batch(vec![c, c2])], enc).unwrap();
            assert_eq!(r.text, later.replace("ex:b", "ex:ß"));
        }
    }

    #[test]
    fn a_difference_is_named_by_field() {
        let enc = Encoding::Utf16;
        let differs = |other: &str| {
            check_against(Syntax::Turtle, TTL, vec![batch(vec![])], enc, Some(other)).unwrap_err()
        };
        // Same length, same tokens: the IRI of one ref differs.
        let err = differs(&TTL.replace("ex:C", "ex:D"));
        assert!(err.starts_with("parsed.refs[2]: edited Ref"), "{err}");
        // The label's literal: the triple that holds it.
        let err = differs(&TTL.replace("\"A\"", "\"Z\""));
        assert!(err.starts_with("parsed.triples[1]: edited Triple"), "{err}");
        // A comment's text is in no field but the text itself.
        let err = differs(&format!("{TTL}# y\n"));
        assert!(
            err.starts_with("tokens: edited has 22, fresh has 23"),
            "{err}"
        );
        let commented = format!("{TTL}# x\n");
        let err = check_against(
            Syntax::Turtle,
            &commented,
            vec![batch(vec![])],
            enc,
            Some(&format!("{TTL}# y\n")),
        )
        .unwrap_err();
        assert!(err.starts_with("text: edited"), "{err}");
    }
}
