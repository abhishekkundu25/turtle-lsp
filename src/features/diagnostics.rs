//! Diagnostics and the quick fixes attached to them.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::syntax::{Form, Severity};
use crate::vocab::VOCABS;
use lsp_types::*;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};

pub const SOURCE: &str = "turtle";

/// Codes of diagnostics that claim something is known nowhere in the
/// workspace, so are only reported once the initial index is in.
pub const NEEDS_INDEX: &[&str] = crate::features::shacl_lint::NEEDS_INDEX;

pub fn needs_index(d: &Diagnostic) -> bool {
    matches!(&d.code, Some(NumberOrString::String(c))
        if NEEDS_INDEX.contains(&c.as_str()) || c == crate::features::lint::CODE)
}

/// A namespace to bind an undeclared prefix name to.
pub struct PrefixCandidate {
    pub namespace: String,
    /// Workspace files that bind the name to it (0 for a well-known one the
    /// project does not use).
    pub files: u32,
}

/// Namespaces for prefix `name`, best first: the project's most common
/// binding, then the well-known vocabulary's if different.
pub fn prefix_candidates(name: &str, k: &Knowledge) -> Vec<PrefixCandidate> {
    // The empty prefix is file-local by convention: no project binding for it.
    let project = if name.is_empty() {
        vec![]
    } else {
        k.bindings(name)
    };
    let mut out: Vec<PrefixCandidate> = project
        .into_iter()
        .take(1)
        .map(|(ns, files)| PrefixCandidate {
            namespace: ns.to_string(),
            files,
        })
        .collect();
    if let Some(v) = VOCABS.by_prefix(name)
        && !out.iter().any(|c| c.namespace == v.namespace)
    {
        out.push(PrefixCandidate {
            namespace: v.namespace.to_string(),
            files: 0,
        });
    }
    out
}

fn candidate_title(prefix: &str, c: &Value) -> Option<(String, String)> {
    let ns = c["namespace"].as_str()?;
    let files = c["files"].as_u64().unwrap_or(0);
    let mut title = format!("Add @prefix {prefix}: <{ns}>");
    match files {
        0 => {}
        1 => title.push_str(" (used in 1 file)"),
        n => title.push_str(&format!(" (used in {n} files)")),
    }
    Some((title, ns.to_string()))
}

/// Which optional diagnostics to compute (from the client's settings).
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub lint_undescribed_terms: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            lint_undescribed_terms: true,
        }
    }
}

impl Options {
    pub fn from_settings(s: &crate::settings::Settings) -> Options {
        Options {
            lint_undescribed_terms: s.lint_undescribed_terms,
        }
    }
}

/// [`diagnostics_with`] with default options.
pub fn diagnostics(a: &Analysis, k: &Knowledge, enc: Encoding) -> Vec<Diagnostic> {
    diagnostics_with(a, k, enc, &Options::default())
}

pub fn diagnostics_with(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    opts: &Options,
) -> Vec<Diagnostic> {
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

    // Undeclared prefixes, with "add @prefix" fixes for every known namespace.
    let mut candidates: FxHashMap<&str, Vec<PrefixCandidate>> = FxHashMap::default();
    // N-Triples has no prefixes at all; the parser already reports the pname.
    let ntriples = a.parsed.syntax == crate::syntax::Syntax::NTriples;
    for u in a
        .parsed
        .prefix_uses
        .iter()
        .filter(|u| !u.resolved && !ntriples)
    {
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
        let cands = candidates
            .entry(u.name.as_str())
            .or_insert_with(|| prefix_candidates(&u.name, k));
        if let Some(first) = cands.first() {
            d.message
                .push_str(&format!(" (add @prefix {shown} <{}>)", first.namespace));
            d.data = Some(json!({
                "prefix": u.name,
                "namespace": first.namespace,
                "candidates": cands
                    .iter()
                    .map(|c| json!({ "namespace": c.namespace, "files": c.files }))
                    .collect::<Vec<_>>(),
            }));
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
    // Suggestions are memoised: a file can use one unknown term thousands of times.
    let mut suggestions: FxHashMap<(&str, &str), Option<&str>> = FxHashMap::default();
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
            // Described by the workspace or a resolved import: newer than the snapshot.
            None if k.is_described(&r.iri) => {}
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
                let suggestion = *suggestions
                    .entry((v.namespace, local))
                    .or_insert_with(|| v.suggest(local).map(|t| t.local.as_str()));
                if let Some(s) = suggestion {
                    msg.push_str(&format!(". Did you mean '{s}'?"));
                    let replacement = match r.form {
                        Form::PName { .. } => {
                            let text_prefix = a
                                .parsed
                                .prefix_uses
                                .iter()
                                .find(|u| u.span == r.span)
                                .map(|u| u.name.to_string())
                                .unwrap_or_default();
                            format!("{text_prefix}:{s}")
                        }
                        _ => format!("<{}{s}>", v.namespace),
                    };
                    d.data = Some(json!({ "replace": replacement }));
                }
                d.message = msg;
                out.push(d);
            }
        }
    }

    for f in crate::features::shacl_lint::lint(a, k) {
        let sev = if f.error {
            DiagnosticSeverity::ERROR
        } else {
            DiagnosticSeverity::WARNING
        };
        out.push(mk(f.span, sev, f.code, f.message));
    }
    if opts.lint_undescribed_terms {
        out.extend(crate::features::lint::undescribed_terms(a, k, enc));
    }
    out
}

/// Quick fixes for the diagnostics above, plus "add all missing prefixes".
pub fn code_actions(
    a: &Analysis,
    k: &Knowledge,
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
    let mut missing: Vec<String> = Vec::new();
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
                let Some(p) = data.and_then(|x| x["prefix"].as_str()) else {
                    continue;
                };
                if missing.iter().any(|q| q == p) {
                    continue;
                }
                missing.push(p.to_string());
                // Older diagnostics carry one namespace and no candidate list.
                let single = data.map(|x| vec![json!({ "namespace": x["namespace"] })]);
                let cands = match data.map(|x| &x["candidates"]) {
                    Some(Value::Array(c)) => c.clone(),
                    _ => single.unwrap_or_default(),
                };
                for (i, c) in cands.iter().enumerate() {
                    let Some((title, ns)) = candidate_title(p, c) else {
                        continue;
                    };
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title,
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![d.clone()]),
                        edit: edit(vec![prefix_insertion(a, text, enc, p, &ns)]),
                        is_preferred: Some(i == 0),
                        ..Default::default()
                    }));
                }
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
            "unknown-term" | crate::features::lint::CODE => {
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
        if !all.iter().any(|(p, _)| *p == u.name)
            && let Some(c) = prefix_candidates(&u.name, k).into_iter().next()
        {
            all.push((u.name.to_string(), c.namespace));
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
