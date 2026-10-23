//! Refactorings: organise the prefix block, shorten and expand IRIs, and
//! extract a prefix for a namespace. Every edit keeps the graph the same.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::lexer::{Kind, Token};
use crate::syntax::names::{
    Scope, declarable_namespace, declaration_edit, declaration_point, is_pn_local, is_pn_prefix,
    newline, shorten, turtle_token_edits,
};
use crate::syntax::{Form, PrefixDecl, Ref, Span};
use crate::vocab::VOCABS;
use crate::workspace::{PREFIX_FILE, file_uri_name};
use lsp_types::*;
use rustc_hash::FxHashSet;
use serde_json::json;

const SHORTEN_ALL: &str = "shorten-all";
const EXTRACT: &str = "extract";

/// Refactorings for the request's range. With `lazy`, the file-wide actions
/// (whose edits can be large) carry no edit; `resolve` computes it.
pub fn code_actions(
    a: &Analysis,
    enc: Encoding,
    params: &CodeActionParams,
    lazy: bool,
) -> Vec<CodeActionOrCommand> {
    // Every refactoring here reads or writes prefixes, which N-Triples and N-Quads lack.
    if !a.parsed.syntax.has_prefixes() {
        return Vec::new();
    }
    let uri = &params.text_document.uri;
    let only = params.context.only.as_deref();
    let mut out = Vec::new();
    let mut push = |kind: CodeActionKind, title: String, edits: Option<Vec<TextEdit>>, data| {
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title,
            kind: Some(kind),
            edit: edits.map(|e| workspace_edit(uri, e)),
            data,
            ..Default::default()
        }));
    };

    if wanted(only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
        && let Some(edit) = organise(a, enc)
    {
        push(
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            "Organise prefixes".into(),
            Some(vec![edit]),
            None,
        );
    }
    if !wanted(only, &CodeActionKind::REFACTOR_REWRITE) {
        return out;
    }

    let start = a.offset(enc, params.range.start).unwrap_or(0);
    let end = a.offset(enc, params.range.end).unwrap_or(start).max(start);
    if let Some(r) = ref_in(a, start, end) {
        match r.form {
            // A relative `<...>` stays relative: a prefix for its resolved IRI
            // would tie the file to where it lives.
            Form::Iri if !written_absolute(a, r) => {}
            Form::Iri => {
                let scope = Scope::at(a, r.span.start);
                if let Some(pname) = shorten(&r.iri, &scope) {
                    push(
                        CodeActionKind::REFACTOR_REWRITE,
                        format!("Shorten IRI to {pname}"),
                        Some(text_edits(a, enc, vec![(r.span, pname)])),
                        None,
                    );
                } else if let Some((ns, name)) = extractable(a, r) {
                    let title = format!("Extract prefix {name}: for <{ns}>");
                    let data = json!({ "uri": uri, "turtle": EXTRACT, "ns": ns, "name": name });
                    let edits = (!lazy).then(|| extract_edits(a, enc, &ns, &name));
                    push(CodeActionKind::REFACTOR_REWRITE, title, edits, Some(data));
                }
            }
            Form::PName { .. } => {
                if let Some(full) = expand(a, &r.iri) {
                    push(
                        CodeActionKind::REFACTOR_REWRITE,
                        "Expand to full IRI".into(),
                        Some(text_edits(a, enc, vec![(r.span, full)])),
                        None,
                    );
                }
            }
            Form::A => {}
        }
    }

    let data = json!({ "uri": uri, "turtle": SHORTEN_ALL });
    if lazy {
        let mut any = false;
        each_shortenable(a, |_, _| {
            any = true;
            false
        });
        if any {
            push(
                CodeActionKind::REFACTOR_REWRITE,
                "Shorten all IRIs in file".into(),
                None,
                Some(data),
            );
        }
    } else {
        let edits = shorten_all_edits(a, enc);
        if !edits.is_empty() {
            push(
                CodeActionKind::REFACTOR_REWRITE,
                "Shorten all IRIs in file".into(),
                Some(edits),
                Some(data),
            );
        }
    }
    out
}

