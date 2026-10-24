//! Quick fixes for the diagnostics of [`super::diagnostics`], and the
//! contract between the two: the [`FixData`] a diagnostic carries in its
//! `data`, written and read in this module only.
//!
//! [`Fixer::for_diagnostic`] gives the fixes of one diagnostic and costs the
//! same however many diagnostics the file has; [`Fixer::add_all_missing`] is
//! the one file-wide action. [`code_actions`] answers `textDocument/codeAction`
//! with both.

use super::diagnostics::SOURCE;
use crate::analysis::{Analysis, Encoding};
use crate::features::refactor::text_edits_in;
use crate::jsonld;
use crate::knowledge::Knowledge;
use crate::syntax::lexer::Token;
use crate::syntax::names::{declaration_edit, declaration_keyword, declarations_edit, newline};
use crate::syntax::{Iri, Node, Span};
use crate::vocab::VOCABS;
use lsp_types::*;
use rustc_hash::FxHashMap;
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};

/// A namespace to bind an undeclared prefix name to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PrefixCandidate {
    pub(crate) namespace: String,
    /// Workspace files that bind the name to it (0 for a well-known one the
    /// project does not use).
    pub(crate) files: u32,
}

/// What a diagnostic tells the quick fix for it, kept in `Diagnostic.data`.
/// The JSON shape of each variant is the wire contract with the clients,
/// which send the diagnostics back with `textDocument/codeAction`; the code
/// of the diagnostic tells which variant it is ([`FixData::of`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum FixData {
    /// `undeclared-prefix`: the namespaces `prefix` could be bound to, best
    /// first (`namespace` repeats the first).
    UndeclaredPrefix {
        prefix: String,
        namespace: String,
        candidates: Vec<PrefixCandidate>,
    },
    /// `jsonld-undefined-prefix`: `prefix` is not in the `@context`, and the
    /// workspace knows `namespace` for it.
    UndefinedPrefix { prefix: String, namespace: String },
    /// `unknown-term` and `undescribed-term`: the text that replaces the
    /// diagnostic's range.
    Replace { replace: String },
}

impl FixData {
    /// The `data` of a diagnostic.
    pub(crate) fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("fix data serialises")
    }

    /// Set `d.data` to this.
    pub(crate) fn attach(&self, d: &mut Diagnostic) {
        d.data = Some(self.to_value());
    }

    /// The fix data of `d`, if it is a diagnostic of ours that carries some.
    pub(crate) fn of(d: &Diagnostic) -> Option<FixData> {
        if d.source.as_deref() != Some(SOURCE) {
            return None;
        }
        let Some(NumberOrString::String(code)) = &d.code else {
            return None;
        };
        let data = d.data.as_ref()?;
        let text = |key: &str| data[key].as_str().map(str::to_string);
        match code.as_str() {
            "undeclared-prefix" => {
                // Older diagnostics carry one namespace and no candidate list.
                let candidates: Vec<PrefixCandidate> = match &data["candidates"] {
                    Value::Array(list) => list
                        .iter()
                        .filter_map(|c| {
                            Some(PrefixCandidate {
                                namespace: c["namespace"].as_str()?.to_string(),
                                files: c["files"].as_u64().unwrap_or(0) as u32,
                            })
                        })
                        .collect(),
                    _ => text("namespace")
                        .map(|namespace| PrefixCandidate {
                            namespace,
                            files: 0,
                        })
                        .into_iter()
                        .collect(),
                };
                Some(FixData::UndeclaredPrefix {
                    prefix: text("prefix")?,
                    namespace: text("namespace")
                        .or_else(|| candidates.first().map(|c| c.namespace.clone()))
                        .unwrap_or_default(),
                    candidates,
                })
            }
            jsonld::lint::UNDEFINED_PREFIX => Some(FixData::UndefinedPrefix {
                prefix: text("prefix")?,
                namespace: text("namespace")?,
            }),
            "unknown-term" | crate::features::lint::CODE => Some(FixData::Replace {
                replace: text("replace")?,
            }),
            _ => None,
        }
    }

    /// The prefix this fix declares: fixes of one prefix are offered once.
    pub(crate) fn prefix(&self) -> Option<&str> {
        match self {
            FixData::UndeclaredPrefix { prefix, .. } | FixData::UndefinedPrefix { prefix, .. } => {
                Some(prefix)
            }
            FixData::Replace { .. } => None,
        }
    }
}

