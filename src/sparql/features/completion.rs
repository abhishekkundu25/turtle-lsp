//! Completion in SPARQL documents.
//!
//! What is offered depends on where the cursor is, from the structure:
//!
//! - at the top level: query forms, update operations, `PREFIX` and `BASE`
//!   before any operation (or after the `;` that ends one); `WHERE`, `FROM`
//!   after a projection; solution modifiers after the `WHERE` group; the
//!   words that continue an update operation;
//! - in a group: graph pattern keywords where a triple pattern may start,
//!   then prefixes and terms (the Turtle candidates and ranking: classes
//!   after `a`, properties in predicate position, ranked by the subject's
//!   `a` classes written in the group, by `rdfs:domain` and SHACL);
//! - in an expression: built-in functions (and aggregates in a projection,
//!   `HAVING` or `ORDER BY`);
//! - after `?` or `$`: the document's variables, with the typed sigil.
//!
//! Nothing is offered inside a string, an IRI or a comment. Accepting a term
//! whose prefix is not declared adds its `PREFIX` line.

use super::actions::Tails;
use super::edits::add_prefix;
use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::completion::{
    self as rdf, Candidate, Slot, fuzzy, may_match, ranking, ranking::Subject,
};
use crate::knowledge::Knowledge;
use crate::sparql::lexer::{self, AGGREGATES};
use crate::sparql::{
    GroupId, GroupKind, OperationKind, Structure, TermRef, Token, TokenKind, TriplePattern,
};
use crate::syntax::lexer::is_pn_chars;
use crate::syntax::{Iri, PrefixMap, RDF_TYPE, Span};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};

const QUERY_FORMS: &[&str] = &["SELECT", "CONSTRUCT", "ASK", "DESCRIBE"];
const UPDATE_FORMS: &[&str] = &[
    "INSERT DATA",
    "DELETE DATA",
    "DELETE WHERE",
    "INSERT",
    "DELETE",
    "WITH",
    "LOAD",
    "CLEAR",
    "DROP",
    "CREATE",
    "ADD",
    "MOVE",
    "COPY",
];
const PROLOGUE: &[&str] = &["PREFIX", "BASE"];
const PATTERN: &[&str] = &[
    "OPTIONAL", "FILTER", "BIND", "VALUES", "MINUS", "GRAPH", "SERVICE", "UNION", "LATERAL",
];
const MODIFIERS: &[&str] = &[
    "GROUP BY", "HAVING", "ORDER BY", "LIMIT", "OFFSET", "VALUES",
];
const EXPRESSION: &[&str] = &["NOT EXISTS", "EXISTS", "IN", "NOT IN", "true", "false"];

