//! Diagnostics and the quick fixes attached to them.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::{Form, Severity};
use crate::vocab::VOCABS;
use lsp_types::*;
use rustc_hash::FxHashMap;
use serde_json::json;

pub const SOURCE: &str = "turtle";

pub fn diagnostics(a: &Analysis, enc: Encoding) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mk = |span, severity, code: &str, message: String| Diagnostic {
        range: a.range(enc, span),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some(SOURCE.to_string()),
        message,
        ..Default::default()
    };

    for e in &a.parsed.errors {
        let sev = match e.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        };
        out.push(mk(e.span, sev, "syntax", e.message.clone()));
    }

    // Undeclared prefixes, with an "add @prefix" fix when the namespace is known.
    for u in a.parsed.prefix_uses.iter().filter(|u| !u.resolved) {
        let shown = if u.name.is_empty() {
            ":".to_string()
        } else {
            format!("{}:", u.name)
        };
        let mut d = mk(
            u.span,
            DiagnosticSeverity::ERROR,
            "undeclared-prefix",
            format!("Undeclared prefix '{shown}'"),
        );
        if let Some(v) = VOCABS.by_prefix(&u.name) {
            d.message
                .push_str(&format!(" (add @prefix {shown} <{}>)", v.namespace));
            d.data = Some(json!({ "prefix": u.name, "namespace": v.namespace }));
        } else {
            d.message
                .push_str(". Declare it, or add it to a prefixes.ttl in the project");
        }
        out.push(d);
    }

    // Duplicate and unused declarations.
    let mut seen: FxHashMap<&str, &str> = FxHashMap::default();
    for p in &a.parsed.prefixes {
        if let Some(prev) = seen.insert(&p.name, &p.iri) {
            if prev == p.iri {
                let mut d = mk(
                    p.span,
                    DiagnosticSeverity::HINT,
                    "duplicate-prefix",
                    format!("Prefix '{}:' is declared twice", p.name),
                );
                d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
                out.push(d);
            } else {
                out.push(mk(
                    p.name_span,
                    DiagnosticSeverity::WARNING,
                    "redefined-prefix",
                    format!("Prefix '{}:' is redefined (was <{prev}>)", p.name),
                ));
            }
        }
    }
    let is_prefix_file = a.uri.ends_with(crate::workspace::PREFIX_FILE);
    if !is_prefix_file {
        let used: rustc_hash::FxHashSet<&str> = a
            .parsed
            .prefix_uses
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        for p in a
            .parsed
            .prefixes
            .iter()
            .filter(|p| !used.contains(p.name.as_str()))
        {
            let mut d = mk(
                p.span,
                DiagnosticSeverity::HINT,
                "unused-prefix",
                format!("Prefix '{}:' is never used", p.name),
            );
            d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
            out.push(d);
        }
    }

    // Terms of well-known vocabularies (matched by namespace IRI, not prefix name).
    for r in &a.parsed.refs {
        if matches!(r.form, Form::A) {
            continue;
        }
        let Some((v, local)) = VOCABS.lookup(&r.iri) else {
            continue;
        };
        if local.is_empty() {
            continue;
        }
        match v.term(local) {
            Some(t) if t.deprecated => {
                let mut d = mk(
                    r.span,
                    DiagnosticSeverity::HINT,
                    "deprecated-term",
                    format!("'{}:{local}' is deprecated", v.prefix),
                );
                d.tags = Some(vec![DiagnosticTag::DEPRECATED]);
                out.push(d);
            }
            Some(_) => {}
            None => {
                let mut msg = format!(
                    "'{local}' is not defined in the {} vocabulary <{}>",
                    v.prefix, v.namespace
                );
                let mut d = mk(
                    r.span,
                    DiagnosticSeverity::WARNING,
                    "unknown-term",
                    String::new(),
                );
                if let Some(s) = v.suggest(local) {
                    msg.push_str(&format!(". Did you mean '{}'?", s.local));
                    let replacement = match r.form {
                        Form::PName { .. } => {
                            let text_prefix = a
                                .parsed
                                .prefix_uses
                                .iter()
                                .find(|u| u.span == r.span)
                                .map(|u| u.name.clone())
                                .unwrap_or_default();
                            format!("{text_prefix}:{}", s.local)
                        }
                        _ => format!("<{}{}>", v.namespace, s.local),
                    };
                    d.data = Some(json!({ "replace": replacement }));
                }
                d.message = msg;
                out.push(d);
            }
        }
    }
    out
}

