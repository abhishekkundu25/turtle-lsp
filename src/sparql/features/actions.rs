//! Code actions in SPARQL documents, with the Turtle titles and kinds: the
//! quick fixes for the diagnostics (add a `PREFIX`, add all missing ones,
//! remove an unused one, change a misspelled term) and the refactorings
//! (organise prefixes, shorten and expand IRIs). Nothing here writes
//! `@prefix` or touches `owl:imports`.

use super::edits::add_prefix;
use super::navigation::{prefix_of, scope_at};
use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::diagnostics::SOURCE;
use crate::features::quickfix::{FixData, candidate_title, prefix_candidates, removal};
use crate::features::refactor::{
    Directive, expand_iri, line_content_end, organise_block, wanted, workspace_edit,
};
use crate::knowledge::Knowledge;
use crate::sparql::lexer;
use crate::sparql::{Form, Level, PrefixSource, TermRef, TokenKind};
use crate::syntax::Span;
use crate::syntax::names::{newline, shorten, token_edits_after};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::json;

/// `data.turtle` of a "Shorten all IRIs in file" offered without its edit.
const SHORTEN_ALL: &str = "shorten-all";

pub(crate) fn code_actions(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    params: &CodeActionParams,
    lazy: bool,
) -> Vec<CodeActionOrCommand> {
    let uri = &params.text_document.uri;
    let only = params.context.only.as_deref();
    let mut out: Vec<CodeAction> = Vec::new();
    let edit = |edits: Vec<TextEdit>| Some(workspace_edit(uri, edits));
    let quick =
        |title: String, d: Option<&Diagnostic>, edits, preferred: Option<bool>| CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: d.map(|d| vec![d.clone()]),
            edit: edit(edits),
            is_preferred: preferred,
            ..Default::default()
        };

    // ---- quick fixes ----
    let mut missing: Vec<String> = Vec::new();
    for d in &params.context.diagnostics {
        if d.source.as_deref() != Some(SOURCE) {
            continue;
        }
        let Some(NumberOrString::String(code)) = &d.code else {
            continue;
        };
        match code.as_str() {
            "undeclared-prefix" => {
                let Some(FixData::UndeclaredPrefix {
                    prefix: p,
                    candidates,
                    ..
                }) = FixData::of(d)
                else {
                    continue;
                };
                if missing.contains(&p) {
                    continue;
                }
                for (i, c) in candidates.iter().enumerate() {
                    let title = candidate_title(&p, c, "PREFIX");
                    let Some(fix) = add_prefix(q, enc, &p, &c.namespace) else {
                        continue;
                    };
                    let fix = in_document_style(q, fix);
                    out.push(quick(title, Some(d), vec![fix], Some(i == 0)));
                }
                missing.push(p);
            }
            "unused-prefix" | "duplicate-prefix" => {
                let (Some(start), Some(end)) =
                    (q.offset(enc, d.range.start), q.offset(enc, d.range.end))
                else {
                    continue;
                };
                let span = removal(q.text(), Span::new(start, end));
                let Some(fix) = text_edits(q, enc, vec![(span, String::new())]) else {
                    continue;
                };
                let title = if code == "unused-prefix" {
                    "Remove unused prefix"
                } else {
                    "Remove duplicate prefix"
                };
                out.push(quick(title.into(), Some(d), fix, Some(true)));
            }
            "unknown-term" | crate::features::lint::CODE => {
                let Some(FixData::Replace { replace: rep }) = FixData::of(d) else {
                    continue;
                };
                let (Some(start), Some(end)) =
                    (q.offset(enc, d.range.start), q.offset(enc, d.range.end))
                else {
                    continue;
                };
                let Some(fix) = text_edits(q, enc, vec![(Span::new(start, end), rep.clone())])
                else {
                    continue;
                };
                out.push(quick(
                    format!("Change to '{rep}'"),
                    Some(d),
                    fix,
                    Some(true),
                ));
            }
            _ => {}
        }
    }
    if let Some(all) = add_all_missing(q, k, enc) {
        out.push(all);
    }
    out.retain(|a| a.kind.as_ref().is_none_or(|kind| wanted(only, kind)));

    // ---- refactorings ----
    if wanted(only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
        && let Some(fix) = organise(q, enc)
    {
        out.push(CodeAction {
            title: "Organise prefixes".into(),
            kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            edit: edit(vec![fix]),
            ..Default::default()
        });
    }
    if wanted(only, &CodeActionKind::REFACTOR_REWRITE) {
        let rewrite = |title: String, edits: Option<Vec<TextEdit>>, data| CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            edit: edits.map(|e| workspace_edit(uri, e)),
            data,
            ..Default::default()
        };
        let start = q.offset(enc, params.range.start).unwrap_or(0);
        let end = q.offset(enc, params.range.end).unwrap_or(start).max(start);
        if let Some(r) = ref_in(q, start, end)
            && let Some(iri) = r.iri.as_deref()
        {
            let at = |new_text: String| text_edits(q, enc, vec![(r.span, new_text)]);
            match r.form {
                Form::Iri => {
                    if let Some(pname) = shorten(iri, &scope_at(q, r.span.start))
                        && let Some(edits) = at(pname.clone())
                    {
                        let title = format!("Shorten IRI to {pname}");
                        out.push(rewrite(title, Some(edits), None));
                    }
                }
                Form::PName { .. } => {
                    if let Some(edits) = expand_iri(iri, true).and_then(at) {
                        out.push(rewrite("Expand to full IRI".into(), Some(edits), None));
                    }
                }
                Form::A => {}
            }
        }
        let data = json!({ "uri": uri, "turtle": SHORTEN_ALL });
        let edits = if lazy {
            (!shortenable(q).is_empty()).then_some(None)
        } else {
            shorten_all_edits(q, enc)
                .filter(|e| !e.is_empty())
                .map(Some)
        };
        if let Some(edits) = edits {
            out.push(rewrite(
                "Shorten all IRIs in file".into(),
                edits,
                Some(data),
            ));
        }
    }
    out.into_iter()
        .map(CodeActionOrCommand::CodeAction)
        .collect()
}

