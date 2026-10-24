//! Completion inside JSON-LD.
//!
//! The slot comes from the text before the cursor, scanned as JSON that may be
//! half typed (an unclosed string ends at its line): a key (in quotes or
//! not), an `@type` value (or a value of one of its aliases), or nothing.
//! What is offered comes from the active context of the innermost object
//! around the cursor ([`crate::jsonld::SourceIndex::scope_at`]):
//!
//! - keys: keywords, the context's terms, `prefix:` for its prefixes, the
//!   workspace's properties under `@vocab` by local name, and compact IRIs
//!   `prefix:local` of properties in the context's namespaces;
//! - `@type` values: classes the same way.
//!
//! Typing `prefix:` for a prefix the context lacks but the prefix catalogue
//! knows offers that vocabulary's terms, and accepting one adds the prefix to
//! the nearest `@context` object. (The idea is SWLS's, the Semantic Web
//! Language Server by IMEC – IDLab – Ghent University, MIT licensed; no code
//! is taken from it.)
//!
//! Ranking, item building and resolve are Turtle's: the item's text edit
//! replaces the word between the quotes, never the quotes.

use super::ranking::{self, Subject};
use super::{
    Candidate, LIMIT, Slot, finish, fuzzy, guess_kind, items, local_name_candidates,
    namespace_terms, prefix_candidates, prefixed_terms,
};
use crate::analysis::{Analysis, Encoding};
use crate::jsonld::context::{ActiveContext, is_keyword};
use crate::jsonld::context_edit::{MEMBER, context_edit, string_end};
use crate::jsonld::source::UseRole;
use crate::knowledge::Knowledge;
use crate::syntax::{Iri, PrefixMap, Span};
use crate::vocab::{TermKind, VOCABS};
use lsp_types::*;
use rustc_hash::FxHashSet;

/// Keywords offered in a key.
const KEYWORDS: &[&str] = &[
    "@id",
    "@type",
    "@context",
    "@graph",
    "@reverse",
    "@included",
    "@nest",
    "@value",
    "@language",
    "@direction",
    "@list",
    "@set",
    "@index",
];

/// What belongs at the cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    Key,
    Type,
}

/// Where the cursor is, from [`scan`].
struct Cursor {
    place: Option<Place>,
    /// The word to replace: inside the quotes, or a bare word.
    word: (usize, usize),
    quoted: bool,
    /// The `{` of each object around the cursor, outermost first.
    objects: Vec<usize>,
}