/// Namespaces for prefix `name`, best first: the project's most common
/// binding, then the well-known vocabulary's if different.
pub(crate) fn prefix_candidates(name: &str, k: &Knowledge) -> Vec<PrefixCandidate> {
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
/// [`FixData`] for the quick fixes; or says where to declare it when nothing
/// is known.
pub(crate) fn add_prefix_note(
    d: &mut Diagnostic,
    name: &str,
    cands: &[PrefixCandidate],
    keyword: &str,
) {
    let Some(first) = cands.first() else {
        d.message
            .push_str(". Declare it, or add it to a prefixes.ttl in the project");
        return;
    };
    d.message
        .push_str(&format!(" (add {keyword} {name}: <{}>)", first.namespace));
    FixData::UndeclaredPrefix {
        prefix: name.to_string(),
        namespace: first.namespace.clone(),
        candidates: cands.to_vec(),
    }
    .attach(d);
}

/// The title of the quick fix that adds candidate `c` for `prefix`, with
/// `keyword` `@prefix` or `PREFIX`.
pub(crate) fn candidate_title(prefix: &str, c: &PrefixCandidate, keyword: &str) -> String {
    let mut title = format!("Add {keyword} {prefix}: <{}>", c.namespace);
    match c.files {
        0 => {}
        1 => title.push_str(" (used in 1 file)"),
        n => title.push_str(&format!(" (used in {n} files)")),
    }
    title
}

/// What removing the directive at `span` deletes: its whole line(s) when it
/// is alone on them, else just the directive.
pub(crate) fn removal(text: &str, span: Span) -> Span {
    let (start, end) = (span.start as usize, span.end as usize);
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    let alone = text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
    if alone {
        Span::new(line_start as u32, line_end as u32)
    } else {
        span
    }
}

/// The quick fixes of one RDF document. What it works out about the document
/// (its ontology, the imports a namespace could come from) it keeps, so the
/// fixes of thousands of diagnostics cost no more than one each.
pub(crate) struct Fixer<'a> {
    a: &'a Analysis,
    k: &'a Knowledge,
    enc: Encoding,
    uri: &'a Uri,
    /// The document's ontology and its first statement ending in '.'.
    ontology: OnceCell<Option<(Iri, Span)>>,
    /// [`Fixer::import_insertion`] by namespace.
    imports: RefCell<FxHashMap<String, Option<(String, TextEdit)>>>,
    /// Where each undeclared prefix is first used.
    first_uses: OnceCell<FxHashMap<&'a str, u32>>,
    /// [`Fixer::declaration`] by prefix, namespace and the offset it serves.
    declarations: RefCell<FxHashMap<(String, String, u32), TextEdit>>,
    /// The document's tokens, lexed once for all the fixes when it keeps none.
    tokens: OnceCell<Cow<'a, [Token]>>,
}