pub(crate) fn resolve_code_action(
    q: &SparqlAnalysis,
    enc: Encoding,
    mut action: CodeAction,
) -> CodeAction {
    let Some(data) = action.data.as_ref() else {
        return action;
    };
    if data["turtle"].as_str() != Some(SHORTEN_ALL) {
        return action;
    }
    let Ok(uri) = serde_json::from_value::<Uri>(data["uri"].clone()) else {
        return action;
    };
    action.edit = shorten_all_edits(q, enc).map(|e| workspace_edit(&uri, e));
    action
}

/// "Add all missing prefixes (n)" when more than one undeclared prefix has a
/// namespace to add: one edit declaring each with its best candidate.
fn add_all_missing(q: &SparqlAnalysis, k: &Knowledge, enc: Encoding) -> Option<CodeAction> {
    let mut all: Vec<(&str, String)> = Vec::new();
    for r in &q.doc.structure().refs {
        let Some(p) = prefix_of(q, r).filter(|_| r.prefix == Some(PrefixSource::Undeclared)) else {
            continue;
        };
        if !all.iter().any(|(n, _)| *n == p)
            && let Some(c) = prefix_candidates(p, k).into_iter().next()
        {
            all.push((p, c.namespace));
        }
    }
    if all.len() < 2 {
        return None;
    }
    let line = |(p, ns): &(&str, String)| format!("PREFIX {p}: <{ns}>");
    let mut fix = add_prefix(q, enc, all[0].0, &all[0].1)?;
    let lines: Vec<String> = all.iter().map(line).collect();
    fix.new_text = fix.new_text.replacen(&line(&all[0]), &lines.join("\n"), 1);
    let fix = in_document_style(q, fix);
    Some(CodeAction {
        title: format!("Add all missing prefixes ({})", all.len()),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(workspace_edit(
            &crate::features::navigation::uri(&q.uri),
            vec![fix],
        )),
        ..Default::default()
    })
}

/// `edit` with the document's line ending in its text.
fn in_document_style(q: &SparqlAnalysis, mut edit: TextEdit) -> TextEdit {
    let nl = newline(q.text());
    if nl != "\n" {
        edit.new_text = edit.new_text.replace('\n', nl);
    }
    edit
}

/// The ref under the cursor, or the first one inside a selection.
fn ref_in(q: &SparqlAnalysis, start: u32, end: u32) -> Option<&TermRef> {
    let refs = &q.doc.structure().refs;
    refs.iter().find(|r| r.span.contains(start)).or_else(|| {
        let i = refs.partition_point(|r| r.span.start < start);
        refs.get(i).filter(|r| r.span.start < end)
    })
}

/// Every `<...>` a prefix in effect there shortens, with its prefixed name.
fn shortenable(q: &SparqlAnalysis) -> Vec<(Span, String)> {
    q.doc
        .structure()
        .refs
        .iter()
        .filter(|r| r.form == Form::Iri)
        .filter_map(|r| {
            Some((
                r.span,
                shorten(r.iri.as_deref()?, &scope_at(q, r.span.start))?,
            ))
        })
        .collect()
}