pub(super) fn complete(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: usize,
) -> CompletionResponse {
    let empty = CompletionResponse::Array(vec![]);
    let text: &str = &a.text;
    let index = &a.source_index;
    let scope = index.scope_at(offset as u32);
    let initial;
    let ctx = match scope {
        Some(s) => index.context(s),
        None => {
            initial = ActiveContext::new(None);
            &initial
        }
    };
    let is_type =
        |key: &str| key == "@type" || ctx.term(key).and_then(|d| d.iri.as_deref()) == Some("@type");
    let cur = scan(text, offset, &is_type);
    let Some(place) = cur.place else {
        return empty;
    };
    let typed = &text[cur.word.0..offset];
    let slot = match place {
        Place::Key => Slot::Predicate,
        Place::Type => Slot::Class,
    };
    let prefixes = context_prefixes(ctx);

    let mut cands: Vec<Candidate> = Vec::new();
    let mut incomplete = false;
    if let Some((pfx, local)) = typed.split_once(':') {
        match prefixed_terms(k, pfx, local, slot, &prefixes) {
            Some(c) => cands = c,
            None => return empty,
        }
    } else {
        incomplete = true;
        if place == Place::Key {
            for kw in KEYWORDS {
                if typed.is_empty() || kw.starts_with(typed) || kw[1..].starts_with(typed) {
                    cands.push(Candidate::plain(
                        kw.to_string(),
                        CompletionItemKind::KEYWORD,
                        "keyword".into(),
                        2,
                    ));
                }
            }
        }
        term_candidates(&mut cands, ctx, typed, place, k);
        if let Some(vocab) = ctx.vocab() {
            cands.extend(
                namespace_terms(k, vocab, typed, slot, &|l| l.to_string(), None)
                    .into_iter()
                    .filter(|c| !c.label.contains(':') && ctx.term(&c.label).is_none()),
            );
        }
        if !typed.starts_with('@') {
            prefix_candidates(&mut cands, typed, &prefixes, "@context", false);
            if !typed.is_empty() {
                for (p, ns) in &prefixes {
                    cands.extend(namespace_terms(
                        k,
                        ns,
                        typed,
                        slot,
                        &|l| format!("{p}:{l}"),
                        None,
                    ));
                }
            }
            local_name_candidates(&mut cands, typed, slot, &prefixes);
        }
    }
    // One item per IRI, the first way of writing it (terms, then
    // vocab-relative names, then compact IRIs), and only what fits the slot.
    let mut seen: FxHashSet<String> = FxHashSet::default();
    cands.retain(|c| {
        let Some(iri) = &c.iri else { return true };
        fits(place, c.kind, iri, k) && seen.insert(iri.clone())
    });

    if place == Place::Key {
        let types = scope
            .filter(|s| cur.objects.last() == Some(&(s.span.start as usize)))
            .map(|s| types_in(a, s.span))
            .unwrap_or_default();
        if !types.is_empty() {
            let ranked = ranking::predicates_for(
                k,
                &[Subject {
                    types,
                    path: vec![],
                }],
                &prefixes,
            );
            incomplete |= apply_ranking(&mut cands, ranked, typed, ctx, &prefixes, &seen);
        }
    }

    incomplete |= finish(
        &mut cands,
        ranking::Ranking::default(),
        slot,
        typed,
        k,
        &prefixes,
    );
    let (start, end) = cur.word;
    let replace = a.range(enc, Span::new(start as u32, end as u32));
    let insertion = std::cell::OnceCell::new();
    let mut items = items(cands, replace, &prefixes, "@context", |p, ns| {
        let (at, del, new_text) = insertion
            .get_or_init(|| context_edit(text, &cur.objects, offset))
            .clone();
        let member = format!(
            "{}: {}",
            serde_json::to_string(p).unwrap_or_default(),
            serde_json::to_string(ns).unwrap_or_default()
        );
        TextEdit {
            range: Range::new(
                a.position(enc, at as u32),
                a.position(enc, (at + del) as u32),
            ),
            new_text: new_text.replace(MEMBER, &member),
        }
    });
    if !cur.quoted {
        for item in &mut items {
            if let Some(CompletionTextEdit::Edit(e)) = &mut item.text_edit {
                e.new_text = format!("\"{}\"", e.new_text);
            }
        }
    }
    CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    })
}

/// The context's terms that fit `place`: in a key, every term but prefixes
/// and classes (aliases as keywords); in an `@type` value, terms for
/// classes. All of them when there are few (the client filters), else those
/// matching `typed`.
fn term_candidates(
    cands: &mut Vec<Candidate>,
    ctx: &ActiveContext,
    typed: &str,
    place: Place,
    k: &Knowledge,
) {
    let few = ctx.terms().count() <= LIMIT;
    for (term, d) in ctx.terms() {
        let Some(iri) = d.iri.as_deref() else {
            continue;
        };
        if d.prefix || !(few || fuzzy(typed, term)) {
            continue;
        }
        if is_keyword(iri) {
            if place == Place::Key {
                cands.push(Candidate::plain(
                    term.to_string(),
                    CompletionItemKind::KEYWORD,
                    iri.to_string(),
                    1,
                ));
            }
            continue;
        }
        let kind = kind_of(k, iri);
        let fits = match place {
            Place::Key => kind != Some(TermKind::Class),
            Place::Type => {
                kind == Some(TermKind::Class)
                    || (kind.is_none() && term.starts_with(char::is_uppercase))
            }
        };
        if !fits {
            continue;
        }
        cands.push(Candidate {
            tier: ranking::NONE,
            label: term.to_string(),
            kind: match place {
                Place::Key => CompletionItemKind::PROPERTY,
                Place::Type => CompletionItemKind::CLASS,
            },
            detail: iri.to_string(),
            iri: Some(iri.to_string()),
            rank: 0,
            import: None,
        });
    }
}

/// What the bundled vocabularies or the workspace say `iri` is; `None` when
/// neither knows it.
fn kind_of(k: &Knowledge, iri: &str) -> Option<TermKind> {
    if let Some(t) = VOCABS.lookup(iri).and_then(|(v, l)| v.term(l)) {
        return Some(t.kind);
    }
    let i = Iri::get(iri)?;
    if !k.instances(&i).is_empty() {
        return Some(TermKind::Class);
    }
    match guess_kind(k, &i) {
        TermKind::Individual if !k.is_described(&i) => None,
        kind => Some(kind),
    }
}

