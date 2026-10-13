//! Refactorings: organise the prefix block, shorten and expand IRIs, and
//! extract a prefix for a namespace. Every edit keeps the graph the same.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::prefix_insertion;
use crate::syntax::lexer::{Kind, is_pn_chars, is_pn_chars_base, is_pn_chars_u};
use crate::syntax::{Form, PrefixDecl, PrefixMap, Ref, Span};
use crate::vocab::VOCABS;
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
            Form::Iri => {
                let scope = scope_at(a, r.span.start);
                if let Some(pname) = shorten(&r.iri, &scope) {
                    let new_text = spaced(&a.text, r.span, pname);
                    push(
                        CodeActionKind::REFACTOR_REWRITE,
                        format!("Shorten IRI to {}", new_text.trim()),
                        Some(vec![TextEdit {
                            range: a.range(enc, r.span),
                            new_text,
                        }]),
                        None,
                    );
                } else if let Some((ns, name)) = extractable(a, enc, r) {
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
                        Some(vec![TextEdit {
                            range: a.range(enc, r.span),
                            new_text: full,
                        }]),
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
pub fn resolve(a: &Analysis, enc: Encoding, mut action: CodeAction) -> CodeAction {
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
pub fn wanted(only: Option<&[CodeActionKind]>, kind: &CodeActionKind) -> bool {
    only.is_none_or(|only| {
        only.iter().any(|o| {
            let (o, k) = (o.as_str(), kind.as_str());
            o.is_empty() || k == o || (k.starts_with(o) && k.as_bytes()[o.len()] == b'.')
        })
    })
}

fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
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

/// Prefixes in scope at `offset`: external ones, then the declarations before it.
fn scope_at(a: &Analysis, offset: u32) -> PrefixMap {
    let mut m = (*a.external).clone();
    for p in a
        .parsed
        .prefixes
        .iter()
        .take_while(|p| p.span.end <= offset)
    {
        m.insert(p.name.clone(), p.iri.to_string());
    }
    m
}

/// `prefix:local` for `iri` using the longest namespace in `scope` whose local
/// part is a PN_LOCAL as is (no backslash escapes needed).
fn shorten(iri: &str, scope: &PrefixMap) -> Option<String> {
    scope
        .iter()
        .filter(|(name, ns)| {
            let (name, ns): (&str, &str) = (name, ns);
            !ns.is_empty()
                && iri.starts_with(ns)
                && is_pn_prefix(name)
                && is_plain_local(&iri[ns.len()..])
        })
        .min_by(|(n1, ns1), (n2, ns2)| {
            ns2.len()
                .cmp(&ns1.len())
                .then(n1.len().cmp(&n2.len()))
                .then(n1.cmp(n2))
        })
        .map(|(name, ns)| format!("{name}:{}", &iri[ns.len()..]))
}

/// Calls `f` with every `<...>` ref that a prefix in scope at its position
/// shortens, and its prefixed name, until `f` returns false.
fn each_shortenable(a: &Analysis, mut f: impl FnMut(&Ref, String) -> bool) {
    let mut scope = (*a.external).clone();
    let mut decls = a.parsed.prefixes.iter().peekable();
    for r in a.parsed.refs.iter().filter(|r| r.form == Form::Iri) {
        while let Some(p) = decls.next_if(|p| p.span.end <= r.span.start) {
            scope.insert(p.name.clone(), p.iri.to_string());
        }
        if let Some(pname) = shorten(&r.iri, &scope)
            && !f(r, pname)
        {
            return;
        }
    }
}

fn shorten_all_edits(a: &Analysis, enc: Encoding) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    each_shortenable(a, |r, pname| {
        edits.push(TextEdit {
            range: a.range(enc, r.span),
            new_text: spaced(&a.text, r.span, pname),
        });
        true
    });
    edits
}

/// A prefixed name replacing the IRI at `span`, with a space added where it
/// would otherwise run into the neighbouring token (`a<x>`, `<x>ex:y`).
fn spaced(text: &str, span: Span, pname: String) -> String {
    let joins = |c: char| is_pn_chars(c) || matches!(c, ':' | '%' | '\\');
    let before = text[..span.start as usize].chars().next_back();
    let mut after = text[span.end as usize..].chars();
    let next = after.next();
    let space_before = before.is_some_and(joins);
    let space_after = match next {
        Some('.') => after.next().is_some_and(joins),
        Some(c) => joins(c),
        None => false,
    };
    match (space_before, space_after) {
        (false, false) => pname,
        (b, s) => format!(
            "{}{pname}{}",
            if b { " " } else { "" },
            if s { " " } else { "" }
        ),
    }
}

/// `<iri>` for a prefixed name, escaping what an IRIREF cannot hold. Only for
/// valid absolute IRIs (the parser warns about others in `<...>`) without dot
/// segments, which a `@base` would remove.
fn expand(a: &Analysis, iri: &str) -> Option<String> {
    if oxiri::Iri::parse(iri).is_err()
        || (a.parsed.base.is_some() && iri.split(['/', '?', '#']).any(|s| s == "." || s == ".."))
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

/// PN_PREFIX (the empty prefix included).
fn is_pn_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => true,
        Some(c) => {
            is_pn_chars_base(c) && chars.all(|c| is_pn_chars(c) || c == '.') && !name.ends_with('.')
        }
    }
}

/// A PN_LOCAL that needs no backslash escapes; `%` only as a `%XX` escape.
fn is_plain_local(local: &str) -> bool {
    let b = local.as_bytes();
    let mut first = true;
    for (i, c) in local.char_indices() {
        let ok = match c {
            '%' => b
                .get(i + 1..i + 3)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)),
            ':' => true,
            '.' => !first,
            c if first => is_pn_chars_u(c) || c.is_ascii_digit(),
            c => is_pn_chars(c),
        };
        if !ok {
            return false;
        }
        first = false;
    }
    !local.ends_with('.')
}