/// Fill in the edit of a file-wide action returned without one.
pub(crate) fn resolve(a: &Analysis, enc: Encoding, mut action: CodeAction) -> CodeAction {
    let Some(data) = action.data.as_ref() else {
        return action;
    };
    let Ok(uri) = serde_json::from_value::<Uri>(data["uri"].clone()) else {
        return action;
    };
    let edits = match data["turtle"].as_str() {
        Some(SHORTEN_ALL) => shorten_all_edits(a, enc),
        Some(EXTRACT) => {
            let (Some(ns), Some(name)) = (data["ns"].as_str(), data["name"].as_str()) else {
                return action;
            };
            // The document may have changed since the action was offered.
            if taken_names(a).contains(name) || bound_namespaces(a).contains(ns) {
                return action;
            }
            extract_edits(a, enc, ns, name)
        }
        _ => return action,
    };
    action.edit = Some(workspace_edit(&uri, edits));
    action
}

/// Whether an action of `kind` passes the client's `context.only` filter
/// (`refactor` admits `refactor.rewrite`).
pub(crate) fn wanted(only: Option<&[CodeActionKind]>, kind: &CodeActionKind) -> bool {
    only.is_none_or(|only| {
        only.iter().any(|o| {
            let (o, k) = (o.as_str(), kind.as_str());
            o.is_empty() || k == o || (k.starts_with(o) && k.as_bytes()[o.len()] == b'.')
        })
    })
}

pub(crate) fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), edits);
    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

/// The ref under the cursor, or the first one inside a selection.
fn ref_in(a: &Analysis, start: u32, end: u32) -> Option<&Ref> {
    a.ref_at(start).or_else(|| {
        let refs = &a.parsed.refs;
        let i = refs.partition_point(|r| r.span.start < start);
        refs.get(i).filter(|r| r.span.start < end)
    })
}

// ---- shorten / expand ----

/// Calls `f` with every `<...>` ref that a prefix in scope at its position
/// shortens, and its prefixed name, until `f` returns false.
fn each_shortenable(a: &Analysis, mut f: impl FnMut(&Ref, String) -> bool) {
    let mut scope = Scope::outside(a);
    let mut decls = a.parsed.prefixes.iter().peekable();
    for r in a.parsed.refs.iter().filter(|r| r.form == Form::Iri) {
        while let Some(p) = decls.next_if(|p| p.span.end <= r.span.start) {
            scope.insert(p.name.clone(), p.iri.to_string());
        }
        if written_absolute(a, r)
            && let Some(pname) = shorten(&r.iri, &scope)
            && !f(r, pname)
        {
            return;
        }
    }
}

/// Whether the `<...>` at `r` is written with a scheme. A relative one is
/// resolved against the document's URL (or its `@base`), and the refactorings
/// that rewrite IRIs leave it as written.
fn written_absolute(a: &Analysis, r: &Ref) -> bool {
    a.text
        .get(r.span.start as usize + 1..r.span.end as usize)
        .is_some_and(crate::syntax::has_scheme)
}

fn shorten_all_edits(a: &Analysis, enc: Encoding) -> Vec<TextEdit> {
    let mut found: Vec<(Span, String)> = Vec::new();
    each_shortenable(a, |r, pname| {
        found.push((r.span, pname));
        true
    });
    text_edits(a, enc, found)
}

/// Edits replacing whole tokens, through [`turtle_token_edits`].
pub(crate) fn text_edits(a: &Analysis, enc: Encoding, reps: Vec<(Span, String)>) -> Vec<TextEdit> {
    text_edits_in(a, &a.tokens_or_lexed(), enc, reps)
}

/// [`text_edits`] with `tokens`, those of `a`, in hand.
pub(crate) fn text_edits_in(
    a: &Analysis,
    tokens: &[Token],
    enc: Encoding,
    reps: Vec<(Span, String)>,
) -> Vec<TextEdit> {
    // Never `None`: a space keeps any two Turtle tokens apart.
    turtle_token_edits(&a.text, tokens, reps)
        .into_iter()
        .flatten()
        .map(|(span, new_text)| TextEdit {
            range: a.range(enc, span),
            new_text,
        })
        .collect()
}