impl<'a> Fixer<'a> {
    /// A fixer for `a`, whose edits go to `uri`.
    pub(crate) fn new(a: &'a Analysis, k: &'a Knowledge, enc: Encoding, uri: &'a Uri) -> Fixer<'a> {
        Fixer {
            a,
            k,
            enc,
            uri,
            ontology: OnceCell::new(),
            imports: RefCell::default(),
            first_uses: OnceCell::new(),
            declarations: RefCell::default(),
            tokens: OnceCell::new(),
        }
    }

    /// Edits replacing whole tokens, through [`text_edits_in`].
    fn text_edits(&self, reps: Vec<(Span, String)>) -> Vec<TextEdit> {
        let tokens = self.tokens.get_or_init(|| self.a.tokens_or_lexed());
        text_edits_in(self.a, tokens, self.enc, reps)
    }

    /// The edit that declares `prefix` as `ns` for a use at `at`. Every use
    /// of one prefix gets the same edit (it goes before the prefix's first
    /// use), so it is worked out once, not once per use.
    fn declaration(&self, prefix: &str, ns: &str, at: u32) -> TextEdit {
        let first = self.first_uses.get_or_init(|| {
            let mut first: FxHashMap<&str, u32> = FxHashMap::default();
            for u in self.a.parsed.prefix_uses.iter().filter(|u| !u.resolved) {
                let at = first.entry(&u.name).or_insert(u.span.start);
                *at = (*at).min(u.span.start);
            }
            first
        });
        let before = first.get(prefix).map_or(at, |f| (*f).min(at));
        let key = (prefix.to_string(), ns.to_string(), before);
        if let Some(edit) = self.declarations.borrow().get(&key) {
            return edit.clone();
        }
        let edit = declaration_edit(self.a, self.enc, prefix, ns, before);
        self.declarations.borrow_mut().insert(key, edit.clone());
        edit
    }

    fn edit(&self, edits: Vec<TextEdit>) -> Option<WorkspaceEdit> {
        let mut changes = std::collections::HashMap::new();
        changes.insert(self.uri.clone(), edits);
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }

    fn quick(
        &self,
        title: String,
        d: &Diagnostic,
        edits: Vec<TextEdit>,
        preferred: bool,
    ) -> CodeAction {
        CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![d.clone()]),
            edit: self.edit(edits),
            is_preferred: Some(preferred),
            ..Default::default()
        }
    }

    /// The quick fixes for `d` alone, preferred first among them as the
    /// diagnostic's kind lists them. Nothing here looks at the other
    /// diagnostics or at the file as a whole.
    pub(crate) fn for_diagnostic(&self, d: &Diagnostic) -> Vec<CodeAction> {
        if d.source.as_deref() != Some(SOURCE) {
            return Vec::new();
        }
        let Some(NumberOrString::String(code)) = &d.code else {
            return Vec::new();
        };
        let (a, enc) = (self.a, self.enc);
        match code.as_str() {
            "undeclared-prefix" => {
                let Some(FixData::UndeclaredPrefix {
                    prefix, candidates, ..
                }) = FixData::of(d)
                else {
                    return Vec::new();
                };
                let at = a.offset(enc, d.range.start).unwrap_or(u32::MAX);
                // The title names what the edit inserts.
                let keyword = declaration_keyword(a);
                let mut out = Vec::new();
                for (i, c) in candidates.iter().enumerate() {
                    let edit = self.declaration(&prefix, &c.namespace, at);
                    out.push(self.quick(
                        candidate_title(&prefix, c, keyword),
                        d,
                        vec![edit],
                        i == 0,
                    ));
                }
                for c in &candidates {
                    let Some((onto, insert)) = self.import_insertion(&c.namespace) else {
                        continue;
                    };
                    let edit = self.declaration(&prefix, &c.namespace, at);
                    out.push(self.quick(
                        format!("Add prefix '{prefix}:' and import <{onto}>"),
                        d,
                        vec![edit, insert],
                        false,
                    ));
                }
                out
            }
            jsonld::lint::UNDEFINED_PREFIX => {
                let Some(FixData::UndefinedPrefix { prefix, namespace }) = FixData::of(d) else {
                    return Vec::new();
                };
                let Some(offset) = a.offset(enc, d.range.start) else {
                    return Vec::new();
                };
                let member = format!("{}: {}", Value::from(prefix), Value::from(namespace));
                let (at, del, new_text) = jsonld::context_edit::add_member_in(
                    &a.source_index,
                    &a.text,
                    offset as usize,
                    &member,
                );
                let edit = TextEdit {
                    range: a.range(enc, Span::new(at as u32, (at + del) as u32)),
                    new_text,
                };
                vec![self.quick(format!("Add {member} to the @context"), d, vec![edit], true)]
            }
            "unused-prefix" | "duplicate-prefix" => {
                let title = if code == "unused-prefix" {
                    "Remove unused prefix"
                } else {
                    "Remove duplicate prefix"
                };
                let start = a.offset(enc, d.range.start).unwrap_or(0);
                let end = a.offset(enc, d.range.end).unwrap_or(0);
                let span = removal(&a.text, Span::new(start, end));
                let edits = self.text_edits(vec![(span, String::new())]);
                vec![self.quick(title.to_string(), d, edits, true)]
            }
            "unknown-term" | crate::features::lint::CODE => {
                let Some(FixData::Replace { replace }) = FixData::of(d) else {
                    return Vec::new();
                };
                let (Some(start), Some(end)) =
                    (a.offset(enc, d.range.start), a.offset(enc, d.range.end))
                else {
                    return Vec::new();
                };
                let edits = self.text_edits(vec![(Span::new(start, end), replace.clone())]);
                vec![self.quick(format!("Change to '{replace}'"), d, edits, true)]
            }
            _ => Vec::new(),
        }
    }

    /// "Add all missing prefixes (n)" when the file has more than one
    /// undeclared prefix with a namespace to add; none for N-Triples and
    /// N-Quads, which have no prefixes to add. It reads the whole file, so
    /// it is made once per request, not once per diagnostic.
    pub(crate) fn add_all_missing(&self) -> Option<CodeAction> {
        let a = self.a;
        if !a.parsed.syntax.has_prefixes() {
            return None;
        }
        let mut all: Vec<(String, String)> = Vec::new();
        for u in a.parsed.prefix_uses.iter().filter(|u| !u.resolved) {
            if !all.iter().any(|(p, _)| *p == u.name)
                && let Some(c) = prefix_candidates(&u.name, self.k).into_iter().next()
            {
                all.push((u.name.to_string(), c.namespace));
            }
        }
        if all.len() < 2 {
            return None;
        }
        let decls: Vec<(&str, &str)> = all.iter().map(|(p, ns)| (&**p, &**ns)).collect();
        let edits = vec![declarations_edit(a, self.enc, &decls, u32::MAX)];
        Some(CodeAction {
            title: format!("Add all missing prefixes ({})", all.len()),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: self.edit(edits),
            ..Default::default()
        })
    }

    /// For a namespace described by an ontology in the workspace or an import
    /// (never a well-known vocabulary), the edit that makes this document's
    /// ontology import it: `<subject> owl:imports <ontology> .` after the
    /// ontology's statement. `None` when the document is no ontology, already
    /// imports it, or is that ontology's own source.
    fn import_insertion(&self, ns: &str) -> Option<(String, TextEdit)> {
        if let Some(known) = self.imports.borrow().get(ns) {
            return known.clone();
        }
        let found = self.find_import_insertion(ns);
        self.imports
            .borrow_mut()
            .insert(ns.to_string(), found.clone());
        found
    }

    fn find_import_insertion(&self, ns: &str) -> Option<(String, TextEdit)> {
        let (a, k) = (self.a, self.k);
        if VOCABS.by_namespace(ns).is_some() {
            return None;
        }
        let (subject, stmt_span) = (*self.ontology.get_or_init(|| document_ontology(a)))
            .as_ref()
            .copied()?;
        let imports = Iri::get(crate::imports::OWL_IMPORTS);
        let imported = |o: Iri| {
            a.parsed
                .triples
                .iter()
                .any(|t| Some(t.p) == imports && t.s == Node::Iri(subject) && t.o == Node::Iri(o))
        };
        let (onto, _) = k
            .ontologies_describing(ns)
            .into_iter()
            .find(|(o, uri)| *uri != a.uri && *o != subject)?;
        if imported(onto) {
            return None;
        }
        let predicate = owl_pname(a, stmt_span.start)
            .map(|p| format!("{p}:imports"))
            .unwrap_or_else(|| format!("<{OWL}imports>"));
        let pos = a.position(self.enc, stmt_span.end);
        Some((
            onto.to_string(),
            TextEdit {
                range: Range::new(pos, pos),
                new_text: format!("{}<{subject}> {predicate} <{onto}> .", newline(&a.text)),
            },
        ))
    }
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// The document's ontology and the span of its first statement that ends
/// in '.'.
fn document_ontology(a: &Analysis) -> Option<(Iri, Span)> {
    let ontology = Iri::get(crate::knowledge::OWL_ONTOLOGY)?;
    a.parsed.statements.iter().find_map(|st| {
        let (s, _) = st.subject.as_ref()?;
        let typed = a.types.get(s).is_some_and(|t| t.contains(&ontology));
        let closed = a.text[..st.span.end as usize].ends_with('.');
        (typed && closed && st.span.end > st.span.start).then_some((*s, st.span))
    })
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

/// Quick fixes for the diagnostics of `params`, plus "add all missing
/// prefixes". A prefix several diagnostics miss is fixed once, at the first.
pub fn code_actions(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    params: &CodeActionParams,
) -> Vec<CodeActionOrCommand> {
    let fixer = Fixer::new(a, k, enc, &params.text_document.uri);
    let mut actions = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for d in &params.context.diagnostics {
        if let Some(prefix) = FixData::of(d).as_ref().and_then(FixData::prefix) {
            if missing.iter().any(|q| q == prefix) {
                continue;
            }
            missing.push(prefix.to_string());
        }
        actions.extend(
            fixer
                .for_diagnostic(d)
                .into_iter()
                .map(CodeActionOrCommand::CodeAction),
        );
    }
    actions.extend(fixer.add_all_missing().map(CodeActionOrCommand::CodeAction));
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::diagnostics::diagnostics;
    use std::sync::Arc;

    /// The quick fixes offered for every diagnostic of `text`.
    fn fixes(uri: &str, text: &str) -> (Analysis, Vec<CodeAction>) {
        let a = Analysis::new(uri.into(), text, Arc::default(), true);
        let k = Knowledge::default();
        let diagnostics = diagnostics(&a, &k, Encoding::Utf8);
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.parse().unwrap(),
            },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let actions = code_actions(&a, &k, Encoding::Utf8, &params)
            .into_iter()
            .filter_map(|c| match c {
                CodeActionOrCommand::CodeAction(c) => Some(c),
                _ => None,
            })
            .collect();
        (a, actions)
    }

    /// The text after applying the edit of the action titled `title`.
    fn applied(a: &Analysis, actions: &[CodeAction], title: &str) -> String {
        let action = actions
            .iter()
            .find(|c| c.title.starts_with(title))
            .unwrap_or_else(|| {
                let titles: Vec<_> = actions.iter().map(|c| &c.title).collect();
                panic!("no {title:?} in {titles:?}")
            });
        let edits: Vec<TextEdit> = action
            .edit
            .clone()
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .flatten()
            .collect();
        crate::features::refactor::apply(a, Encoding::Utf8, &edits)
    }

    #[test]
    fn the_prefix_fix_declares_above_the_first_use() {
        let text = "@prefix a: <http://a/> .\nfoaf:x a:p a:o .\n@prefix b: <http://b/> .\nb:y a:p foaf:z .\n";
        let (a, actions) = fixes("file:///t/doc.ttl", text);
        assert_eq!(
            applied(&a, &actions, "Add @prefix foaf:"),
            "@prefix a: <http://a/> .\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\nfoaf:x a:p a:o .\n@prefix b: <http://b/> .\nb:y a:p foaf:z .\n"
        );
        // With no declaration above the use, the top of the file.
        let text = "# header\nfoaf:x a:p a:o .\n@prefix a: <http://a/> .\n";
        let (a, actions) = fixes("file:///t/doc.ttl", text);
        assert_eq!(
            applied(&a, &actions, "Add @prefix foaf:"),
            "# header\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\nfoaf:x a:p a:o .\n@prefix a: <http://a/> .\n"
        );
    }

    #[test]
    fn inserted_prefix_lines_follow_the_files_line_ending() {
        let text = "@prefix a: <http://a/> .\r\nfoaf:x a:p rdfs:z .\r\n";
        let (a, actions) = fixes("file:///t/doc.ttl", text);
        let one = applied(&a, &actions, "Add @prefix foaf:");
        assert_eq!(
            one,
            "@prefix a: <http://a/> .\r\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\r\nfoaf:x a:p rdfs:z .\r\n"
        );
        let all = applied(&a, &actions, "Add all missing prefixes");
        assert_eq!(
            all.matches('\n').count(),
            all.matches("\r\n").count(),
            "{all:?}"
        );
        assert!(all.contains("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\r\n"));
    }

    #[test]
    fn the_prefix_fix_lands_after_a_lone_cr_line_end() {
        let text = "# c\rex:s sd:p sd:o .";
        let (a, actions) = fixes("file:///t/doc.ttl", text);
        let action = actions
            .iter()
            .find(|c| c.title.starts_with("Add @prefix sd:"));
        let edits = action.unwrap().edit.clone().unwrap().changes.unwrap();
        let start = edits.into_values().flatten().next().unwrap().range.start;
        assert_eq!(start, Position::new(1, 0));
        let fixed = applied(&a, &actions, "Add @prefix sd:");
        assert!(fixed.starts_with("# c\r@prefix sd: <"), "{fixed:?}");
    }

    #[test]
    fn add_all_writes_no_prefixes_into_line_based_syntaxes() {
        let text = "foaf:x rdfs:label foaf:z .\n";
        let (_, ttl) = fixes("file:///t/doc.ttl", text);
        assert!(
            ttl.iter()
                .any(|c| c.title.starts_with("Add all missing prefixes (2)")),
            "{:?}",
            ttl.iter().map(|c| &c.title).collect::<Vec<_>>()
        );
        for uri in ["file:///t/doc.nt", "file:///t/doc.nq"] {
            let (_, actions) = fixes(uri, text);
            assert!(
                actions.iter().all(|c| !c.title.contains("prefix")),
                "{uri}: {:?}",
                actions.iter().map(|c| &c.title).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn the_duplicate_prefix_fix_is_not_titled_unused() {
        let text = "@prefix a: <http://a/> .\n@prefix a: <http://a/> .\na:x a:p a:o .\n";
        let (a, actions) = fixes("file:///t/doc.ttl", text);
        assert!(actions.iter().all(|c| c.title != "Remove unused prefix"));
        assert_eq!(
            applied(&a, &actions, "Remove duplicate prefix"),
            "@prefix a: <http://a/> .\na:x a:p a:o .\n"
        );
        let text = "@prefix a: <http://a/> .\n@prefix b: <http://b/> .\na:x a:p a:o .\n";
        let (_, actions) = fixes("file:///t/doc.ttl", text);
        assert!(actions.iter().any(|c| c.title == "Remove unused prefix"));
    }

    #[test]
    fn a_jsonld_context_member_is_added_from_the_documents_index() {
        use crate::syntax::Syntax;
        let text = "{\n  \"@context\": {\n    \"ex\": \"http://e.org/\"\n  },\n  \"@id\": \"ex:a\",\n  \"rdfs:label\": \"x\"\n}\n";
        let uri = "file:///t/doc.jsonld";
        let a = Analysis::with_syntax(uri.into(), text, Arc::default(), true, Syntax::JsonLd);
        let k = Knowledge::default();
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.parse().unwrap(),
            },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics: diagnostics(&a, &k, Encoding::Utf8),
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let actions: Vec<CodeAction> = code_actions(&a, &k, Encoding::Utf8, &params)
            .into_iter()
            .filter_map(|c| match c {
                CodeActionOrCommand::CodeAction(c) => Some(c),
                _ => None,
            })
            .collect();
        let fixed = applied(&a, &actions, "Add \"rdfs\"");
        assert!(
            fixed.contains("\"rdfs\": \"http://www.w3.org/2000/01/rdf-schema#\""),
            "{fixed}"
        );
        assert!(
            crate::jsonld::lint::undefined_prefixes(
                &fixed,
                &Analysis::with_syntax(uri.into(), &fixed, Arc::default(), true, Syntax::JsonLd)
                    .source_index
            )
            .is_empty()
        );
    }

    #[test]
    fn fix_data_keeps_its_wire_shape() {
        use serde_json::json;
        let cands = vec![
            PrefixCandidate {
                namespace: "http://a/".into(),
                files: 2,
            },
            PrefixCandidate {
                namespace: "http://b/".into(),
                files: 0,
            },
        ];
        let mut d = Diagnostic {
            source: Some(SOURCE.into()),
            code: Some(NumberOrString::String("undeclared-prefix".into())),
            ..Default::default()
        };
        add_prefix_note(&mut d, "p", &cands, "@prefix");
        assert_eq!(
            d.data,
            Some(json!({
                "prefix": "p",
                "namespace": "http://a/",
                "candidates": [
                    {"namespace": "http://a/", "files": 2},
                    {"namespace": "http://b/", "files": 0},
                ],
            }))
        );
        assert_eq!(
            FixData::of(&d),
            Some(FixData::UndeclaredPrefix {
                prefix: "p".into(),
                namespace: "http://a/".into(),
                candidates: cands,
            })
        );
        // An older diagnostic: one namespace, no candidate list.
        d.data = Some(json!({"prefix": "p", "namespace": "http://old/"}));
        let Some(FixData::UndeclaredPrefix { candidates, .. }) = FixData::of(&d) else {
            panic!("{:?}", d.data)
        };
        assert_eq!(candidates[0].namespace, "http://old/");
        // The code decides the variant; a foreign diagnostic carries none.
        let mut r = Diagnostic {
            source: Some(SOURCE.into()),
            code: Some(NumberOrString::String("unknown-term".into())),
            ..Default::default()
        };
        FixData::Replace {
            replace: "rdfs:label".into(),
        }
        .attach(&mut r);
        assert_eq!(r.data, Some(json!({"replace": "rdfs:label"})));
        assert!(matches!(FixData::of(&r), Some(FixData::Replace { .. })));
        r.source = Some("other".into());
        assert_eq!(FixData::of(&r), None);
    }

    /// A document kept without its tokens is lexed once for all its fixes,
    /// not once per fix.
    #[test]
    fn the_fixes_of_a_document_without_tokens_lex_it_once() {
        let unused: String = (0..200)
            .map(|i| format!("@prefix p{i}: <http://example.org/p{i}/> .\n"))
            .collect();
        let text = format!("{unused}<http://example.org/s> <http://example.org/p> 1 .\n");
        let uri: Uri = "file:///t/doc.ttl".parse().unwrap();
        let a = Analysis::new(uri.to_string(), &text, Arc::default(), false);
        let k = Knowledge::default();
        let found = diagnostics(&a, &k, Encoding::Utf8);
        let fixer = Fixer::new(&a, &k, Encoding::Utf8, &uri);
        crate::syntax::lexer::LEXED.with(|c| c.set(0));
        let fixes: usize = found.iter().map(|d| fixer.for_diagnostic(d).len()).sum();
        assert_eq!(fixes, 200);
        let lexed = crate::syntax::lexer::LEXED.with(|c| c.get());
        // The text once, then a line or so around each fix.
        assert!(
            lexed < 4 * text.len(),
            "{lexed} bytes lexed for {}",
            text.len()
        );
        assert!(!a.has_tokens());
    }

    #[test]
    fn a_diagnostics_fixes_do_not_include_the_file_wide_action() {
        let text = "foaf:x rdfs:label foaf:z .\n";
        let uri: Uri = "file:///t/doc.ttl".parse().unwrap();
        let a = Analysis::new(uri.to_string(), text, Arc::default(), true);
        let k = Knowledge::default();
        let fixer = Fixer::new(&a, &k, Encoding::Utf8, &uri);
        let found = diagnostics(&a, &k, Encoding::Utf8);
        let undeclared: Vec<_> = found
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("undeclared-prefix".into())))
            .collect();
        assert!(undeclared.len() >= 2);
        for d in undeclared {
            let fixes = fixer.for_diagnostic(d);
            assert!(!fixes.is_empty());
            assert!(
                fixes
                    .iter()
                    .all(|f| f.diagnostics.as_deref() == Some(std::slice::from_ref(d))),
                "{fixes:?}"
            );
        }
        assert!(fixer.add_all_missing().is_some());
    }

    #[test]
    fn the_prefix_fix_never_splits_a_statement_or_string() {
        for text in [
            "@prefix ex: <http://ex/> . ex:s\n  foaf:knows ex:o .\n",
            "@prefix ex: <http://ex/> . ex:s ex:p \"\"\"a\nb\"\"\" .\nex:t foaf:knows ex:o .\n",
        ] {
            let (a, actions) = fixes("file:///t/doc.ttl", text);
            let after = applied(&a, &actions, "Add @prefix foaf:");
            let p = crate::syntax::parse(&after, &Default::default(), Default::default());
            assert!(p.errors.is_empty(), "{after:?}");
            assert!(p.prefix_uses.iter().all(|u| u.resolved), "{after:?}");
        }
    }

    /// The edit of the code action titled `title` among `actions`, applied
    /// to the query `q`.
    fn applied_to_query(
        q: &crate::analysis::SparqlAnalysis,
        actions: Vec<CodeActionOrCommand>,
        title: &str,
    ) -> String {
        let action = actions
            .into_iter()
            .find_map(|c| match c {
                CodeActionOrCommand::CodeAction(c) if c.title.starts_with(title) => Some(c),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {title:?}"));
        let mut edits: Vec<(u32, u32, String)> = action
            .edit
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .flatten()
            .map(|e| {
                let at = |p| q.offset(Encoding::Utf8, p).unwrap();
                (at(e.range.start), at(e.range.end), e.new_text)
            })
            .collect();
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        let mut out = q.text().to_string();
        for (s, e, t) in edits {
            out.replace_range(s as usize..e as usize, &t);
        }
        out
    }

    /// Removing a directive between two names keeps them apart.
    #[test]
    fn a_removed_prefix_leaves_its_neighbours_apart() {
        for (text, want) in [
            (
                "@prefix ex: <http://e/> .\nex:s ex:p ex:o.@prefix u: <http://u/> .ex:t ex:p ex:o .\n",
                "@prefix ex: <http://e/> .\nex:s ex:p ex:o. ex:t ex:p ex:o .\n",
            ),
            (
                "@prefix ex: <http://e/> .\n@prefix e1: <http://e1/> .\nex:s ex:p 1.PREFIX u: <http://u/>e1:x ex:p ex:o .\n",
                "@prefix ex: <http://e/> .\n@prefix e1: <http://e1/> .\nex:s ex:p 1. e1:x ex:p ex:o .\n",
            ),
        ] {
            let (a, actions) = fixes("file:///t/doc.ttl", text);
            let out = applied(&a, &actions, "Remove unused prefix");
            assert_eq!(out, want);
            let p = crate::syntax::parse(&out, &Default::default(), Default::default());
            assert!(p.errors.is_empty() && p.triples.len() == 2, "{out}");
        }
        let text = "PREFIX ex: <http://e/>\nPREFIX u: <http://u/>SELECT * { ex:s ?p ?o }\n";
        let q = crate::analysis::SparqlAnalysis::new("file:///t/q.rq".into(), text, Arc::default());
        let k = Knowledge::default();
        let diagnostics =
            crate::sparql::features::diagnostics(&q, &k, Encoding::Utf8, &Default::default());
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t/q.rq".parse().unwrap(),
            },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let actions = crate::sparql::features::code_actions(&q, &k, Encoding::Utf8, &params, false);
        assert_eq!(
            applied_to_query(&q, actions, "Remove unused prefix"),
            "PREFIX ex: <http://e/>\nSELECT * { ex:s ?p ?o }\n"
        );
    }

    /// "Change to" a full IRI after `<` leaves `<` the operator: the query parses.
    #[test]
    fn a_change_to_a_full_iri_keeps_the_operator_apart() {
        let text = "PREFIX ex: <http://e/>\nSELECT * { ?a ex:p ?o FILTER(?a<ex:typo) }\n";
        let q = crate::analysis::SparqlAnalysis::new("file:///t/q.rq".into(), text, Arc::default());
        let at = text.find("ex:typo").unwrap() as u32;
        let mut d = Diagnostic {
            range: Range::new(
                q.position(Encoding::Utf8, at),
                q.position(Encoding::Utf8, at + 7),
            ),
            code: Some(NumberOrString::String("unknown-term".into())),
            source: Some(SOURCE.into()),
            ..Default::default()
        };
        FixData::Replace {
            replace: "<http://f/type>".into(),
        }
        .attach(&mut d);
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t/q.rq".parse().unwrap(),
            },
            range: d.range,
            context: CodeActionContext {
                diagnostics: vec![d],
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let k = Knowledge::default();
        let actions = crate::sparql::features::code_actions(&q, &k, Encoding::Utf8, &params, false);
        let out = applied_to_query(&q, actions, "Change to");
        assert_eq!(
            out,
            "PREFIX ex: <http://e/>\nSELECT * { ?a ex:p ?o FILTER(?a<<http://f/type>) }\n"
        );
        assert!(
            spargebra::SparqlParser::new().parse_query(&out).is_ok(),
            "{out}"
        );
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::features::diagnostics::diagnostics;
    use std::sync::Arc;

    /// The fix's title names the keyword the edit inserts: `PREFIX` in a
    /// document whose declarations are SPARQL-style.
    #[test]
    fn the_prefix_fix_is_titled_with_the_keyword_it_inserts() {
        for (text, keyword) in [
            ("PREFIX a: <http://a/>\nfoaf:x a:p a:o .\n", "PREFIX"),
            ("@prefix a: <http://a/> .\nfoaf:x a:p a:o .\n", "@prefix"),
        ] {
            let uri: Uri = "file:///t/doc.ttl".parse().unwrap();
            let a = Analysis::new(uri.to_string(), text, Arc::default(), true);
            let k = Knowledge::default();
            let fixer = Fixer::new(&a, &k, Encoding::Utf8, &uri);
            let found = diagnostics(&a, &k, Encoding::Utf8);
            let d = found
                .iter()
                .find(|d| d.code == Some(NumberOrString::String("undeclared-prefix".into())))
                .unwrap();
            let fix = &fixer.for_diagnostic(d)[0];
            let inserted: Vec<String> = fix
                .edit
                .clone()
                .unwrap()
                .changes
                .unwrap()
                .into_values()
                .flatten()
                .map(|e| e.new_text)
                .collect();
            assert!(
                inserted[0].starts_with(&format!("{keyword} foaf:")),
                "{inserted:?}"
            );
            assert!(
                fix.title.starts_with(&format!("Add {keyword} foaf: <")),
                "{}",
                fix.title
            );
        }
    }
}