/// Where the cursor is.
enum Context {
    /// Outside any group and expression, with the keywords that fit there.
    Top(Vec<&'static str>),
    /// In a group: a term in `slot`, with the keywords that fit there.
    Group(Slot, Vec<&'static str>),
    /// In an expression; `aggregates` in a projection, `HAVING` or `ORDER BY`.
    Expression { aggregates: bool },
}

pub(crate) fn completion(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    params: &CompletionParams,
) -> Option<CompletionResponse> {
    completion_at(q, k, enc, params.text_document_position.position)
}

pub(crate) fn completion_at(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
) -> Option<CompletionResponse> {
    let text = q.text();
    let offset = q.offset(enc, pos)? as usize;
    let empty = || Some(CompletionResponse::Array(vec![]));
    let toks = q.doc.tokens();
    if in_literal(toks, text, offset) {
        return empty();
    }
    let mut start = word_start(text, offset);
    let end = word_end(text, offset);
    let sigil = start > 0
        && matches!(text.as_bytes()[start - 1], b'?' | b'$')
        && !text[..start - 1]
            .chars()
            .next_back()
            .is_some_and(|c| is_pn_chars(c) || matches!(c, ':' | ')' | '>'));
    if sigil {
        start -= 1;
    }
    let typed = &text[start..offset];
    let replace = q.range(enc, Span::new(start as u32, end as u32));
    let prefixes = q.prefix_map();
    let st = q.doc.structure();

    let mut cands: Vec<Candidate> = Vec::new();
    let mut incomplete = true;
    let mut ranked = ranking::Ranking::default();
    let mut slot = Slot::Object;

    if sigil {
        variables(&mut cands, st, typed, Span::new(start as u32, end as u32));
    } else if let Some((pfx, local)) = typed.split_once(':') {
        let ctx = context(q, start as u32);
        if let Context::Group(s, _) = ctx {
            slot = s;
        }
        let Some(c) = rdf::prefixed_terms(k, pfx, local, slot, &prefixes) else {
            return empty();
        };
        cands = c;
        incomplete = false;
        ranked = rank(q, k, start as u32, slot, typed, &prefixes);
    } else {
        match context(q, start as u32) {
            Context::Top(words) => keywords(&mut cands, &words, typed),
            Context::Expression { aggregates } => {
                functions(&mut cands, typed, aggregates);
                keywords(&mut cands, EXPRESSION, typed);
                if aggregates {
                    keywords(&mut cands, &["AS"], typed);
                }
                if !typed.is_empty() {
                    rdf::prefix_candidates(&mut cands, typed, &prefixes, "PREFIX", false);
                }
            }
            Context::Group(s, words) => {
                slot = s;
                keywords(&mut cands, &words, typed);
                rdf::prefix_candidates(&mut cands, typed, &prefixes, "PREFIX", true);
                rdf::described_candidates(&mut cands, typed, slot, k, &prefixes);
                rdf::local_name_candidates(&mut cands, typed, slot, &prefixes);
                ranked = rank(q, k, start as u32, slot, typed, &prefixes);
            }
        }
    }

    incomplete |= rdf::finish(&mut cands, ranked, slot, typed, k, &prefixes);
    let nl = crate::syntax::names::newline(q.text());
    let tails = Tails::default();
    let items = rdf::items(cands, replace, &prefixes, "PREFIX", |p, ns| {
        let mut e = add_prefix(q, enc, p, ns, &tails)?;
        e.new_text = e.new_text.replace('\n', nl);
        Some(e)
    });
    Some(CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    }))
}

/// Term items are resolved by the Turtle resolver (they carry `data.iri`).
pub(crate) fn resolve_completion(item: CompletionItem, k: &Knowledge) -> CompletionItem {
    rdf::resolve(item, k)
}

// ---- candidates ----

fn keywords(cands: &mut Vec<Candidate>, words: &[&'static str], typed: &str) {
    for w in words {
        if starts_with_ignore_case(w, typed) && !cands.iter().any(|c| c.label == *w) {
            cands.push(Candidate::plain(
                w.to_string(),
                CompletionItemKind::KEYWORD,
                "keyword".into(),
                1,
            ));
        }
    }
}

fn functions(cands: &mut Vec<Candidate>, typed: &str, aggregates: bool) {
    for (w, kind) in lexer::words() {
        if kind != TokenKind::Function || !starts_with_ignore_case(w, typed) {
            continue;
        }
        let aggregate = AGGREGATES.contains(&w);
        if aggregate && !aggregates {
            continue;
        }
        cands.push(Candidate::plain(
            w.to_string(),
            CompletionItemKind::FUNCTION,
            if aggregate { "aggregate" } else { "function" }.into(),
            0,
        ));
    }
}

/// The variables of the document matching `typed` (sigil included), except
/// the occurrence being typed at `at`.
fn variables(cands: &mut Vec<Candidate>, st: &Structure, typed: &str, at: Span) {
    let (sigil, name) = typed.split_at(1);
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    for v in &st.vars {
        if v.span == at || !fuzzy(name, &v.name) || !seen.insert(&v.name) {
            continue;
        }
        cands.push(Candidate::plain(
            format!("{sigil}{}", v.name),
            CompletionItemKind::VARIABLE,
            "variable".into(),
            0,
        ));
    }
}

fn starts_with_ignore_case(word: &str, typed: &str) -> bool {
    word.len() >= typed.len()
        && word.is_char_boundary(typed.len())
        && word[..typed.len()].eq_ignore_ascii_case(typed)
}

// ---- where the cursor is ----

/// Inside a string, an IRI (closed or still being typed) or a comment.
fn in_literal(toks: &[Token], text: &str, offset: usize) -> bool {
    let o = offset as u32;
    let i = toks.partition_point(|t| t.span.end < o);
    if let Some(t) = toks.get(i).filter(|t| t.span.start < o) {
        let open_ended = match t.kind {
            TokenKind::Comment => true,
            // An unterminated string.
            TokenKind::Error => text[t.span.start as usize..].starts_with(['"', '\'']),
            _ => false,
        };
        let inside = o < t.span.end || open_ended;
        if inside && matches!(t.kind, TokenKind::String | TokenKind::Iri) || open_ended {
            return true;
        }
    }
    // `<http://...` not closed yet: no whitespace since the `<`.
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..offset];
    line.rfind('<').is_some_and(|lt| {
        let after = &line[lt + 1..];
        !after.is_empty()
            && !after.contains(|c: char| c.is_whitespace() || c == '>')
            && after.contains([':', '/', '#'])
    })
}