fn shorten_all_edits(q: &SparqlAnalysis, enc: Encoding) -> Option<Vec<TextEdit>> {
    text_edits(q, enc, shortenable(q))
}

/// Edits replacing whole tokens, through [`token_edits_after`] on a window
/// around them: how `<` lexes depends on the brackets open before it, so the
/// window starts where the document kept the lexer's state, and it ends two
/// tokens past the last edit, where the edited text leaves the lexer as the
/// old one did (else at the end of the text), so what follows lexes as before.
/// `None` when the edits cannot keep the tokens (see [`token_edits_after`]).
pub(super) fn text_edits(
    q: &SparqlAnalysis,
    enc: Encoding,
    reps: Vec<(Span, String)>,
) -> Option<Vec<TextEdit>> {
    let (text, tokens) = (q.text(), q.doc.tokens());
    let lo = reps.iter().map(|r| r.0.start).min().unwrap_or(0);
    let hi = reps.iter().map(|r| r.0.end).max().unwrap_or(0);
    let (from, state) = q.doc.lexer_state_at(lo);
    let lex_text = |s: &str| lexer::resume(s, state.clone());
    let reps: Vec<_> = reps
        .into_iter()
        .map(|(s, r)| (Span::new(s.start - from, s.end - from), r))
        .collect();
    // The lexer remembers the last two tokens: past them, at a gap, the states can meet again.
    let (mut to, mut more) = (hi as usize, 2);
    for t in &tokens[tokens.partition_point(|t| t.span.end <= hi)..] {
        if more == 0 && t.span.start as usize > to {
            break;
        }
        more -= usize::from(more > 0 && t.kind != TokenKind::Comment);
        to = t.span.end as usize;
    }
    let edits_to = |to: usize| {
        let window = &text[from as usize..to];
        let edits = token_edits_after(window, reps.clone(), &lexer::kinds, &|s| lex_text(s).0)?;
        let mut new = window.to_owned();
        for (s, t) in edits.iter().rev() {
            new.replace_range(s.start as usize..s.end as usize, t);
        }
        (to == text.len() || lex_text(&new).1 == lex_text(window).1).then_some(edits)
    };
    let edits = edits_to(to).or_else(|| edits_to(text.len()))?;
    let edits = edits
        .into_iter()
        .map(|(span, new_text)| TextEdit {
            range: q.range(enc, Span::new(span.start + from, span.end + from)),
            new_text,
        })
        .collect();
    Some(edits)
}

