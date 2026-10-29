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
//! ([`diagnose::document`]). The first difference is the error, named by
//! field. The chaos tests run it on every iteration of their edit
//! mode.

use crate::analysis::{Analysis, Encoding, LineIndex, offset_in, position_in};
use crate::diagnose::{self, DiagnosticInputs};
use crate::settings::Settings;
use crate::syntax::{Iri, IriMap, Node, Parsed, PrefixMap, Syntax};
use crate::workspace::Workspace;
use lsp_types::{Diagnostic, Position, Range, TextDocumentContentChangeEvent};
use std::fmt::Debug;

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
}

/// Apply `batches` to a `syntax` document opened on `start`, positions in
/// `enc`, and compare the result with a fresh workspace opened on the text
/// they leave. `Err` names the first field that differs.
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
    let settings = settings();
    let mut ws = Workspace::new(Vec::new(), PrefixMap::default());
    ws.open_doc_as(uri.clone(), start.to_string(), 0, syntax);
    // As a client asks on open: the edits then have an analysis to go stale.
    diagnostics(&mut ws, &uri, enc, &settings);
    let mut text = start.to_string();
    for (i, batch) in batches.into_iter().enumerate() {
        for ch in &batch.changes {
            apply(&mut text, ch, enc);
        }
        ws.change_doc(&uri, i as i32 + 1, batch.changes, enc);
        if batch.analyse {
            diagnostics(&mut ws, &uri, enc, &settings);
        }
    }
    let got_diags = diagnostics(&mut ws, &uri, enc, &settings);
    let mut fresh_ws = Workspace::new(Vec::new(), PrefixMap::default());
    let want_text = fresh.unwrap_or(&text).to_string();
    fresh_ws.open_doc_as(uri.clone(), want_text, 0, syntax);
    let want_diags = diagnostics(&mut fresh_ws, &uri, enc, &settings);

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
    Ok(Report { text })
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

/// The document's diagnostics as the server composes them.
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

fn extension(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Turtle => "ttl",
        Syntax::TriG => "trig",
        Syntax::NTriples => "nt",
        Syntax::NQuads => "nq",
        Syntax::JsonLd => "jsonld",
    }
}

fn same<T: PartialEq + Debug>(field: &str, got: &T, want: &T) -> Result<(), String> {
    if got == want {
        return Ok(());
    }
    Err(format!("{field}: edited {got:?}, fresh {want:?}"))
}

fn same_list<T: PartialEq + Debug>(field: &str, got: &[T], want: &[T]) -> Result<(), String> {
    if let Some(i) = (0..got.len().min(want.len())).find(|&i| got[i] != want[i]) {
        return Err(format!(
            "{field}[{i}]: edited {:?}, fresh {:?}",
            got[i], want[i]
        ));
    }
    if got.len() != want.len() {
        let n = got.len().min(want.len());
        return Err(format!(
            "{field}: edited has {}, fresh has {}; first extra {:?}",
            got.len(),
            want.len(),
            got.get(n)
                .map_or_else(|| format!("{:?}", want[n]), |g| format!("{g:?}"))
        ));
    }
    Ok(())
}

/// The first key, in IRI order, whose values differ.
fn same_map<V: PartialEq + Debug>(
    field: &str,
    got: &IriMap<V>,
    want: &IriMap<V>,
) -> Result<(), String> {
    if **got == **want {
        return Ok(());
    }
    let mut keys: Vec<Iri> = got.keys().chain(want.keys()).copied().collect();
    keys.sort();
    let k = keys
        .into_iter()
        .find(|k| got.get(k) != want.get(k))
        .expect("unequal maps differ at a key");
    Err(format!(
        "{field}[{k}]: edited {:?}, fresh {:?}",
        got.get(&k),
        want.get(&k)
    ))
}

fn same_parse(got: &Parsed, want: &Parsed) -> Result<(), String> {
    // Every field, named: a field added to `Parsed` fails to compile here.
    let Parsed {
        syntax,
        prefixes,
        base,
        document_base,
        statements,
        graphs,
        refs,
        prefix_uses,
        triples,
        errors,
        comments,
        tokens,
    } = got;
    same("parsed.syntax", syntax, &want.syntax)?;
    same_list("parsed.prefixes", prefixes, &want.prefixes)?;
    same("parsed.base", base, &want.base)?;
    same("parsed.document_base", document_base, &want.document_base)?;
    same_list("parsed.statements", statements, &want.statements)?;
    same_list("parsed.graphs", graphs, &want.graphs)?;
    same_list("parsed.refs", refs, &want.refs)?;
    same_list("parsed.prefix_uses", prefix_uses, &want.prefix_uses)?;
    same_list("parsed.triples", triples, &want.triples)?;
    same_list("parsed.errors", errors, &want.errors)?;
    same_list("parsed.comments", comments, &want.comments)?;
    same_list("parsed.tokens", tokens, &want.tokens)
}

fn same_analysis(got: &Analysis, want: &Analysis) -> Result<(), String> {
    same_parse(&got.parsed, &want.parsed)?;
    same_list("tokens", got.tokens(), want.tokens())?;
    same_list("dropped", &got.dropped, &want.dropped)?;
    same_list("used_terms", &got.used_terms, &want.used_terms)?;
    same("external", &*got.external, &*want.external)?;
    same_map("defs", &got.defs, &want.defs)?;
    same_map("refs_by_iri", &got.refs_by_iri, &want.refs_by_iri)?;
    same_map("types", &got.types, &want.types)?;
    same_map("supers", &got.supers, &want.supers)?;
    // The label and comment indexes are private to the analysis: read them
    // for every IRI that can be a key (a subject is a ref, but read the
    // triples too).
    let mut keys: Vec<Iri> = got.refs_by_iri.keys().copied().collect();
    keys.extend(got.parsed.triples.iter().filter_map(|t| match t.s {
        Node::Iri(s) => Some(s),
        _ => None,
    }));
    keys.extend(want.refs_by_iri.keys());
    keys.sort();
    keys.dedup();
    for k in keys {
        same(
            &format!("labels[{k}]"),
            &got.label_en(&k),
            &want.label_en(&k),
        )?;
        same(
            &format!("descriptions[{k}]"),
            &got.comment_en(&k),
            &want.comment_en(&k),
        )?;
    }
    same_lines(got, want)?;
    // Last: a stale analysis is better named by the first field it gets wrong.
    same("text", &got.text, &want.text)
}

/// Offset to position and back, in both encodings, at about 256 offsets
/// spread over the text and at its end; each position also one column past
/// itself, which clamps at a line's end.
fn same_lines(got: &Analysis, want: &Analysis) -> Result<(), String> {
    let len = want.text.len();
    let step = (len / 256).max(1);
    for x in (0..len).step_by(step).chain([len]) {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            let x = x as u32;
            let pos = want.position(enc, x);
            same(
                &format!("position({enc:?}, {x})"),
                &got.position(enc, x),
                &pos,
            )?;
            for p in [pos, Position::new(pos.line, pos.character + 1)] {
                same(
                    &format!("offset({enc:?}, {}:{})", p.line, p.character),
                    &got.offset(enc, p),
                    &want.offset(enc, p),
                )?;
            }
        }
    }
    Ok(())
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