/// Whether a candidate for `iri` belongs at `place`: no classes in keys;
/// only classes in `@type` values. Terms were sorted out already.
fn fits(place: Place, kind: CompletionItemKind, iri: &str, k: &Knowledge) -> bool {
    match place {
        Place::Key => kind == CompletionItemKind::PROPERTY,
        Place::Type => {
            kind == CompletionItemKind::CLASS
                || (kind == CompletionItemKind::REFERENCE
                    && Iri::get(iri).is_some_and(|i| !k.instances(&i).is_empty()))
        }
    }
}

/// The context's prefixes: terms usable as the prefix of a compact IRI.
fn context_prefixes(ctx: &ActiveContext) -> PrefixMap {
    ctx.terms()
        .filter(|(_, d)| d.prefix && !d.reverse)
        .filter_map(|(t, d)| Some((t.to_string(), d.iri.as_deref()?.to_string())))
        .filter(|(_, ns)| !is_keyword(ns) && !ns.starts_with("_:"))
        .collect()
}

/// The `@type` values of the object `span` itself (not of objects inside it).
fn types_in(a: &Analysis, span: Span) -> Vec<Iri> {
    let index = &a.source_index;
    let uses = &index.uses;
    let from = uses.partition_point(|u| u.span.start < span.start);
    uses[from..]
        .iter()
        .take_while(|u| u.span.start < span.end)
        .filter(|u| u.role == UseRole::Type)
        .filter(|u| index.scope_at(u.span.start).is_some_and(|s| s.span == span))
        .filter_map(|u| Iri::get(u.iri.as_deref()?))
        .collect()
}

/// Tags the candidates the ranking knows about and adds the ranked
/// properties not offered yet, written for this context. Returns whether the
/// ranking left some out.
fn apply_ranking(
    cands: &mut Vec<Candidate>,
    ranked: ranking::Ranking,
    typed: &str,
    ctx: &ActiveContext,
    prefixes: &PrefixMap,
    present: &FxHashSet<String>,
) -> bool {
    for c in cands.iter_mut() {
        if let Some(h) = c
            .iri
            .as_deref()
            .and_then(Iri::get)
            .and_then(|i| ranked.iris.get(&i))
        {
            c.tier = h.tier;
            if let Some(d) = &h.detail {
                c.detail = d.clone();
            }
        }
    }
    for (iri, h) in &ranked.iris {
        if present.contains(iri.as_str()) {
            continue;
        }
        let label = written(iri, ctx, prefixes);
        if !fuzzy(typed, &label) {
            continue;
        }
        cands.push(Candidate {
            tier: h.tier,
            label,
            kind: CompletionItemKind::PROPERTY,
            detail: h.detail.clone().unwrap_or_else(|| iri.to_string()),
            iri: Some(iri.to_string()),
            rank: 0,
            import: None,
        });
    }
    ranked.truncated
}

/// How to write `iri` as a key here: a term, a word under `@vocab`, a
/// compact IRI, else in full.
fn written(iri: &str, ctx: &ActiveContext, prefixes: &PrefixMap) -> String {
    if let Some((t, _)) = ctx
        .terms()
        .filter(|(_, d)| !d.prefix && d.iri.as_deref() == Some(iri))
        .min_by_key(|(t, _)| (t.len(), *t))
    {
        return t.to_string();
    }
    if let Some(local) = ctx.vocab().and_then(|v| iri.strip_prefix(v))
        && !local.is_empty()
        && !local.contains([':', '/', '#'])
        && ctx.term(local).is_none()
    {
        return local.to_string();
    }
    super::shorten(iri, prefixes).unwrap_or_else(|| iri.to_string())
}

// ---- the text around the cursor ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Key,
    Colon,
    Value,
}

enum Frame {
    Object {
        start: usize,
        expect: Expect,
        key: Option<(usize, usize)>,
        in_context: bool,
    },
    Array {
        key: Option<(usize, usize)>,
        in_context: bool,
    },
}

impl Frame {
    fn in_context(&self) -> bool {
        match self {
            Frame::Object { in_context, .. } | Frame::Array { in_context, .. } => *in_context,
        }
    }
}

fn is_bare(c: u8) -> bool {
    !(c.is_ascii_whitespace() || matches!(c, b'"' | b',' | b':' | b'{' | b'}' | b'[' | b']'))
}