/// `<iri>` for a prefixed name, escaping what an IRIREF cannot hold. Only for
/// valid absolute IRIs (the parser warns about others in `<...>`) without dot
/// segments, which a `@base` would remove.
fn expand(a: &Analysis, iri: &str) -> Option<String> {
    expand_iri(
        iri,
        a.parsed.base.is_some() || a.parsed.document_base.is_some(),
    )
}

/// `<iri>` for a prefixed name standing for `iri`; `None` for an invalid
/// absolute IRI, or one with dot segments when `resolved` (a base would
/// remove them).
pub(crate) fn expand_iri(iri: &str, resolved: bool) -> Option<String> {
    if oxiri::Iri::parse(iri).is_err()
        || (resolved && iri.split(['/', '?', '#']).any(|s| s == "." || s == ".."))
    {
        return None;
    }
    let mut out = String::with_capacity(iri.len() + 2);
    out.push('<');
    for c in iri.chars() {
        if c <= ' ' || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\') {
            out.push_str(&format!("\\u{:04X}", c as u32));
        } else {
            out.push(c);
        }
    }
    out.push('>');
    Some(out)
}

// ---- extract prefix ----

/// Namespace and new prefix name for the IRI at `r`, when no prefix covers it.
/// Not for an invalid IRI: the declaration writes the namespace raw.
fn extractable(a: &Analysis, r: &Ref) -> Option<(String, String)> {
    let iri: &str = &r.iri;
    oxiri::Iri::parse(iri).ok()?;
    let ns = declarable_namespace(iri)?;
    if iri.len() == ns.len() || !is_pn_local(&iri[ns.len()..]) {
        return None;
    }
    if bound_namespaces(a).contains(ns) {
        return None;
    }
    // The declaration goes after the last one; IRIs before it cannot use it.
    if insertion_offset(a) > r.span.start {
        return None;
    }
    let taken = taken_names(a);
    let name = VOCABS
        .by_namespace(ns)
        .map(|v| v.prefix.to_string())
        .filter(|p| !taken.contains(p.as_str()) && is_pn_prefix(p))
        .unwrap_or_else(|| unique(&derive_name(ns), &taken));
    Some((ns.to_string(), name))
}

fn extract_edits(a: &Analysis, enc: Encoding, ns: &str, name: &str) -> Vec<TextEdit> {
    let from = insertion_offset(a);
    let mut reps = Vec::new();
    for r in a.parsed.refs.iter().filter(|r| r.form == Form::Iri) {
        let iri: &str = &r.iri;
        if r.span.start >= from
            && iri.starts_with(ns)
            && is_pn_local(&iri[ns.len()..])
            && written_absolute(a, r)
        {
            reps.push((r.span, format!("{name}:{}", &iri[ns.len()..])));
        }
    }
    let mut edits = vec![declaration_edit(a, enc, name, ns, u32::MAX)];
    edits.extend(text_edits(a, enc, reps));
    edits
}

fn insertion_offset(a: &Analysis) -> u32 {
    declaration_point(a, u32::MAX).0 as u32
}

fn taken_names(a: &Analysis) -> FxHashSet<String> {
    let mut names: FxHashSet<String> = a.external.keys().cloned().collect();
    names.extend(a.parsed.prefixes.iter().map(|p| p.name.clone()));
    names.extend(a.parsed.prefix_uses.iter().map(|u| u.name.to_string()));
    names
}

fn bound_namespaces(a: &Analysis) -> FxHashSet<String> {
    let mut ns: FxHashSet<String> = a.external.values().cloned().collect();
    ns.extend(a.parsed.prefixes.iter().map(|p| p.iri.to_string()));
    ns
}

const GENERIC_SEGMENTS: &[&str] = &[
    "ns",
    "vocab",
    "vocabulary",
    "ontology",
    "ontologies",
    "onto",
    "ont",
    "schema",
    "schemas",
    "def",
    "terms",
    "resource",
    "resources",
    "id",
    "owl",
    "rdf",
];