fn is_word_char(c: char) -> bool {
    is_pn_chars(c) || matches!(c, ':' | '.' | '%' | '\\')
}

fn word_start(text: &str, offset: usize) -> usize {
    let mut s = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            s = i;
        } else {
            break;
        }
    }
    // A name never starts with '.' (that ends a triple).
    while text[s..offset].starts_with('.') {
        s += 1;
    }
    s
}

fn word_end(text: &str, offset: usize) -> usize {
    let mut e = offset;
    for (i, c) in text[offset..].char_indices() {
        if is_pn_chars(c) || c == ':' {
            e = offset + i + c.len_utf8();
        } else {
            break;
        }
    }
    e
}

fn is_punct(q: &SparqlAnalysis, t: &Token, p: &str) -> bool {
    t.kind == TokenKind::Punct && t.text(q.text()) == p
}

fn keyword_of(q: &SparqlAnalysis, t: &Token) -> Option<&'static str> {
    (t.kind == TokenKind::Keyword)
        .then(|| lexer::keyword(t.text(q.text())))
        .flatten()
}

/// The innermost group whose inside (between its braces) holds `at`.
fn open_group(st: &Structure, at: u32) -> Option<GroupId> {
    let end = st.groups.partition_point(|g| g.open.start < at);
    (0..end)
        .rev()
        .find(|&i| {
            let g = &st.groups[i];
            g.open.end <= at && g.close.is_none_or(|c| at <= c.start)
        })
        .map(|i| i as GroupId)
}