/// Scans the JSON before `offset` (forgiving errors) for what the cursor is
/// in. `is_type(key)` tells whether a key's value is an `@type` value.
fn scan(text: &str, offset: usize, is_type: &dyn Fn(&str) -> bool) -> Cursor {
    let b = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    let mut in_string: Option<(usize, usize)> = None;
    while i < offset {
        match b[i] {
            b'"' => {
                let (end, _) = string_end(b, i);
                if offset <= end {
                    // The cursor is in this string; a bare end runs to the
                    // end of the word.
                    let mut e = end;
                    if b.get(end) != Some(&b'"') {
                        e = offset;
                        while e < end && (is_bare(b[e]) || b[e] == b':') {
                            e += 1;
                        }
                    }
                    in_string = Some((i + 1, e));
                    break;
                }
                if let Some(Frame::Object { expect, key, .. }) = stack.last_mut()
                    && *expect == Expect::Key
                {
                    *key = Some((i + 1, end));
                    *expect = Expect::Colon;
                }
                i = end + 1;
                continue;
            }
            b'{' => {
                let in_context = context_child(&stack, text);
                stack.push(Frame::Object {
                    start: i,
                    expect: Expect::Key,
                    key: None,
                    in_context,
                });
            }
            b'[' => {
                let in_context = context_child(&stack, text);
                let key = match stack.last() {
                    Some(Frame::Object { key, .. }) => *key,
                    _ => None,
                };
                stack.push(Frame::Array { key, in_context });
            }
            b'}' | b']' => {
                stack.pop();
            }
            b':' => {
                if let Some(Frame::Object { expect, .. }) = stack.last_mut() {
                    *expect = Expect::Value;
                }
            }
            b',' => {
                if let Some(Frame::Object { expect, key, .. }) = stack.last_mut() {
                    *expect = Expect::Key;
                    *key = None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (word, quoted) = match in_string {
        Some(w) => (w, true),
        None => {
            let mut s = offset;
            while s > 0 && (is_bare(b[s - 1]) || b[s - 1] == b':') {
                s -= 1;
            }
            let mut e = offset;
            while e < b.len() && (is_bare(b[e]) || b[e] == b':') {
                e += 1;
            }
            ((s, e), false)
        }
    };
    let key_is_type = |key: &Option<(usize, usize)>| key.is_some_and(|(s, e)| is_type(&text[s..e]));
    let place = match stack.last() {
        Some(f) if f.in_context() => None,
        Some(Frame::Object {
            expect: Expect::Key,
            ..
        }) => Some(Place::Key),
        Some(Frame::Object {
            expect: Expect::Value,
            key,
            ..
        }) if key_is_type(key) => Some(Place::Type),
        Some(Frame::Array { key, .. }) if key_is_type(key) => Some(Place::Type),
        _ => None,
    };
    // A bare word in a value is a number or a literal, not a type.
    let place = place.filter(|p| quoted || *p == Place::Key);
    let objects = stack
        .iter()
        .filter_map(|f| match f {
            Frame::Object { start, .. } => Some(*start),
            Frame::Array { .. } => None,
        })
        .collect();
    Cursor {
        place,
        word,
        quoted,
        objects,
    }
}

/// Whether a container opened now is inside a `@context`: its parent is, or
/// it is the parent's `@context` value.
fn context_child(stack: &[Frame], text: &str) -> bool {
    match stack.last() {
        None => false,
        Some(f @ Frame::Array { .. }) => f.in_context(),
        Some(Frame::Object {
            in_context, key, ..
        }) => *in_context || key.is_some_and(|(s, e)| &text[s..e] == "@context"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(text: &str) -> (Option<Place>, String, bool) {
        let offset = text.find('|').unwrap();
        let text = text.replace('|', "");
        let c = scan(&text, offset, &|k| k == "@type" || k == "type");
        (c.place, text[c.word.0..c.word.1].to_string(), c.quoted)
    }

    #[test]
    fn finds_the_slot_in_half_typed_json() {
        assert_eq!(
            place(r#"{ "na|me": 1 }"#),
            (Some(Place::Key), "name".into(), true)
        );
        assert_eq!(
            place("{ \"na|\n  \"x\": 1 }"),
            (Some(Place::Key), "na".into(), true)
        );
        assert_eq!(
            place(r#"{ "a": 1, |"#),
            (Some(Place::Key), "".into(), false)
        );
        assert_eq!(
            place(r#"{ "a": 1, fo|"#),
            (Some(Place::Key), "fo".into(), false)
        );
        assert_eq!(
            place(r#"{ "@type": "Wi|dget" }"#),
            (Some(Place::Type), "Widget".into(), true)
        );
        assert_eq!(
            place(r#"{ "type": ["A", "|"#),
            (Some(Place::Type), "".into(), true)
        );
        assert_eq!(place(r#"{ "name": "A|"#).0, None);
        assert_eq!(place(r#"{ "name"|: 1 }"#).0, None);
        assert_eq!(place(r#"{ "@context": { "na|" } }"#).0, None);
        assert_eq!(place(r#"{ "@context": [ "x", { "na|" } ] }"#).0, None);
        assert_eq!(place(r#"{ "@graph": [ { "na|" } ] }"#).0, Some(Place::Key));
        assert_eq!(
            place(r#"{ "a": "x\"y", "@type": "fo:B|"#),
            (Some(Place::Type), "fo:B".into(), true)
        );
    }

    fn edit(text: &str) -> String {
        let offset = text.find('|').unwrap();
        let text = text.replace('|', "");
        let c = scan(&text, offset, &|k| k == "@type");
        let (at, del, new) = context_edit(&text, &c.objects, offset);
        let new = new.replace(MEMBER, r#""p": "n""#);
        format!("{}{}{}", &text[..at], new, &text[at + del..])
    }

    #[test]
    fn adds_the_prefix_to_the_nearest_context() {
        assert_eq!(
            edit("{\n  \"@context\": {\n    \"a\": \"b\"\n  },\n  \"x|\"\n}"),
            "{\n  \"@context\": {\n    \"p\": \"n\",\n    \"a\": \"b\"\n  },\n  \"x\"\n}"
        );
        assert_eq!(
            edit(r#"{ "@context": {}, "x|" }"#),
            r#"{ "@context": {"p": "n"}, "x" }"#
        );
        assert_eq!(
            edit(r#"{ "@context": "https://s.org/", "x|" }"#),
            r#"{ "@context": ["https://s.org/", { "p": "n" }], "x" }"#
        );
        assert_eq!(
            edit(r#"{ "@context": ["https://s.org/"], "x|" }"#),
            r#"{ "@context": ["https://s.org/", { "p": "n" }], "x" }"#
        );
        assert_eq!(edit(r#"{ "x|" }"#), r#"{ "@context": { "p": "n" }, "x" }"#);
        // The inner object's own context is nearer.
        assert_eq!(
            edit(r#"{ "@context": { "a": "b" }, "y": { "@context": { "c": "d" }, "x|" } }"#),
            r#"{ "@context": { "a": "b" }, "y": { "@context": { "p": "n", "c": "d" }, "x" } }"#
        );
    }

    /// A `//` comment or an unclosed string, which run to the end of their
    /// line, take in no part of the member.
    #[test]
    fn the_member_lands_outside_comments_and_unclosed_strings() {
        let lexed = |text: &str| -> Vec<String> {
            crate::jsonld::json::tokens(text)
                .iter()
                .map(|t| text[t.span.start as usize..t.span.end as usize].to_string())
                .collect()
        };
        // An unclosed array ends in a comment or an unclosed string: the
        // member goes after its `[`.
        let out = edit(r#"{ "x|": 1, "@context": [ // c"#);
        assert_eq!(out, r#"{ "x": 1, "@context": [{ "p": "n" } // c"#);
        assert_eq!(
            lexed(&out),
            ["{", r#""x""#, ":", "1", ",", r#""@context""#, ":", "["]
                .into_iter()
                .chain(["{", r#""p""#, ":", r#""n""#, "}", "// c"])
                .collect::<Vec<_>>()
        );
        assert_eq!(
            edit(r#"{ "x|": 1, "@context": ["c.jsonld"#),
            r#"{ "x": 1, "@context": [{ "p": "n" }, "c.jsonld"#
        );
        // An unclosed string value: the rest of the array starts a line.
        let out = edit("{ \"@context\": \"c.jsonld\n  \"x|\": 1 }");
        assert_eq!(
            out,
            "{ \"@context\": [\"c.jsonld\n, { \"p\": \"n\" }]\n  \"x\": 1 }"
        );
        assert_eq!(
            lexed(&out)[..6],
            ["{", r#""@context""#, ":", "[", "\"c.jsonld", ","]
        );
    }
}