/// A prefix name from the last meaningful path segment, else the host.
fn derive_name(ns: &str) -> String {
    let rest = ns.split_once("://").map_or(ns, |(_, r)| r);
    let (host, path) = rest.split_at(rest.find(['/', '#']).unwrap_or(rest.len()));
    let clean = |s: &str| -> String {
        let s = s.to_ascii_lowercase();
        let s: String = s.chars().filter(char::is_ascii_alphanumeric).collect();
        let s = s.trim_start_matches(|c: char| c.is_ascii_digit());
        s.chars().take(12).collect()
    };
    let is_version = |s: &str| {
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    };
    for seg in path.split(['/', '#']).rev().filter(|s| !s.is_empty()) {
        let seg = seg
            .rsplit_once('.')
            .filter(|(_, ext)| ["ttl", "owl", "rdf", "nt", "n3", "jsonld", "xml"].contains(ext))
            .map_or(seg, |(stem, _)| stem);
        let name = clean(seg);
        if !name.is_empty() && !is_version(seg) && !GENERIC_SEGMENTS.contains(&name.as_str()) {
            return name;
        }
    }
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host.split('.')
        .filter(|l| *l != "www")
        .map(clean)
        .find(|l| !l.is_empty())
        .unwrap_or_else(|| "ns".into())
}

fn unique(base: &str, taken: &FxHashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (1u32..)
        .map(|i| {
            let n = i.to_string();
            let keep = base.len().min(12usize.saturating_sub(n.len()));
            format!("{}{n}", &base[..keep])
        })
        .find(|c| !taken.contains(c))
        .unwrap_or_default()
}

// ---- organise prefixes ----

/// One directive of the leading block with the comment lines above it.
struct Unit<'a> {
    name: &'a str,
    text: String,
}

/// A prefix declaration of a block to organise.
pub(crate) struct Directive<'a> {
    pub(crate) name: &'a str,
    /// The whole declaration.
    pub(crate) span: Span,
}

/// The edit that sorts the leading prefix block and drops duplicate and unused
/// declarations, or None when that is unsafe or would change nothing.
fn organise(a: &Analysis, enc: Encoding) -> Option<TextEdit> {
    let text: &str = &a.text;
    let block = leading_block(a)?;
    let (first, last) = (block.first()?, block.last()?);

    // A prefix redefined with another IRI makes order meaningful.
    let mut iris: rustc_hash::FxHashMap<&str, &str> = Default::default();
    for p in &a.parsed.prefixes {
        if iris
            .insert(&p.name, &p.iri)
            .is_some_and(|prev| prev != p.iri)
        {
            return None;
        }
    }
    let region_end = line_content_end(text, last.span.end as usize);
    if a.parsed
        .errors
        .iter()
        .any(|e| (e.span.start as usize) < region_end && e.span.end >= first.span.start)
    {
        return None;
    }
    let block: Vec<Directive> = block
        .iter()
        .map(|d| Directive {
            name: &d.name,
            span: d.span,
        })
        .collect();
    let used: FxHashSet<&str> = a.parsed.prefix_uses.iter().map(|u| &*u.name).collect();
    let keep_unused = file_uri_name(&a.uri).as_deref() == Some(PREFIX_FILE);
    let (span, new_text) = organise_block(
        text,
        &block,
        |name| keep_unused || used.contains(name),
        false,
    )?;
    Some(TextEdit {
        range: a.range(enc, span),
        new_text,
    })
}