fn context(q: &SparqlAnalysis, at: u32) -> Context {
    let st = q.doc.structure();
    let toks = q.doc.tokens();
    let before = &toks[..toks.partition_point(|t| t.span.end <= at)];
    let code: Vec<&Token> = before
        .iter()
        .rev()
        .filter(|t| t.kind != TokenKind::Comment)
        .take(1)
        .collect();
    let prev = code.first().copied();
    let group = open_group(st, at);
    let op = st
        .operations
        .iter()
        .rev()
        .find(|o| o.keyword.start < at && prev.is_none_or(|p| !is_punct(q, p, ";")));
    let region = match (group, op) {
        (Some(g), _) => st.groups[g as usize].open.end,
        (None, Some(o)) => o.keyword.start,
        (None, None) => at,
    };

    // Which parentheses between the region's start and the cursor are open,
    // and whether each holds an expression; and whether the last closed one
    // did.
    let mut stack: Vec<bool> = Vec::new();
    let mut closed_expr = false;
    let from = before.partition_point(|t| t.span.start < region);
    let mut last: Option<&Token> = None;
    for t in before[from..]
        .iter()
        .filter(|t| t.kind != TokenKind::Comment)
    {
        if is_punct(q, t, "(") {
            let expr = stack.last().copied().unwrap_or(false)
                || last.is_some_and(|l| match l.kind {
                    TokenKind::Function => true,
                    TokenKind::Keyword => {
                        matches!(
                            keyword_of(q, l),
                            Some("FILTER" | "BIND" | "HAVING" | "BY" | "IN" | "SELECT")
                        )
                    }
                    TokenKind::Var | TokenKind::Punct => group.is_none(),
                    TokenKind::PName | TokenKind::Iri => {
                        group.is_none() || is_function_ref(st, l.span)
                    }
                    _ => false,
                })
                || (group.is_none() && last.is_none_or(|l| keyword_of(q, l) != Some("VALUES")));
            stack.push(expr);
        } else if is_punct(q, t, ")") {
            closed_expr = stack.pop().unwrap_or(false);
        }
        last = Some(t);
    }
    let in_group_kind = group.map(|g| st.groups[g as usize].kind);
    let aggregates = matches!(in_group_kind, None | Some(GroupKind::Subquery));
    if stack.last().copied().unwrap_or(false)
        || prev.is_some_and(|p| matches!(keyword_of(q, p), Some("FILTER" | "HAVING" | "BY")))
    {
        return Context::Expression { aggregates };
    }

    let Some(g) = group else {
        return Context::Top(top_keywords(q, st, op, prev, at));
    };
    let slot = slot_at(q, st, g, prev, closed_expr, at);
    let words: Vec<&'static str> = match st.groups[g as usize].kind {
        GroupKind::Values => vec!["UNDEF"],
        GroupKind::Template | GroupKind::Data => match slot {
            Slot::Subject => vec!["GRAPH"],
            Slot::Predicate => vec!["a"],
            _ => vec![],
        },
        _ => match slot {
            Slot::Subject => {
                let mut w = PATTERN.to_vec();
                if prev.is_some_and(|p| is_punct(q, p, "{")) {
                    w.push("SELECT");
                }
                w
            }
            Slot::Predicate => vec!["a"],
            _ => vec![],
        },
    };
    Context::Group(slot, words)
}

fn is_function_ref(st: &Structure, span: Span) -> bool {
    st.refs
        .iter()
        .any(|r| r.span == span && r.role == crate::sparql::TermRole::Function)
}

fn top_keywords(
    q: &SparqlAnalysis,
    st: &Structure,
    op: Option<&crate::sparql::Operation>,
    prev: Option<&Token>,
    at: u32,
) -> Vec<&'static str> {
    let Some(op) = op else {
        let update_only = prev.is_some_and(|p| is_punct(q, p, ";"));
        let mut w = Vec::new();
        if !update_only {
            w.extend_from_slice(QUERY_FORMS);
        }
        w.extend_from_slice(UPDATE_FORMS);
        w.extend_from_slice(PROLOGUE);
        return w;
    };
    let groups_after: Vec<_> = st
        .groups
        .iter()
        .filter(|g| g.parent.is_none() && g.open.start > op.keyword.start && g.open.start < at)
        .collect();
    let where_done = groups_after
        .iter()
        .any(|g| g.kind == GroupKind::Where && g.close.is_some_and(|c| c.end <= at));
    let prev_kw = prev.and_then(|p| keyword_of(q, p));
    match op.kind {
        OperationKind::Select
        | OperationKind::Construct
        | OperationKind::Describe
        | OperationKind::Ask => {
            if where_done {
                MODIFIERS.to_vec()
            } else if groups_after.iter().all(|g| g.close.is_some()) {
                let mut w = vec!["WHERE", "FROM", "NAMED"];
                if prev_kw == Some("SELECT") {
                    w.extend(["DISTINCT", "REDUCED"]);
                }
                w
            } else {
                vec![]
            }
        }
        _ if where_done => vec![],
        OperationKind::Load => vec!["SILENT", "INTO", "GRAPH"],
        OperationKind::Clear | OperationKind::Drop => {
            vec!["SILENT", "GRAPH", "DEFAULT", "NAMED", "ALL"]
        }
        OperationKind::Create => vec!["SILENT", "GRAPH"],
        OperationKind::Add | OperationKind::Move | OperationKind::Copy => {
            vec!["SILENT", "TO", "DEFAULT", "GRAPH"]
        }
        _ => match prev_kw {
            Some("INSERT") => vec!["DATA"],
            Some("DELETE") => vec!["DATA", "WHERE"],
            _ if groups_after.iter().all(|g| g.close.is_some()) => {
                vec!["INSERT", "DELETE", "USING", "NAMED", "WHERE"]
            }
            _ => vec![],
        },
    }
}