/// Quick fixes for the diagnostics above, plus "add all missing prefixes".
pub fn code_actions(
    a: &Analysis,
    text: &str,
    enc: Encoding,
    params: &CodeActionParams,
) -> Vec<CodeActionOrCommand> {
    let uri = params.text_document.uri.clone();
    let mut actions = Vec::new();
    let edit = |edits: Vec<TextEdit>| {
        let mut changes = std::collections::HashMap::new();
        changes.insert(uri.clone(), edits);
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    };
    let mut missing: Vec<(String, String)> = Vec::new();
    for d in &params.context.diagnostics {
        if d.source.as_deref() != Some(SOURCE) {
            continue;
        }
        let code = match &d.code {
            Some(NumberOrString::String(c)) => c.as_str(),
            _ => continue,
        };
        let data = d.data.as_ref();
        match code {
            "undeclared-prefix" => {
                let (Some(p), Some(ns)) = (
                    data.and_then(|x| x["prefix"].as_str()),
                    data.and_then(|x| x["namespace"].as_str()),
                ) else {
                    continue;
                };
                if missing.iter().any(|(q, _)| q == p) {
                    continue;
                }
                missing.push((p.to_string(), ns.to_string()));
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Add @prefix {p}: <{ns}>"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: edit(vec![prefix_insertion(a, text, enc, p, ns)]),
                    is_preferred: Some(true),
                    ..Default::default()
                }));
            }
            "unused-prefix" | "duplicate-prefix" => {
                let start = a.offset(enc, d.range.start).unwrap_or(0) as usize;
                let end = a.offset(enc, d.range.end).unwrap_or(0) as usize;
                // Delete the whole line(s) when the directive is alone on them.
                let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
                let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
                let alone = text[line_start..start].trim().is_empty()
                    && text[end..line_end].trim().is_empty();
                let span = if alone {
                    (line_start, line_end)
                } else {
                    (start, end)
                };
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Remove unused prefix".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: edit(vec![TextEdit {
                        range: a.range(enc, crate::syntax::Span::new(span.0 as u32, span.1 as u32)),
                        new_text: String::new(),
                    }]),
                    is_preferred: Some(true),
                    ..Default::default()
                }));
            }
            "unknown-term" => {
                let Some(rep) = data.and_then(|x| x["replace"].as_str()) else {
                    continue;
                };
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Change to '{rep}'"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: edit(vec![TextEdit {
                        range: d.range,
                        new_text: rep.to_string(),
                    }]),
                    is_preferred: Some(true),
                    ..Default::default()
                }));
            }
            _ => {}
        }
    }
    // Offer "add all" whenever the file has more than one fixable undeclared prefix.
    let mut all: Vec<(String, String)> = Vec::new();
    for u in a.parsed.prefix_uses.iter().filter(|u| !u.resolved) {
        if let Some(v) = VOCABS.by_prefix(&u.name)
            && !all.iter().any(|(p, _)| *p == u.name)
        {
            all.push((u.name.clone(), v.namespace.to_string()));
        }
    }
    if all.len() > 1 {
        let edits = vec![TextEdit {
            range: prefix_insertion(a, text, enc, "", "").range,
            new_text: all.iter().map(|(p, ns)| prefix_line(a, p, ns)).collect(),
        }];
        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Add all missing prefixes ({})", all.len()),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: edit(edits),
            ..Default::default()
        }));
    }
    actions
}

fn prefix_line(a: &Analysis, prefix: &str, ns: &str) -> String {
    let sparql = a.parsed.prefixes.last().is_some_and(|p| {
        a.parsed
            .tokens
            .iter()
            .any(|t| t.start == p.span.start && t.kind == crate::syntax::lexer::Kind::SparqlPrefix)
    });
    if sparql {
        format!("PREFIX {prefix}: <{ns}>\n")
    } else {
        format!("@prefix {prefix}: <{ns}> .\n")
    }
}

/// Insert a prefix declaration after the last one (or at the top of the file).
pub fn prefix_insertion(
    a: &Analysis,
    text: &str,
    enc: Encoding,
    prefix: &str,
    ns: &str,
) -> TextEdit {
    let at = match a.parsed.prefixes.last() {
        Some(p) => text[p.span.end as usize..]
            .find('\n')
            .map_or(text.len(), |i| p.span.end as usize + i + 1),
        None => {
            // After a leading comment block, if any.
            let mut off = 0;
            for line in text.split_inclusive('\n') {
                if line.trim_start().starts_with('#') {
                    off += line.len();
                } else {
                    break;
                }
            }
            off
        }
    };
    let mut new_text = prefix_line(a, prefix, ns);
    if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
        new_text.insert(0, '\n');
    }
    let pos = a.position(enc, at as u32);
    TextEdit {
        range: Range::new(pos, pos),
        new_text,
    }
}