/// The span to replace and its new text that sort a leading block of
/// prefix declarations, drop the duplicates and those `keep` rejects, and
/// carry along the comment lines above each one and the comment after it.
/// The comment lines directly above the first one stay where they are with
/// `header_stays`, else they move with it. `None` when the block holds
/// something else or nothing would change.
pub(crate) fn organise_block(
    text: &str,
    block: &[Directive],
    keep: impl Fn(&str) -> bool,
    header_stays: bool,
) -> Option<(Span, String)> {
    let (first, last) = (block.first()?, block.last()?);
    let region_end = line_content_end(text, last.span.end as usize);
    let mut units: Vec<Unit> = Vec::new();
    let mut region_start = line_start(text, first.span.start as usize);
    let mut prev_end: Option<usize> = None;
    for (i, d) in block.iter().enumerate() {
        let (start, end) = (d.span.start as usize, d.span.end as usize);
        let ls = line_start(text, start);
        let mut comments = Vec::new();
        match prev_end {
            // Shares a line with the previous directive.
            Some(pe) if pe > ls => {
                if !text[pe..start].trim().is_empty() {
                    return None;
                }
            }
            Some(pe) => {
                if !text[ls..start].trim().is_empty() {
                    return None;
                }
                let between = &text[line_content_end(text, pe)..ls];
                comments.extend(
                    between
                        .split(['\n', '\r'])
                        .map(str::trim)
                        .filter(|l| l.starts_with('#'))
                        .map(str::to_string),
                );
            }
            None => {
                if !text[ls..start].trim().is_empty() {
                    return None;
                }
                // Comment lines directly above the first directive go with it.
                while region_start > 0 && !header_stays {
                    let above = line_start(text, region_start - 1);
                    let line = text[above..region_start].trim();
                    if !line.starts_with('#') {
                        break;
                    }
                    comments.push(line.to_string());
                    region_start = above;
                }
                comments.reverse();
            }
        }
        // The rest of the line: the next directive, a comment, or nothing.
        let next_start = block.get(i + 1).map(|n| n.span.start as usize);
        let le = line_content_end(text, end);
        let stop = next_start.filter(|n| *n < le).unwrap_or(le);
        let rest = text[end..stop].trim();
        if !(rest.is_empty() || rest.starts_with('#')) {
            return None;
        }
        comments.push(text[start..stop].trim_end().to_string());
        units.push(Unit {
            name: d.name,
            text: comments.join("\n"),
        });
        prev_end = Some(end);
    }

    let mut seen: FxHashSet<&str> = FxHashSet::default();
    let mut kept: Vec<(usize, &Unit)> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| seen.insert(u.name))
        .filter(|(_, u)| keep(u.name))
        .collect();
    kept.sort_by(|(i, x), (j, y)| {
        let (x, y): (&str, &str) = (x.name, y.name);
        x.to_lowercase()
            .cmp(&y.to_lowercase())
            .then(x.cmp(y))
            .then(i.cmp(j))
    });
    if kept.len() == units.len() && kept.iter().enumerate().all(|(n, (i, _))| n == *i) {
        return None;
    }
    let nl = newline(text);
    // A multi-line directive keeps its own breaks, in the document's style.
    let new_text = kept
        .iter()
        .map(|(_, u)| u.text.replace("\r\n", "\n").replace('\n', nl))
        .collect::<Vec<_>>()
        .join(nl);
    Some((Span::new(region_start as u32, region_end as u32), new_text))
}

/// The prefix directives before the first statement, if they form one run
/// (no `@base` or `@version` between them) and all parsed cleanly.
fn leading_block(a: &Analysis) -> Option<Vec<&PrefixDecl>> {
    let toks = a.tokens();
    let decls = &a.parsed.prefixes;
    let mut block: Vec<&PrefixDecl> = Vec::new();
    let mut barrier_after_block = false;
    let mut i = 0;
    while let Some(t) = toks.get(i) {
        match t.kind {
            Kind::Comment => i += 1,
            Kind::PrefixDirective | Kind::SparqlPrefix => {
                if barrier_after_block && !block.is_empty() {
                    return None;
                }
                let d = decls.iter().find(|d| d.span.start == t.start)?;
                block.push(d);
                while toks.get(i).is_some_and(|t| t.start < d.span.end) {
                    i += 1;
                }
            }
            Kind::BaseDirective
            | Kind::SparqlBase
            | Kind::VersionDirective
            | Kind::SparqlVersion => {
                if !block.is_empty() {
                    barrier_after_block = true;
                }
                i += 1;
                if toks
                    .get(i)
                    .is_some_and(|t| matches!(t.kind, Kind::Iri | Kind::String))
                {
                    i += 1;
                }
                if toks.get(i).is_some_and(|t| t.kind == Kind::Dot) {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    (!block.is_empty()).then_some(block)
}

// A bare `\r` ends a line too, as in the Turtle and SPARQL grammars.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind(['\n', '\r']).map_or(0, |i| i + 1)
}

/// End of the line's content: before its `\n`, `\r\n` or bare `\r`.
pub(crate) fn line_content_end(text: &str, at: usize) -> usize {
    text[at..].find(['\n', '\r']).map_or(text.len(), |i| at + i)
}

/// Text after applying `edits` (positions in `enc`), for tests.
pub fn apply(a: &Analysis, enc: Encoding, edits: &[TextEdit]) -> String {
    let mut spans: Vec<(u32, u32, &str)> = edits
        .iter()
        .map(|e| {
            (
                a.offset(enc, e.range.start).unwrap_or(0),
                a.offset(enc, e.range.end).unwrap_or(0),
                e.new_text.as_str(),
            )
        })
        .collect();
    spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.1)));
    let mut out = a.text.to_string();
    for (s, e, t) in spans {
        out.replace_range(s as usize..e as usize, t);
    }
    out
}

