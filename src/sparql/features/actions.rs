//! Code actions in SPARQL documents, with the Turtle titles and kinds: the
//! quick fixes for the diagnostics (add a `PREFIX`, add all missing ones,
//! remove an unused one, change a misspelled term) and the refactorings
//! (organise prefixes, shorten and expand IRIs). Nothing here writes
//! `@prefix` or touches `owl:imports`.

use super::edits::add_prefix;
use super::navigation::{prefix_of, scope_at};
use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::diagnostics::{SOURCE, candidate_title, prefix_candidates, removal};
use crate::features::refactor::{
    Directive, expand_iri, line_content_end, organise_block, wanted, workspace_edit,
};
use crate::knowledge::Knowledge;
use crate::sparql::{Form, Level, PrefixSource, TermRef};
use crate::syntax::Span;
use crate::syntax::names::{newline, shorten, spaced};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};

/// `data.turtle` of a "Shorten all IRIs in file" offered without its edit.
const SHORTEN_ALL: &str = "shorten-all";

pub fn code_actions(
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
    let mut missing: Vec<&str> = Vec::new();
    for d in &params.context.diagnostics {
        if d.source.as_deref() != Some(SOURCE) {
            continue;
        }
        let Some(NumberOrString::String(code)) = &d.code else {
            continue;
        };
        let data = d.data.as_ref();
        match code.as_str() {
            "undeclared-prefix" => {
                let Some(p) = data.and_then(|x| x["prefix"].as_str()) else {
                    continue;
                };
                if missing.contains(&p) {
                    continue;
                }
                missing.push(p);
                let Some(Value::Array(cands)) = data.map(|x| &x["candidates"]) else {
                    continue;
                };
                for (i, c) in cands.iter().enumerate() {
                    if let Some((title, ns)) = candidate_title(p, c, "PREFIX") {
                        let fix = in_document_style(q, add_prefix(q, enc, p, &ns));
                        out.push(quick(title, Some(d), vec![fix], Some(i == 0)));
                    }
                }
            }
            "unused-prefix" | "duplicate-prefix" => {
                let (Some(start), Some(end)) =
                    (q.offset(enc, d.range.start), q.offset(enc, d.range.end))
                else {
                    continue;
                };
                let span = removal(q.text(), Span::new(start, end));
                let fix = TextEdit {
                    range: q.range(enc, span),
                    new_text: String::new(),
                };
                let title = if code == "unused-prefix" {
                    "Remove unused prefix"
                } else {
                    "Remove duplicate prefix"
                };
                out.push(quick(title.into(), Some(d), vec![fix], Some(true)));
            }
            "unknown-term" | crate::features::lint::CODE => {
                let Some(rep) = data.and_then(|x| x["replace"].as_str()) else {
                    continue;
                };
                let fix = TextEdit {
                    range: d.range,
                    new_text: rep.to_string(),
                };
                out.push(quick(
                    format!("Change to '{rep}'"),
                    Some(d),
                    vec![fix],
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
            let at = |new_text: String| TextEdit {
                range: q.range(enc, r.span),
                new_text,
            };
            match r.form {
                Form::Iri => {
                    if let Some(pname) = shorten(iri, &scope_at(q, r.span.start)) {
                        let new_text = spaced(q.text(), r.span, pname);
                        let title = format!("Shorten IRI to {}", new_text.trim());
                        out.push(rewrite(title, Some(vec![at(new_text)]), None));
                    }
                }
                Form::PName { .. } => {
                    if let Some(full) = expand_iri(iri, true) {
                        out.push(rewrite(
                            "Expand to full IRI".into(),
                            Some(vec![at(full)]),
                            None,
                        ));
                    }
                }
                Form::A => {}
            }
        }
        let data = json!({ "uri": uri, "turtle": SHORTEN_ALL });
        let edits = if lazy {
            (!shortenable(q).is_empty()).then_some(None)
        } else {
            let edits = shorten_all_edits(q, enc);
            (!edits.is_empty()).then_some(Some(edits))
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

pub fn resolve_code_action(
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
    action.edit = Some(workspace_edit(&uri, shorten_all_edits(q, enc)));
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
    let mut fix = add_prefix(q, enc, all[0].0, &all[0].1);
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

fn shorten_all_edits(q: &SparqlAnalysis, enc: Encoding) -> Vec<TextEdit> {
    shortenable(q)
        .into_iter()
        .map(|(span, pname)| TextEdit {
            range: q.range(enc, span),
            new_text: spaced(q.text(), span, pname),
        })
        .collect()
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
