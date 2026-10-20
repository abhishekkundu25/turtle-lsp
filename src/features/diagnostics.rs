//! Diagnostics and the quick fixes attached to them.

use crate::analysis::{Analysis, Encoding};
use crate::jsonld;
use crate::knowledge::Knowledge;
use crate::syntax::{Form, Severity, Syntax};
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

/// Completes an `undeclared-prefix` diagnostic for prefix `name`: names the
/// best namespace in the message as a declaration in the document's syntax
/// (`keyword` is `@prefix` or `PREFIX`), and keeps every candidate in its
/// `data` (`prefix`, `namespace`, `candidates`) for the quick fixes; or
/// says where to declare it when nothing is known.
pub fn add_prefix_note(d: &mut Diagnostic, name: &str, cands: &[PrefixCandidate], keyword: &str) {
    let Some(first) = cands.first() else {
        d.message
            .push_str(". Declare it, or add it to a prefixes.ttl in the project");
        return;
    };
    d.message
        .push_str(&format!(" (add {keyword} {name}: <{}>)", first.namespace));
    d.data = Some(json!({
        "prefix": name,
        "namespace": first.namespace,
        "candidates": cands
            .iter()
            .map(|c| json!({ "namespace": c.namespace, "files": c.files }))
            .collect::<Vec<_>>(),
    }));
}

/// One written occurrence of an IRI (never `a`), for the knowledge lints
/// RDF and SPARQL documents share. `K` is how the document holds IRIs: an
/// interned [`crate::syntax::Iri`], or a `&str`.
pub struct TermUse<'a, K> {
    pub span: crate::syntax::Span,
    pub iri: K,
    /// The prefix as written, for a prefixed name (`foaf` in `foaf:name`).
    pub prefix: Option<&'a str>,
    /// JSON-LD: the IRI is written in full in a string, as this text (an
    /// absolute IRI, or a word `@vocab` or the base completes), so a fix
    /// keeps that spelling, without angle brackets.
    pub bare: Option<&'a str>,
}

/// `unknown-term` (warning, with a "did you mean" and `data.replace` in the
/// document's own spelling) and `deprecated-term` (hint, deprecated tag) for
/// terms of the well-known vocabularies, matched by namespace IRI. `mk`
/// builds a diagnostic at a span.
pub fn vocabulary_terms<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    k: &Knowledge,
    mk: &impl Fn(crate::syntax::Span, DiagnosticSeverity, &str, String) -> Diagnostic,
    out: &mut Vec<Diagnostic>,
) where
    K: std::ops::Deref<Target = str> + crate::syntax::IriKey,
{
    // Suggestions are memoised: a file can use one unknown term thousands of times.
    let mut suggestions: FxHashMap<(&str, String), Option<&str>> = FxHashMap::default();
    for u in uses {
        let Some((v, local)) = VOCABS.lookup(&u.iri) else {
            continue;
        };
        if local.is_empty() {
            continue;
        }
        match v.term(local) {
            Some(t) if t.deprecated => {
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::HINT,
                    "deprecated-term",
                    format!("'{}:{local}' is deprecated", v.prefix),
                );
                d.tags = Some(vec![DiagnosticTag::DEPRECATED]);
                out.push(d);
            }
            Some(_) => {}
            // Described by the workspace or a resolved import: newer than the snapshot.
            None if k.is_described(&u.iri) => {}
            None => {
                let mut msg = format!(
                    "'{local}' is not defined in the {} vocabulary <{}>",
                    v.prefix, v.namespace
                );
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::WARNING,
                    "unknown-term",
                    String::new(),
                );
                let suggestion = *suggestions
                    .entry((v.namespace, local.to_string()))
                    .or_insert_with(|| v.suggest(local).map(|t| t.local.as_str()));
                if let Some(s) = suggestion {
                    msg.push_str(&format!(". Did you mean '{s}'?"));
                    let replacement = match (u.prefix, u.bare) {
                        (Some(p), _) => format!("{p}:{s}"),
                        (None, Some(written)) => bare_replacement(&u.iri, written, v.namespace, s),
                        (None, None) => format!("<{}{s}>", v.namespace),
                    };
                    d.data = Some(json!({ "replace": replacement }));
                }
                d.message = msg;
                out.push(d);
            }
        }
    }
}