// ---- extract prefix ----

/// Namespace and new prefix name for the IRI at `r`, when no prefix covers it.
fn extractable(a: &Analysis, enc: Encoding, r: &Ref) -> Option<(String, String)> {
    let iri: &str = &r.iri;
    let ns = namespace_of(iri)?;
    if iri.len() == ns.len() || !is_plain_local(&iri[ns.len()..]) {
        return None;
    }
    if bound_namespaces(a).contains(ns) {
        return None;
    }
    // The declaration goes after the last one; IRIs before it cannot use it.
    if insertion_offset(a, enc) > r.span.start {
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
    let text: &str = &a.text;
    let insert = prefix_insertion(a, text, enc, name, ns);
    let from = insertion_offset(a, enc);
    let mut edits = vec![insert];
    for r in a.parsed.refs.iter().filter(|r| r.form == Form::Iri) {
        let iri: &str = &r.iri;
        if r.span.start >= from && iri.starts_with(ns) && is_plain_local(&iri[ns.len()..]) {
            edits.push(TextEdit {
                range: a.range(enc, r.span),
                new_text: spaced(text, r.span, format!("{name}:{}", &iri[ns.len()..])),
            });
        }
    }
    edits
}

fn insertion_offset(a: &Analysis, enc: Encoding) -> u32 {
    let at = prefix_insertion(a, &a.text, enc, "", "").range.start;
    a.offset(enc, at).unwrap_or(0)
}

/// `iri` up to its last `#` or `/`, when that is past the authority.
fn namespace_of(iri: &str) -> Option<&str> {
    let scheme_end = iri.find(':')?;
    let rest = &iri[scheme_end + 1..];
    let path_start = match rest.strip_prefix("//") {
        Some(auth) => scheme_end + 3 + auth.find(['/', '#', '?'])?,
        None => scheme_end + 1,
    };
    let cut = iri.rfind(['#', '/'])?;
    (cut >= path_start && !iri[..cut].contains('?')).then(|| &iri[..=cut])
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
    decl: &'a PrefixDecl,
    text: String,
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
                        .lines()
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
                while region_start > 0 {
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
            decl: d,
            text: comments.join("\n"),
        });
        prev_end = Some(end);
    }

    let used: FxHashSet<&str> = a.parsed.prefix_uses.iter().map(|u| &*u.name).collect();
    let keep_unused = a.uri.ends_with(crate::workspace::PREFIX_FILE);
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    let mut kept: Vec<(usize, &Unit)> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| seen.insert(&u.decl.name))
        .filter(|(_, u)| keep_unused || used.contains(&*u.decl.name))
        .collect();
    kept.sort_by(|(i, x), (j, y)| {
        let (x, y): (&str, &str) = (&x.decl.name, &y.decl.name);
        x.to_lowercase()
            .cmp(&y.to_lowercase())
            .then(x.cmp(y))
            .then(i.cmp(j))
    });
    if kept.len() == units.len() && kept.iter().enumerate().all(|(n, (i, _))| n == *i) {
        return None;
    }
    let nl = if text[region_start..region_end].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let new_text = kept
        .iter()
        .map(|(_, u)| u.text.replace('\n', nl))
        .collect::<Vec<_>>()
        .join(nl);
    Some(TextEdit {
        range: a.range(enc, Span::new(region_start as u32, region_end as u32)),
        new_text,
    })
}

/// The prefix directives before the first statement, if they form one run
/// (no `@base` or `@version` between them) and all parsed cleanly.
fn leading_block(a: &Analysis) -> Option<Vec<&PrefixDecl>> {
    let toks = &a.parsed.tokens;
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

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// End of the line's content (before `\r\n` or `\n`).
fn line_content_end(text: &str, at: usize) -> usize {
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    if end > at && text.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
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
        let p = crate::syntax::parse(text, &PrefixMap::default());
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
    fn plain_local_names() {
        for ok in ["", "a", "a.b", "1a", "_x", "a:b", "a%20b", "a-b", "é"] {
            assert!(is_plain_local(ok), "{ok}");
        }
        for bad in ["a.", ".a", "-a", "a/b", "a#b", "a%2", "a?b", "a b", "a~"] {
            assert!(!is_plain_local(bad), "{bad}");
        }
    }

    #[test]
    fn spacing_keeps_tokens_apart() {
        let t = "_:b<http://e/p>.<http://e/x>";
        let s = Span::new(3, 15);
        assert_eq!(spaced(t, s, "e:p".into()), " e:p");
        let t = "<http://e/p>.ex:a";
        assert_eq!(spaced(t, Span::new(0, 12), "e:p".into()), "e:p ");
    }

    #[test]
    fn names_are_derived_from_the_namespace() {
        assert_eq!(
            namespace_of("http://xmlns.com/foaf/0.1/Person"),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(namespace_of("http://example.org"), None);
        assert_eq!(namespace_of("http://example.org/a?x=/y"), None);
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