/// Sorts the prologue's run of `PREFIX` lines, drops duplicates and unused
/// ones; `None` when a prefix is redefined, a `BASE` or `VERSION` sits in
/// the run, it has a syntax error, or nothing would change. The comment
/// lines above the first `PREFIX` stay where they are.
fn organise(q: &SparqlAnalysis, enc: Encoding) -> Option<TextEdit> {
    let st = q.doc.structure();
    let text = q.text();
    let first_op = st.operations.first().map_or(u32::MAX, |o| o.keyword.start);
    let block: Vec<_> = st
        .prefixes
        .iter()
        .filter(|p| p.span.end <= first_op)
        .collect();
    let (first, last) = (block.first()?, block.last()?);
    let iri = |p: &crate::sparql::PrefixDecl| p.resolved.clone().unwrap_or_else(|| p.iri.clone());
    let mut iris: FxHashMap<&str, String> = FxHashMap::default();
    for p in &st.prefixes {
        if iris
            .insert(&p.name, iri(p))
            .is_some_and(|prev| prev != iri(p))
        {
            return None;
        }
    }
    let (start, end) = (first.span.start, last.span.end);
    let region_end = line_content_end(text, end as usize) as u32;
    let barrier = st
        .bases
        .iter()
        .map(|b| b.span)
        .chain(st.versions.iter().map(|v| v.span))
        .any(|s| s.start > start && s.start < end);
    let broken = q
        .doc
        .diagnostics()
        .iter()
        .any(|d| d.level == Level::Error && d.span.start < region_end && d.span.end >= start);
    if barrier || broken {
        return None;
    }
    let used: FxHashSet<&str> = st.refs.iter().filter_map(|r| prefix_of(q, r)).collect();
    let directives: Vec<Directive> = block
        .iter()
        .map(|p| Directive {
            name: &p.name,
            span: p.span,
        })
        .collect();
    let (span, new_text) = organise_block(text, &directives, |n| used.contains(n), true)?;
    Some(TextEdit {
        range: q.range(enc, span),
        new_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    /// "Change to" in a group of `n` triples lexes the text around the term,
    /// not the operation: the bytes lexed do not grow with `n`.
    #[test]
    fn a_token_edit_lexes_its_neighbourhood_not_the_operation() {
        let lexed = |n: usize, new: &str| {
            let text = format!(
                "SELECT * WHERE {{ {}FILTER(?s < ex:a) }}",
                "?s ?p ?o . ".repeat(n)
            );
            let q =
                SparqlAnalysis::new("file:///q.rq".into(), &text, Arc::new(PrefixMap::default()));
            let at = text.find("ex:a").unwrap() as u32;
            lexer::LEXED.with(|c| c.set(0));
            let edits = text_edits(
                &q,
                Encoding::Utf8,
                vec![(Span::new(at, at + 4), new.into())],
            );
            let edits: Vec<_> = edits.into_iter().flatten().map(|e| e.new_text).collect();
            (edits, lexer::LEXED.with(|c| c.get()))
        };
        // In context the `<` is the operator, so the IRI needs no space.
        let (one, two) = (lexed(10_000, "<http://e/b>"), lexed(20_000, "<http://e/b>"));
        assert_eq!(one.0, ["<http://e/b>"]);
        assert!(
            one.1 < 4096 && two.1 < 4096,
            "{} then {} bytes",
            one.1,
            two.1
        );
        // An edit that opens a group changes how the rest lexes: the window grows to the end.
        assert_eq!(lexed(10, "{").0, ["{"]);
    }

    /// The new text of each edit `text_edits` makes for `(token, new)` in `text`.
    fn edited(text: &str, token: &str, new: &str) -> Option<Vec<String>> {
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let at = text.rfind(token).unwrap() as u32;
        let reps = vec![(Span::new(at, at + token.len() as u32), new.into())];
        let edits = text_edits(&q, Encoding::Utf8, reps)?;
        Some(edits.into_iter().map(|e| e.new_text).collect())
    }

    /// Each action offered with the cursor on the last `token` of `text`,
    /// with the text its edit leaves.
    fn offered(text: &str, token: &str) -> Vec<(String, String)> {
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let (k, enc) = (Knowledge::default(), Encoding::Utf8);
        let at = text.rfind(token).unwrap() as u32;
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///q.rq".parse().unwrap(),
            },
            range: q.range(enc, Span::new(at, at)),
            context: CodeActionContext {
                diagnostics: super::super::diagnostics::diagnostics(
                    &q,
                    &k,
                    enc,
                    &Default::default(),
                ),
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let mut out = Vec::new();
        for a in code_actions(&q, &k, enc, &params, false) {
            let CodeActionOrCommand::CodeAction(a) = a else {
                continue;
            };
            let changes = a.edit.and_then(|e| e.changes).unwrap_or_default();
            let mut edits: Vec<_> = changes.into_values().flatten().collect();
            edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
            let mut after = text.to_owned();
            for e in edits {
                let (s, t) = (q.offset(enc, e.range.start), q.offset(enc, e.range.end));
                after.replace_range(s.unwrap() as usize..t.unwrap() as usize, &e.new_text);
            }
            out.push((a.title, after));
        }
        out
    }

    #[test]
    fn an_iri_that_would_read_as_less_than_is_not_offered() {
        let text = "PREFIX ex: <http://e/>\nSELECT * { ?a ex:p ?o FILTER(?a ex:b) }\n";
        let expand = |token: &str| {
            offered(text, token)
                .into_iter()
                .any(|(title, _)| title == "Expand to full IRI")
        };
        assert!(expand("ex:p"));
        assert!(!expand("ex:b"));
    }

    #[test]
    fn a_prefix_named_prefix_is_removed_with_its_keyword() {
        let text = "PREFIX PREFIX:<http://example/># c";
        let removed = offered(text, "PREFIX:")
            .into_iter()
            .find(|(title, _)| title == "Remove unused prefix");
        assert_eq!(removed.map(|r| r.1).as_deref(), Some("# c"));
    }

    #[test]
    fn the_empty_local_name_stays_apart_from_a_keyword() {
        let iri = "<http://e/>";
        let text = format!("PREFIX : {iri}\nSELECT * {{ BIND((1+{iri}SELECT *?o) AS ?x) }}");
        assert_eq!(edited(&text, iri, ":"), Some(vec![": ".to_owned()]));
    }
}