/// The triples of group `g`.
fn group_triples(st: &Structure, g: GroupId) -> impl Iterator<Item = &TriplePattern> {
    st.triples.iter().filter(move |t| t.group == Some(g))
}

fn ref_starting(st: &Structure, at: u32) -> Option<&TermRef> {
    st.refs.iter().find(|r| r.span.start == at)
}

fn is_type_predicate(st: &Structure, span: Span) -> bool {
    ref_starting(st, span.start)
        .is_some_and(|r| r.span == span && r.iri.as_deref() == Some(RDF_TYPE))
}

/// What kind of term belongs after `prev` in group `g`.
fn slot_at(
    q: &SparqlAnalysis,
    st: &Structure,
    g: GroupId,
    prev: Option<&Token>,
    closed_expr: bool,
    at: u32,
) -> Slot {
    let Some(prev) = prev else {
        return Slot::Subject;
    };
    let p = prev.text(q.text());
    match prev.kind {
        TokenKind::A => return Slot::Class,
        TokenKind::Punct => match p {
            "{" | "." | "}" => return Slot::Subject,
            ";" | "[" | "{|" => return Slot::Predicate,
            "^^" => return Slot::Datatype,
            "," => {
                let pred = group_triples(st, g)
                    .filter_map(|t| t.predicate)
                    .filter(|s| s.end <= prev.span.start)
                    .max_by_key(|s| s.start);
                return match pred {
                    Some(s) if is_type_predicate(st, s) => Slot::Class,
                    _ => Slot::Object,
                };
            }
            ")" if closed_expr => return Slot::Subject,
            _ => {}
        },
        TokenKind::Keyword => return Slot::Subject,
        _ => {}
    }
    // A term (or the end of `[ ]`, `( )`): where it is in its triple. The
    // word being typed at `at` is not written yet.
    let span = prev.span;
    let unwritten = |s: Option<Span>| s.is_none_or(|s| s.start >= at);
    for t in group_triples(st, g) {
        if t.subject.end == span.end && unwritten(t.predicate) {
            return Slot::Predicate;
        }
        if t.predicate.is_some_and(|s| s.end == span.end) && unwritten(t.object) {
            return if is_type_predicate(st, t.predicate.unwrap()) {
                Slot::Class
            } else {
                Slot::Object
            };
        }
    }
    if group_triples(st, g).any(|t| t.object.is_some_and(|o| o.end == span.end)) {
        return Slot::Subject;
    }
    match prev.kind {
        TokenKind::Var | TokenKind::PName | TokenKind::Iri | TokenKind::BlankNode => {
            Slot::Predicate
        }
        _ => Slot::Subject,
    }
}

// ---- ranking ----