/// What replaces `written`, a JSON-LD string that spells `iri` in full or
/// as its end (a `@vocab` word), so it spells `namespace` + `local`: the
/// same beginning stays implied, else the full IRI.
fn bare_replacement(iri: &str, written: &str, namespace: &str, local: &str) -> String {
    let target = format!("{namespace}{local}");
    iri.strip_suffix(written)
        .and_then(|implied| target.strip_prefix(implied))
        .filter(|r| !r.is_empty())
        .map_or_else(|| target.clone(), str::to_string)
}

/// The title of the quick fix that adds candidate `c` (an `undeclared-prefix`
/// `data.candidates` entry) for `prefix`, with `keyword` `@prefix` or
/// `PREFIX`, and its namespace.
pub fn candidate_title(prefix: &str, c: &Value, keyword: &str) -> Option<(String, String)> {
    let ns = c["namespace"].as_str()?;
    let files = c["files"].as_u64().unwrap_or(0);
    let mut title = format!("Add {keyword} {prefix}: <{ns}>");
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

    let severity = |s| match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    };
    for e in &a.parsed.errors {
        out.push(mk(e.span, severity(e.severity), e.code, e.message.clone()));
    }

    // Undeclared prefixes, with "add @prefix" fixes for every known namespace.
    let mut candidates: FxHashMap<&str, Vec<PrefixCandidate>> = FxHashMap::default();
    // N-Triples and N-Quads have no prefixes at all; the parser already
    // reports the pname.
    let has_prefixes = a.parsed.syntax.has_prefixes();
    for u in a
        .parsed
        .prefix_uses
        .iter()
        .filter(|u| !u.resolved && has_prefixes)
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
        add_prefix_note(&mut d, &u.name, cands, "@prefix");
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
    let text_at = |span: crate::syntax::Span, len: u32| {
        a.text.get(span.start as usize..(span.start + len) as usize)
    };
    if a.parsed.syntax == Syntax::JsonLd {
        for f in jsonld::lint::dropped(&a.text, &a.dropped) {
            out.push(mk(f.span, severity(f.severity), f.code, f.message));
        }
        // A term of the context at its definition; anything else as written.
        let iris = jsonld::lint::vocabulary_iris(&a.text, &a.parsed.refs, &a.used_terms);
        let uses = iris.iter().map(|w| match w.form {
            Form::PName { prefix_len } => TermUse {
                span: w.span,
                iri: w.iri,
                prefix: text_at(w.span, prefix_len),
                bare: None,
            },
            _ => TermUse {
                span: w.span,
                iri: w.iri,
                prefix: None,
                bare: text_at(w.span, w.span.end - w.span.start),
            },
        });
        vocabulary_terms(uses, k, &mk, &mut out);
    } else {
        let uses = a.parsed.refs.iter().filter_map(|r| match r.form {
            Form::A => None,
            Form::PName { prefix_len } => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: text_at(r.span, prefix_len),
                bare: None,
            }),
            Form::Iri => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: None,
                bare: None,
            }),
        });
        vocabulary_terms(uses, k, &mk, &mut out);
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
                    let Some((title, ns)) = candidate_title(p, c, "@prefix") else {
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
                for c in &cands {
                    let Some(ns) = c["namespace"].as_str() else {
                        continue;
                    };
                    let Some((onto, insert)) = import_insertion(a, k, text, enc, ns) else {
                        continue;
                    };
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add prefix '{p}:' and import <{onto}>"),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![d.clone()]),
                        edit: edit(vec![prefix_insertion(a, text, enc, p, ns), insert]),
                        is_preferred: Some(false),
                        ..Default::default()
                    }));
                }
            }
            "unused-prefix" | "duplicate-prefix" => {
                let start = a.offset(enc, d.range.start).unwrap_or(0);
                let end = a.offset(enc, d.range.end).unwrap_or(0);
                let span = removal(text, crate::syntax::Span::new(start, end));
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Remove unused prefix".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: edit(vec![TextEdit {
                        range: a.range(enc, span),
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

/// What removing the directive at `span` deletes: its whole line(s) when it
/// is alone on them, else just the directive.
pub fn removal(text: &str, span: crate::syntax::Span) -> crate::syntax::Span {
    let (start, end) = (span.start as usize, span.end as usize);
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    let alone = text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
    if alone {
        crate::syntax::Span::new(line_start as u32, line_end as u32)
    } else {
        span
    }
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// For a namespace described by an ontology in the workspace or an import
/// (never a well-known vocabulary), the edit that makes this document's
/// ontology import it: `<subject> owl:imports <ontology> .` after the
/// ontology's statement. `None` when the document is no ontology, already
/// imports it, or is that ontology's own source.
fn import_insertion(
    a: &Analysis,
    k: &Knowledge,
    text: &str,
    enc: Encoding,
    ns: &str,
) -> Option<(String, TextEdit)> {
    if VOCABS.by_namespace(ns).is_some() {
        return None;
    }
    let ontology = crate::syntax::Iri::get(crate::knowledge::OWL_ONTOLOGY)?;
    let imports = crate::syntax::Iri::get(crate::imports::OWL_IMPORTS);
    // The document's ontology and its first statement ending in '.'.
    let (subject, stmt) = a.parsed.statements.iter().find_map(|st| {
        let (s, _) = st.subject.as_ref()?;
        let typed = a.types.get(s).is_some_and(|t| t.contains(&ontology));
        let closed = text[..st.span.end as usize].ends_with('.');
        (typed && closed && st.span.end > st.span.start).then_some((*s, st))
    })?;
    let imported = |o: crate::syntax::Iri| {
        a.parsed.triples.iter().any(|t| {
            Some(t.p) == imports
                && t.s == crate::syntax::Node::Iri(subject)
                && t.o == crate::syntax::Node::Iri(o)
        })
    };
    let (onto, _) = k
        .ontologies_describing(ns)
        .into_iter()
        .find(|(o, uri)| *uri != a.uri && *o != subject)?;
    if imported(onto) {
        return None;
    }
    let predicate = owl_pname(a, stmt.span.start)
        .map(|p| format!("{p}:imports"))
        .unwrap_or_else(|| format!("<{OWL}imports>"));
    let pos = a.position(enc, stmt.span.end);
    Some((
        onto.to_string(),
        TextEdit {
            range: Range::new(pos, pos),
            new_text: format!("\n<{subject}> {predicate} <{onto}> ."),
        },
    ))
}

/// A prefix name bound to the OWL namespace at offset `at` (declared before
/// it in the document, else from the project's prefixes file).
fn owl_pname(a: &Analysis, at: u32) -> Option<String> {
    let mut bound: FxHashMap<&str, &str> = FxHashMap::default();
    for p in a.parsed.prefixes.iter().filter(|p| p.span.end <= at) {
        bound.insert(&p.name, &p.iri);
    }
    let mut names: Vec<&str> = bound
        .iter()
        .filter(|(_, iri)| **iri == OWL)
        .map(|(n, _)| *n)
        .collect();
    if names.is_empty() {
        names = a
            .external
            .iter()
            .filter(|(n, iri)| *iri == OWL && !bound.contains_key(n.as_str()))
            .map(|(n, _)| n.as_str())
            .collect();
    }
    names.sort_by_key(|n| (*n != "owl", *n));
    names.first().map(|n| n.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Each diagnostic as (code, severity, the text it covers, data.replace).
    fn jsonld(text: &str) -> Vec<(String, DiagnosticSeverity, String, Option<String>)> {
        let a = Analysis::with_syntax(
            "file:///x/a.jsonld".to_string(),
            text,
            Arc::default(),
            false,
            Syntax::JsonLd,
        );
        diagnostics(&a, &Knowledge::default(), Encoding::Utf8)
            .into_iter()
            .map(|d| {
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap();
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap();
                let code = match d.code {
                    Some(NumberOrString::String(c)) => c,
                    _ => String::new(),
                };
                let replace = d
                    .data
                    .and_then(|x| x["replace"].as_str().map(str::to_string));
                (
                    code,
                    d.severity.unwrap(),
                    text[start as usize..end as usize].to_string(),
                    replace,
                )
            })
            .collect()
    }

    fn d(
        code: &str,
        severity: DiagnosticSeverity,
        at: &str,
        replace: Option<&str>,
    ) -> (String, DiagnosticSeverity, String, Option<String>) {
        (
            code.to_string(),
            severity,
            at.to_string(),
            replace.map(str::to_string),
        )
    }

    #[test]
    fn jsonld_lints_at_their_positions() {
        // LINTS of test/jsonld-fixtures.mjs.
        let text = r#"{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "vcard": "http://www.w3.org/2006/vcard/ns#",
    "weight": "ex:weight",
    "title": "rdfs:lable",
    "seeAlso": { "@id": "rdfs:seeAlso", "@type": "@id" }
  },
  "@id": "ex:w11",
  "@type": "vcard:Email",
  "weight": 3,
  "wieght": 4,
  "rdfs:labl": "Widget eleven",
  "title": "Eleven",
  "seeAlso": "http://exa mple.org/x",
  "rdfs:comment": { "@value": "Hallo", "@language": "de_DE" }
}
"#;
        use DiagnosticSeverity as S;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&format!("\"{}\"", x.2)));
        assert_eq!(
            got,
            [
                // On the definition, not on "title" where it is used.
                d("unknown-term", S::WARNING, "rdfs:lable", None),
                d("deprecated-term", S::HINT, "vcard:Email", None),
                d("jsonld-undefined-term", S::WARNING, "wieght", None),
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d("invalid-iri", S::WARNING, "http://exa mple.org/x", None),
                d("invalid-language-tag", S::WARNING, "de_DE", None),
            ]
        );
    }

    #[test]
    fn jsonld_fixes_keep_the_spelling_written() {
        use DiagnosticSeverity as S;
        // A definition written as a compact IRI, an absolute IRI and a
        // @vocab word, each fixed in its own spelling.
        let text = r#"{
  "@context": {
    "@vocab": "http://www.w3.org/2000/01/rdf-schema#",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "a": "rdfs:labl",
    "b": {"@id": "http://www.w3.org/2000/01/rdf-schema#seeAlsoo"}
  },
  "@id": "http://ex.org/x",
  "a": "x",
  "b": "y",
  "a2": "z",
  "commnt": "w"
}"#;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&x.2));
        assert_eq!(
            got,
            [
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d(
                    "unknown-term",
                    S::WARNING,
                    "http://www.w3.org/2000/01/rdf-schema#seeAlsoo",
                    Some("http://www.w3.org/2000/01/rdf-schema#seeAlso")
                ),
                d("unknown-term", S::WARNING, "a2", None),
                d("unknown-term", S::WARNING, "commnt", Some("comment")),
            ]
        );
    }

    #[test]
    fn a_bare_replacement_keeps_what_is_implied() {
        let schema = "http://schema.org/";
        let r = |iri, written| bare_replacement(iri, written, schema, "Person");
        assert_eq!(r("http://schema.org/Persn", "Persn"), "Person");
        assert_eq!(
            r("http://schema.org/Persn", "http://schema.org/Persn"),
            "http://schema.org/Person"
        );
        // Written escaped, or under another namespace: the full IRI.
        assert_eq!(
            r("http://schema.org/Persn", "Pers\\u006E"),
            "http://schema.org/Person"
        );
        assert_eq!(
            bare_replacement("https://schema.org/Persn", "Persn", schema, "Person"),
            "http://schema.org/Person"
        );
    }
}