/// All edits of an action for `uri`, resolving it first if needed.
pub fn edits_of(a: &Analysis, enc: Encoding, action: &CodeAction) -> Vec<TextEdit> {
    let action = if action.edit.is_none() {
        resolve(a, enc, action.clone())
    } else {
        action.clone()
    };
    action
        .edit
        .and_then(|e| e.changes)
        .into_iter()
        .flat_map(|c| c.into_values().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    fn analysis(uri: &str, text: &str) -> Analysis {
        Analysis::new(uri.into(), text, Arc::new(PrefixMap::default()), true)
    }

    fn params(a: &Analysis, off: usize) -> CodeActionParams {
        let pos = a.position(Encoding::Utf16, off as u32);
        CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///t/doc.ttl".parse().unwrap(),
            },
            range: Range::new(pos, pos),
            context: Default::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    fn triples(text: &str) -> Vec<String> {
        let p = crate::syntax::parse(text, &PrefixMap::default(), Default::default());
        let mut t: Vec<String> = p
            .triples
            .iter()
            .map(|t| format!("{:?} {:?} {:?}", t.s, t.p, t.o))
            .collect();
        t.sort();
        t
    }

    /// Run `title` at `needle` and check the result and that the graph is kept.
    fn run(text: &str, needle: &str, title: &str, lazy: bool) -> Option<String> {
        let a = analysis("file:///t/doc.ttl", text);
        let off = text.find(needle).unwrap();
        let actions = code_actions(&a, Encoding::Utf16, &params(&a, off), lazy);
        let action = actions.into_iter().find_map(|c| match c {
            CodeActionOrCommand::CodeAction(c) if c.title.starts_with(title) => Some(c),
            _ => None,
        })?;
        let out = apply(&a, Encoding::Utf16, &edits_of(&a, Encoding::Utf16, &action));
        assert_eq!(triples(text), triples(&out), "graph changed:\n{out}");
        Some(out)
    }

    #[test]
    fn names_are_derived_from_the_namespace() {
        assert_eq!(
            declarable_namespace("http://xmlns.com/foaf/0.1/Person"),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(declarable_namespace("http://example.org"), None);
        assert_eq!(declarable_namespace("http://example.org/a?x=/y"), None);
        assert_eq!(derive_name("http://xmlns.com/foaf/0.1/"), "foaf");
        assert_eq!(derive_name("https://w3id.org/sd/vocab/"), "sd");
        assert_eq!(derive_name("http://example.org/"), "example");
        assert_eq!(derive_name("http://example.org/ont/core.ttl#"), "core");
        assert_eq!(
            derive_name("http://e.org/VeryLongSegmentName#"),
            "verylongsegm"
        );
        let taken: FxHashSet<String> = ["core".into(), "core1".into()].into_iter().collect();
        assert_eq!(unique("core", &taken), "core2");
        let taken: FxHashSet<String> = ["abcdefghijkl".into()].into_iter().collect();
        assert_eq!(unique("abcdefghijkl", &taken), "abcdefghijk1");
    }

    #[test]
    fn organise_keeps_the_graph_of_a_file_with_bare_cr_line_breaks() {
        // A bare `\r` ends a line, and a comment, as `\n` does.
        let text = "@prefix z: <http://z/> .\r# about e\r@prefix e: <http://e/> .\r\r# data\re:a e:p z:b .\r";
        let organised = run(text, "e:a", "Organise prefixes", false).unwrap();
        assert_eq!(
            organised,
            "# about e\r@prefix e: <http://e/> .\r@prefix z: <http://z/> .\r\r# data\re:a e:p z:b .\r"
        );
        // Every prefix unused: the declarations go, and only they (it once
        // removed the whole file, read as one line).
        let text = "@prefix z: <http://z/> .\r@prefix e: <http://e/> .\r<http://e/a> <http://e/p> <http://z/b> .\r";
        let organised = run(text, "http://e/a", "Organise prefixes", false).unwrap();
        assert_eq!(organised, "\r<http://e/a> <http://e/p> <http://z/b> .\r");
    }

    #[test]
    fn graph_is_kept_by_every_action() {
        let text = "# header\n\n@prefix z: <http://z/> .\n# about e\n@prefix e: <http://e/> .\n@prefix u: <http://u/> .\n\ne:a e:p <http://z/b>, <http://e/c.d>, <http://other.org/ns/x> ; z:q <http://other.org/ns/y>.\n_:b<http://e/p> e:x .\n";
        let organised = run(text, "e:a", "Organise prefixes", false).unwrap();
        assert!(
            organised.starts_with(
                "# header\n\n# about e\n@prefix e: <http://e/> .\n@prefix z: <http://z/> .\n\ne:a"
            ),
            "{organised}"
        );
        let short = run(text, "e:a", "Shorten all", false).unwrap();
        assert!(
            short.contains("e:p z:b, e:c.d,") && short.contains("_:b e:p e:x"),
            "{short}"
        );
        assert_eq!(run(text, "e:a", "Shorten all", true).unwrap(), short);
        let one = run(text, "http://z/b", "Shorten IRI", false).unwrap();
        assert!(one.contains("e:p z:b, <http://e/c.d>"), "{one}");
        let ext = run(text, "other.org/ns/x", "Extract prefix", true).unwrap();
        assert!(
            ext.contains("@prefix u: <http://u/> .\n@prefix other: <http://other.org/ns/> .\n"),
            "{ext}"
        );
        assert!(ext.contains("other:x ; z:q other:y."), "{ext}");
        let exp = run(text, "e:a", "Expand to full IRI", false).unwrap();
        assert!(exp.contains("\n<http://e/a> e:p"), "{exp}");
    }

    #[test]
    fn a_rewritten_iri_never_joins_its_neighbour() {
        // Reduced from a chaos input: the `.` ending a statement sat between a
        // name and the IRI that starts the next one.
        let line = "<http://s/s> a ex:C ; ex:p ex:a.b.<http://e/V> a <http://e/C> .\n";
        let text = format!("@prefix ex: <http://ex/> .\n@prefix sh: <http://e/> .\n{line}");
        let short = run(&text, "ex:C", "Shorten all", false).unwrap();
        assert!(short.contains("ex:a.b. sh:V a sh:C ."), "{short}");
        let one = run(&text, "http://e/V", "Shorten IRI", false).unwrap();
        assert!(one.contains("ex:a.b. sh:V a <http://e/C>"), "{one}");
        let text = format!("@prefix ex: <http://ex/> .\n{line}");
        let ext = run(&text, "http://e/V", "Extract prefix", false).unwrap();
        assert!(ext.contains("ex:a.b. e:V a e:C ."), "{ext}");
        let text = "<http://n.org/a><http://n.org/b><http://n.org/c>.\n";
        let ext = run(text, "n.org/a", "Extract prefix", false).unwrap();
        assert!(ext.ends_with("\nn:a n:b n:c.\n"), "{ext}");
    }

    #[test]
    fn an_extracted_prefix_is_declared_between_statements() {
        // Reduced from chaos inputs: the first `\n` after the declaration
        // sat inside a statement, then inside a string of a `\r` file.
        let text = "@prefix ex: <http://e/1.5e> .  <http://x/a>  <http://q/def/n>  \n\"Al\" ;  <http://q/def/m>  \"Ally\" . ";
        let ext = run(text, "http://q/def/n", "Extract prefix", false).unwrap();
        assert!(
            ext.starts_with(
                "@prefix ex: <http://e/1.5e> .\n@prefix q: <http://q/def/> .  <http://x/a>  q:n"
            ),
            "{ext:?}"
        );
        let text = "@prefix ex: <http://ex/> .\r@prefix sh: <http://www.w3.org/ns/shacl#> .\rex:s sh:select \"\"\"SELECT ?x\n WHERE { ?x a ex:T }\"\"\" ;\r  ex:v \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\r";
        let ext = run(text, "XMLSchema", "Extract prefix", false).unwrap();
        assert!(
            ext.contains("shacl#> .\r@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\rex:s"),
            "{ext:?}"
        );
        assert!(ext.contains("\"1\"^^xsd:integer"), "{ext:?}");
    }

    #[test]
    fn no_prefix_is_extracted_from_an_invalid_iri() {
        // Reduced from a chaos input: the `\u0020`, written raw in the
        // declaration, turned its `#> .` into a comment.
        let text = "@prefix ex: <http://example.org/> . ex:s ex:p <http://example.org/a\\u0020b#c> ; ex:q 1 .";
        assert_eq!(
            run(text, "http://example.org/a", "Extract prefix", false),
            None
        );
    }

    #[test]
    fn organise_refuses_unsafe_or_useless_edits() {
        let use_all = "a:x b:y c:z .\n";
        let redefined = format!(
            "@prefix b: <http://b/> .\n@prefix a: <http://a/> .\n@prefix c: <http://c/> .\n{use_all}@prefix a: <http://a2/> .\n"
        );
        assert!(run(&redefined, "a:x", "Organise", false).is_none());
        let based = format!(
            "@prefix b: <http://b/> .\n@base <http://x/> .\n@prefix a: <http://a/> .\n@prefix c: <http://c/> .\n{use_all}"
        );
        assert!(run(&based, "a:x", "Organise", false).is_none());
        let sorted = format!(
            "@prefix a: <http://a/> .\n\n@prefix b: <http://b/> .\nPREFIX c: <http://c/>\n{use_all}"
        );
        assert!(run(&sorted, "a:x", "Organise", false).is_none());
        let dup = format!(
            "PREFIX c: <http://c/>\n@prefix a: <http://a/> . @prefix b: <http://b/> . # bee\n@prefix a: <http://a/> .\n{use_all}"
        );
        assert_eq!(
            run(&dup, "a:x", "Organise", false).unwrap(),
            format!(
                "@prefix a: <http://a/> .\n@prefix b: <http://b/> . # bee\nPREFIX c: <http://c/>\n{use_all}"
            )
        );
    }

    #[test]
    fn organising_a_crlf_file_keeps_every_line_break_a_crlf() {
        // The second directive spans two lines.
        let text = "@prefix b: <http://b/> .\r\n@prefix a:\r\n  <http://a/> .\r\nb:x a:y a:z .\r\n";
        let a = analysis("file:///t/doc.ttl", text);
        let edit = organise(&a, Encoding::Utf16).unwrap();
        assert_eq!(
            edit.new_text,
            "@prefix a:\r\n  <http://a/> .\r\n@prefix b: <http://b/> ."
        );
        assert!(!edit.new_text.contains("\r\r"));
    }

    #[test]
    fn only_a_file_named_prefixes_ttl_keeps_unused_prefixes() {
        let text = "@prefix b: <http://b/> .\n@prefix a: <http://a/> .\n";
        for uri in ["file:///t/myprefixes.ttl", "file:///t/prefixes.ttl.bak.ttl"] {
            let a = analysis(uri, text);
            let edit = organise(&a, Encoding::Utf16).unwrap();
            assert_eq!(edit.new_text, "", "{uri}");
        }
        let a = analysis("file:///t/sub%20dir/prefixes.ttl", text);
        assert_eq!(
            organise(&a, Encoding::Utf16).unwrap().new_text,
            "@prefix a: <http://a/> .\n@prefix b: <http://b/> ."
        );
    }

    #[test]
    fn prefix_file_keeps_unused_prefixes() {
        let text = "@prefix b: <http://b/> .\n@prefix a: <http://a/> .\n";
        let a = analysis("file:///t/prefixes.ttl", text);
        let edit = organise(&a, Encoding::Utf16).unwrap();
        assert_eq!(
            edit.new_text,
            "@prefix a: <http://a/> .\n@prefix b: <http://b/> ."
        );
    }
}