/// The ontology ranking for a term in `slot` at `at`: by the subject's
/// classes in predicate position, by the predicate (and subject) in object
/// position.
fn rank(
    q: &SparqlAnalysis,
    k: &Knowledge,
    at: u32,
    slot: Slot,
    typed: &str,
    prefixes: &PrefixMap,
) -> ranking::Ranking {
    let st = q.doc.structure();
    let Some(g) = open_group(st, at) else {
        return ranking::Ranking::default();
    };
    let refs: FxHashMap<u32, &TermRef> = st.refs.iter().map(|r| (r.span.start, r)).collect();
    let iri_at = |s: Span| {
        refs.get(&s.start)
            .filter(|r| r.span == s)
            .and_then(|r| r.iri.as_deref())
    };
    // The triple the cursor is in: the last one of the group starting before
    // it, outside `[ ]` (whose triples start with the brackets).
    let text = q.text();
    let triple = group_triples(st, g)
        .filter(|t| t.subject.start < at && !t.quoted)
        .filter(|t| !text[t.subject.start as usize..].starts_with('['))
        .max_by_key(|t| t.subject.start);
    let Some(triple) = triple else {
        return ranking::Ranking::default();
    };
    let subject_text = &text[triple.subject.start as usize..triple.subject.end as usize];
    let subject_iri = iri_at(triple.subject).and_then(Iri::get);
    let mut types: Vec<Iri> = Vec::new();
    if let Some(i) = subject_iri {
        types.extend(k.types(&i));
    }
    // `?s a ex:C` (or `rdf:type`) written in this group or around it.
    let same_subject = |t: &TriplePattern| {
        let s = &text[t.subject.start as usize..t.subject.end as usize];
        match (
            subject_text.strip_prefix(['?', '$']),
            s.strip_prefix(['?', '$']),
        ) {
            (Some(a), Some(b)) => a == b,
            (None, None) => {
                iri_at(t.subject).is_some() && iri_at(t.subject) == iri_at(triple.subject)
            }
            _ => false,
        }
    };
    let mut chain = Some(g);
    while let Some(c) = chain {
        for t in group_triples(st, c) {
            if let (Some(p), Some(o)) = (t.predicate, t.object)
                && iri_at(p) == Some(RDF_TYPE)
                && same_subject(t)
                && let Some(class) = iri_at(o).and_then(Iri::get)
                && !types.contains(&class)
            {
                types.push(class);
            }
        }
        let group = &st.groups[c as usize];
        chain = group
            .parent
            .filter(|&p| st.groups[p as usize].scope == group.scope);
    }
    let subjects = [Subject {
        types,
        path: Vec::new(),
    }];
    match slot {
        Slot::Predicate => ranking::predicates_for(k, &subjects, prefixes),
        Slot::Object => {
            let Some(pred) = triple
                .predicate
                .filter(|p| p.end <= at)
                .and_then(iri_at)
                .and_then(Iri::get)
            else {
                return ranking::Ranking::default();
            };
            ranking::objects_for(k, pred, &subjects, subject_iri, prefixes, &|iri| {
                may_match(&iri, typed, prefixes)
            })
        }
        _ => ranking::Ranking::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;
    use crate::knowledge::Source;
    use std::sync::Arc;

    const ONTOLOGY: &str = "@prefix ex: <http://e/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        ex:Person a owl:Class .\n\
        ex:Org a owl:Class .\n\
        ex:worksFor a owl:ObjectProperty ; rdfs:domain ex:Person ; rdfs:range ex:Org .\n\
        ex:name a owl:DatatypeProperty .\n\
        ex:acme a ex:Org .\n\
        ex:zed a ex:Org .\n";

    /// The labels offered at `|` in `text`.
    fn offered(text: &str) -> Vec<String> {
        items(text).into_iter().map(|i| i.label).collect()
    }

    /// The items offered at `|` in `text`.
    fn items(text: &str) -> Vec<lsp_types::CompletionItem> {
        let at = text.find('|').unwrap();
        let text = text.replace('|', "");
        let q = SparqlAnalysis::new("file:///q.rq".into(), &text, Arc::new(PrefixMap::default()));
        let mut k = Knowledge::default();
        let a = Arc::new(Analysis::new(
            "file:///o.ttl".into(),
            ONTOLOGY,
            Arc::new(PrefixMap::default()),
            true,
        ));
        k.set("file:///o.ttl", Source::Open, a);
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: "file:///q.rq".parse().unwrap(),
                },
                position: q.position(Encoding::Utf16, at as u32),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };
        match completion(&q, &k, Encoding::Utf16, &params) {
            Some(CompletionResponse::List(l)) => l.items,
            Some(CompletionResponse::Array(a)) => a,
            None => vec![],
        }
    }

    #[test]
    fn no_prefix_is_added_where_its_iri_would_read_as_less_than() {
        let import = |text: &str| {
            let items = items(text);
            let foaf = items.iter().find(|i| i.label.starts_with("foaf:")).unwrap();
            foaf.additional_text_edits.is_some()
        };
        assert!(import("VERSION '1.2'\nSELECT * WHERE { ?s foaf:na| }\n"));
        // In an unclosed `BIND(`, `<` after the string `'1.2'` is the operator.
        assert!(!import(
            "VERSION '1.2'BIND(\nVERSION '1.2'\nSELECT * WHERE { ?s foaf:na| }\n"
        ));
    }

    #[test]
    fn an_added_prefix_ends_its_line_like_the_file() {
        let items = items("PREFIX a: <http://a/>\rSELECT * WHERE { ?s foaf:na| }\r");
        let edit = items
            .iter()
            .find_map(|i| i.additional_text_edits.as_ref())
            .map(|e| e[0].new_text.clone());
        assert_eq!(
            edit.as_deref(),
            Some("PREFIX foaf: <http://xmlns.com/foaf/0.1/>\r")
        );
    }

    fn has(got: &[String], want: &[&str]) {
        for w in want {
            assert!(got.iter().any(|g| g == w), "{w} not in {got:?}");
        }
    }

    fn lacks(got: &[String], unwanted: &[&str]) {
        for w in unwanted {
            assert!(!got.iter().any(|g| g == w), "{w} in {got:?}");
        }
    }

    const EX: &str = "PREFIX ex: <http://e/>\n";

    #[test]
    fn update_keywords() {
        has(&offered(&format!("{EX}INSERT |")), &["DATA"]);
        let after = offered(&format!("{EX}INSERT DATA {{ ex:a ex:b ex:c }} ;\n|"));
        // A prologue may come before each operation.
        has(&after, &["INSERT DATA", "DELETE", "LOAD", "PREFIX"]);
        lacks(&after, &["SELECT"]);
        has(
            &offered(&format!("{EX}DELETE {{ ?s ex:p ?o }} |")),
            &["INSERT", "WHERE"],
        );
    }

    #[test]
    fn pattern_keywords_after_nested_groups_and_filters() {
        for text in [
            "SELECT * WHERE { ?s ex:p ?o OPTIONAL { ?s ex:q ?x } | }",
            "SELECT * WHERE { ?s ex:p ?o FILTER (?o > 1) | }",
            "SELECT * WHERE { ?s ex:p \"x\" . | }",
        ] {
            has(&offered(&format!("{EX}{text}")), &["OPTIONAL", "BIND"]);
        }
        // A subquery can start a group.
        has(&offered(&format!("{EX}SELECT * WHERE {{ |")), &["SELECT"]);
    }

    #[test]
    fn values_rows_and_expressions() {
        has(
            &offered(&format!("{EX}SELECT * WHERE {{ VALUES ?x {{ | }} }}")),
            &["UNDEF"],
        );
        let filter = offered(&format!("{EX}SELECT * WHERE {{ ?s ex:p ?o FILTER(| }}"));
        has(&filter, &["STRLEN", "NOT EXISTS"]);
        lacks(&filter, &["COUNT", "OPTIONAL"]);
        has(
            &offered(&format!("{EX}SELECT * WHERE {{ ?s ex:p ?o }} ORDER BY |")),
            &["STR", "COUNT"],
        );
        has(
            &offered(&format!("{EX}SELECT * WHERE {{ ?s ex:p ?o }} HAVING (c|")),
            &["COUNT", "CONCAT"],
        );
    }

    #[test]
    fn slots_follow_the_triple() {
        // After `a ex:Person ,` classes still come first.
        let classes = offered(&format!("{EX}SELECT * WHERE {{ ?s a ex:Person , ex:| }}"));
        assert!(
            classes[0] == "ex:Org" || classes[0] == "ex:Person",
            "{classes:?}"
        );
        // After the predicate, an object: instances of the range first.
        let objects = offered(&format!(
            "{EX}SELECT * WHERE {{ ?s a ex:Person ; ex:worksFor ex:| }}"
        ));
        let mut first_two = objects[..2].to_vec();
        first_two.sort();
        assert_eq!(first_two, ["ex:acme", "ex:zed"], "{objects:?}");
        // After the subject, a predicate: the subject's class's properties.
        let preds = offered(&format!(
            "{EX}SELECT * WHERE {{ ?s a ex:Person . ?t ex:p ?o . ?s ex:| }}"
        ));
        assert_eq!(preds[0], "ex:worksFor", "{preds:?}");
        // Not for another subject.
        let other = offered(&format!(
            "{EX}SELECT * WHERE {{ ?s a ex:Person . ?t ex:| }}"
        ));
        assert_eq!(other[0], "ex:name", "{other:?}");
    }

    #[test]
    fn variables_keep_their_sigil_and_skip_the_one_typed() {
        let got = offered(&format!(
            "{EX}SELECT * WHERE {{ ?person ex:p $name . FILTER(?na| }}"
        ));
        has(&got, &["?name"]);
        lacks(&got, &["?na", "?person"]);
    }

    #[test]
    fn nothing_inside_strings_iris_or_comments() {
        for text in [
            "SELECT * WHERE { ?s ex:p \"ex:|\" }",
            "SELECT * WHERE { ?s ex:p \"ex:| }",
            "SELECT * WHERE { ?s ex:p <http://e/|> }",
            "# SEL|\nSELECT * {}",
        ] {
            assert!(offered(&format!("{EX}{text}")).is_empty(), "{text}");
        }
    }

    /// Every display feature at every offset of broken, multi-byte texts.
    #[test]
    fn never_panics_anywhere() {
        let texts = [
            "PREFIX é: <http://é/>\nSELECT ?ü (COUNT(?ü AS ?n) WHERE { ?ü a é:Ä ; é:p \"ö\"@de , <http://x/ü . OPTIONAL { ?ü é:",
            "INSERT DATA { <<( é:a é:b é:c )>> é:p [ é:q ( 1 2 ] } ; DELETE WHERE { ?s ?p ?o } ;; LOAD",
            "# ü\n$x ?? $ ? { } ) ( FILTER(( BIND(é:f(?x) AS ?y) } VALUES (?a { (1 UNDEF) } GROUP BY",
            "SELECT * {{{{ ?s ?p ?o }}}}} ORDER BY DESC(?s) LIMIT 1 OFFSET \"",
        ];
        let k = Knowledge::default();
        for text in texts {
            let q =
                SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
            for enc in [Encoding::Utf8, Encoding::Utf16] {
                let end = q.position(enc, text.len() as u32);
                let all = Range::new(Position::new(0, 0), end);
                let _ = super::super::semantic_tokens_full(&q, &k, enc);
                let _ = super::super::semantic_tokens_range(&q, &k, enc, all);
                let _ =
                    super::super::inlay_hints(&q, &k, enc, all, crate::settings::InlayLabels::All);
                let _ = super::super::document_symbols(&q, enc);
                let _ = super::super::folding_ranges(&q);
                for (o, _) in text.char_indices().chain([(text.len(), ' ')]) {
                    let pos = q.position(enc, o as u32);
                    let _ = super::super::hover(&q, &k, enc, pos);
                    let params = CompletionParams {
                        text_document_position: TextDocumentPositionParams {
                            text_document: TextDocumentIdentifier {
                                uri: "file:///q.rq".parse().unwrap(),
                            },
                            position: pos,
                        },
                        work_done_progress_params: Default::default(),
                        partial_result_params: Default::default(),
                        context: None,
                    };
                    let _ = completion(&q, &k, enc, &params);
                }
            }
        }
    }
}
