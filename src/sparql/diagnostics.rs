//! The checks a SPARQL document needs no workspace knowledge for: syntax
//! (by `spargebra`, plus what the structure catches that `spargebra` does
//! not), undeclared, unused, duplicate and redefined prefixes, and projected
//! variables the `WHERE` clause never binds.
//!
//! Every undeclared prefix is reported, on its prefixed name, and never as a
//! syntax error: for the `spargebra` run each undeclared prefix name is bound
//! to a placeholder namespace, so the parser does not stop at the first one
//! and finds the syntax errors after them.

use super::structure::{GroupKind, PrefixSource, Structure, reaches};
use super::validate::{self, ErrorAt, ErrorKind};
use super::{Diagnostic, Level, Token, TokenKind};
use crate::syntax::{PrefixMap, Span};
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::SparqlParser;

/// The namespace the `n`th undeclared prefix is bound to for the
/// `spargebra` run (not built from the name, which need not be IRI text).
fn placeholder(n: usize) -> String {
    format!("urn:x-turtle-lsp:undeclared:{n}:")
}

/// `foaf:` (or `:` for the empty prefix).
fn shown(name: &str) -> String {
    format!("{name}:")
}

pub(super) fn diagnostics(
    text: &str,
    tokens: &[Token],
    st: &Structure,
    prefixes: &PrefixMap,
    base: Option<&str>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    // Undeclared prefixes, every one.
    let mut undeclared: FxHashSet<&str> = FxHashSet::default();
    for r in &st.refs {
        if r.prefix == Some(PrefixSource::Undeclared)
            && let Some(p) = r.prefix_span()
        {
            let name = &text[p.start as usize..p.end as usize - 1];
            undeclared.insert(name);
            out.push(Diagnostic {
                span: r.span,
                level: Level::Error,
                code: "undeclared-prefix",
                message: format!("Undeclared prefix '{}'", shown(name)),
            });
        }
    }

    // Syntax, with the undeclared prefixes bound to placeholders.
    let mut parser = SparqlParser::new();
    if let Some(b) = base
        && let Ok(p) = parser.clone().with_base_iri(b)
    {
        parser = p;
    }
    for (name, ns) in prefixes {
        parser = match parser.clone().with_prefix(name, ns) {
            Ok(p) => p,
            Err(_) => parser
                .with_prefix(name, placeholder(0))
                .expect("placeholder namespace is an IRI"),
        };
    }
    for (n, name) in undeclared.iter().enumerate() {
        parser = parser
            .with_prefix(*name, placeholder(n + 1))
            .expect("placeholder namespace is an IRI");
    }
    // `spargebra` takes a prologue only at the start of an update, where the
    // grammar allows one before any operation: those declarations are left
    // out of the text it parses and their prefixes given to it instead.
    let mut blanked: Option<(String, Vec<Token>)> = None;
    if st.is_update
        && let Some(first) = st.operations.first()
    {
        let later = |s: &Span| s.start > first.keyword.start;
        let spans: Vec<Span> = (st.prefixes.iter().map(|p| p.span).filter(later))
            .chain(st.bases.iter().map(|b| b.span).filter(later))
            .chain(st.versions.iter().map(|v| v.span).filter(later))
            .collect();
        if !spans.is_empty() {
            let mut t = text.as_bytes().to_vec();
            for s in &spans {
                for b in &mut t[s.start as usize..s.end as usize] {
                    if !matches!(*b, b'\n' | b'\r') {
                        *b = b' ';
                    }
                }
            }
            // Every byte of whole characters became a space: still UTF-8.
            let t = String::from_utf8(t).unwrap_or_else(|_| text.to_owned());
            for p in st.prefixes.iter().filter(|p| later(&p.span)) {
                let ns = p.resolved.clone().unwrap_or_else(|| placeholder(0));
                parser = match parser.clone().with_prefix(&p.name, ns) {
                    Ok(next) => next,
                    Err(_) => parser,
                };
            }
            let toks = super::lexer::tokenize(&t);
            blanked = Some((t, toks));
        }
    }
    let (checked, checked_tokens) = match &blanked {
        Some((t, toks)) => (t.as_str(), toks.as_slice()),
        None => (text, tokens),
    };
    let structural: Vec<Diagnostic> = st
        .errors
        .iter()
        .map(|(span, msg)| syntax(*span, msg.clone()))
        .collect();
    if let Some((span, message)) = too_complex(text, checked_tokens) {
        out.push(syntax(span, message));
    } else if let Some(e) = validate::check_tokens(checked, checked_tokens, parser, st.is_update) {
        match &e.kind {
            ErrorKind::UndeclaredPrefix { name } => {
                let ErrorAt::Token(span) = e.at else {
                    unreachable!("an undeclared prefix has a position")
                };
                if !out
                    .iter()
                    .any(|d| d.code == "undeclared-prefix" && d.span.contains(span.start))
                {
                    out.push(Diagnostic {
                        span,
                        level: Level::Error,
                        code: "undeclared-prefix",
                        message: format!("Undeclared prefix '{}'", shown(name)),
                    });
                }
            }
            ErrorKind::Syntax => {
                let span = match e.at {
                    ErrorAt::Token(s) => s,
                    ErrorAt::End => end_span(text, tokens),
                    ErrorAt::Unknown => unknown_span(text, tokens, &e.message),
                };
                // What the structure found first explains the error better
                // (`spargebra` reports some checks at the end of a construct).
                if !structural.iter().any(|d| d.span.start <= span.start) {
                    out.push(syntax(span, syntax_message(text, span, &e)));
                }
            }
        }
    }
    out.extend(structural);

    prefix_lints(st, &mut out);
    unbound_projections(text, st, &mut out);
    out.sort_by_key(|d| (d.span.start, d.span.end));
    out
}

/// Nesting `spargebra` is not given: it recurses per level, and a few
/// thousand levels overflow a worker thread's stack. No query comes near.
const MAX_NESTING: u32 = 256;

/// `OPTIONAL` and `MINUS` clauses `spargebra` is not given: each nests the
/// algebra one level deeper (about 20,000 overflow an 8 MB stack).
const MAX_OPTIONALS: u32 = 4096;

/// Why `spargebra` is not given the text, and where: the bracket that opens
/// nesting level [`MAX_NESTING`] + 1, or the `OPTIONAL` / `MINUS` past
/// [`MAX_OPTIONALS`].
fn too_complex(text: &str, tokens: &[Token]) -> Option<(Span, String)> {
    let mut depth = 0u32;
    let mut optionals = 0u32;
    for t in tokens {
        match (t.kind, t.text(text)) {
            (TokenKind::Punct, "{" | "(" | "[" | "<<" | "<<(" | "{|") => {
                depth += 1;
                if depth > MAX_NESTING {
                    return Some((
                        t.span,
                        format!(
                            "SPARQL syntax error: nested too deeply (more than {MAX_NESTING} levels)"
                        ),
                    ));
                }
            }
            (TokenKind::Punct, "}" | ")" | "]" | ">>" | ")>>" | "|}") => {
                depth = depth.saturating_sub(1);
            }
            (TokenKind::Keyword, w)
                if w.eq_ignore_ascii_case("OPTIONAL") || w.eq_ignore_ascii_case("MINUS") =>
            {
                optionals += 1;
                if optionals > MAX_OPTIONALS {
                    return Some((
                        t.span,
                        format!(
                            "SPARQL syntax error: too large to check (more than {MAX_OPTIONALS} OPTIONAL and MINUS clauses)"
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    None
}

fn syntax(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        span,
        level: Level::Error,
        code: "syntax",
        message,
    }
}

fn syntax_message(text: &str, span: Span, e: &validate::SyntaxError) -> String {
    // A check the grammar's action made (`IRI parsing failed`, ...): an
    // unquoted item with words.
    let items = e.expected.strip_prefix("one of ").unwrap_or(&e.expected);
    if let Some(note) = items
        .split(", ")
        .find(|i| i.contains(' ') && !i.starts_with('[') && !i.starts_with('"'))
    {
        return format!("SPARQL syntax error: {note}");
    }
    match e.shown_expected() {
        Some(x) => format!("SPARQL syntax error: expected {}", describe(x)),
        None if e.expected.is_empty() => format!("SPARQL syntax error: {}", e.message),
        None if span.start < span.end => format!(
            "SPARQL syntax error: unexpected '{}'",
            &text[span.start as usize..span.end as usize]
        ),
        None => "SPARQL syntax error: the query ends too early".into(),
    }
}

/// The parser's expected items (`one of "$", "(", ['0'..='9'], ...`) in
/// words: the kinds of term they start, then the punctuation.
fn describe(expected: &str) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    let mut punct: Vec<String> = Vec::new();
    for item in expected
        .strip_prefix("one of ")
        .unwrap_or(expected)
        .split(", ")
    {
        let kind = match item {
            r#""?""# | r#""$""# => Some("a variable"),
            r#""<""# | r#"":""# => Some("an IRI"),
            r#""'""# | r#""'''""# | r#""\"""# | r#""\"\"\"""# | r#""true""# | r#""false""# => {
                Some("a literal")
            }
            r#""_:""# | r#""[""# => Some("a blank node"),
            "[_]" => continue,
            i if i.starts_with("['0'") => Some("a literal"),
            i if i.starts_with("['A'") => Some("an IRI"),
            _ => None,
        };
        match kind {
            Some(k) if !kinds.contains(&k) => kinds.push(k),
            Some(_) => {}
            None => {
                let shown = match item.strip_prefix('"').and_then(|i| i.strip_suffix('"')) {
                    Some(p) => format!("'{}'", p.replace("\\\"", "\"")),
                    None => item.to_owned(),
                };
                if !punct.contains(&shown) {
                    punct.push(shown);
                }
            }
        }
    }
    let all: Vec<String> = kinds.into_iter().map(str::to_owned).chain(punct).collect();
    match all.len() {
        0 => expected.to_owned(),
        1 => all[0].clone(),
        n => format!("{} or {}", all[..n - 1].join(", "), all[n - 1]),
    }
}

/// The last token that is not a comment, for "the text ends too early".
fn end_span(text: &str, tokens: &[Token]) -> Span {
    tokens
        .iter()
        .rev()
        .find(|t| t.kind != TokenKind::Comment)
        .map_or(Span::new(text.len() as u32, text.len() as u32), |t| t.span)
}

/// An error without a position: the blank node it names, else the first
/// token.
fn unknown_span(text: &str, tokens: &[Token], message: &str) -> Span {
    if let Some(i) = message.find("_:") {
        let label = message[i..].split_whitespace().next().unwrap_or("");
        if let Some(t) = tokens.iter().rev().find(|t| {
            t.kind == TokenKind::BlankNode
                && &text[t.span.start as usize..t.span.end as usize] == label
        }) {
            return t.span;
        }
    }
    tokens
        .iter()
        .find(|t| t.kind != TokenKind::Comment)
        .map_or(Span::new(0, 0), |t| t.span)
}

/// Duplicate, redefined and unused `PREFIX` declarations, as in Turtle.
fn prefix_lints(st: &Structure, out: &mut Vec<Diagnostic>) {
    let iri = |p: &super::PrefixDecl| p.resolved.clone().unwrap_or_else(|| p.iri.clone());
    let mut seen: FxHashMap<&str, String> = FxHashMap::default();
    for p in &st.prefixes {
        let this = iri(p);
        if let Some(prev) = seen.insert(&p.name, this.clone()) {
            if prev == this {
                out.push(Diagnostic {
                    span: p.span,
                    level: Level::Hint,
                    code: "duplicate-prefix",
                    message: format!("Prefix '{}' is declared twice", shown(&p.name)),
                });
            } else {
                out.push(Diagnostic {
                    span: p.name_span,
                    level: Level::Warning,
                    code: "redefined-prefix",
                    message: format!("Prefix '{}' is redefined (was <{prev}>)", shown(&p.name)),
                });
            }
        }
    }
    let used: FxHashSet<&str> = st
        .refs
        .iter()
        .filter_map(|r| match r.prefix {
            Some(PrefixSource::Declared(i)) => Some(st.prefixes[i as usize].name.as_str()),
            _ => None,
        })
        .collect();
    for p in st
        .prefixes
        .iter()
        .filter(|p| !used.contains(p.name.as_str()))
    {
        out.push(Diagnostic {
            span: p.span,
            level: Level::Hint,
            code: "unused-prefix",
            message: format!("Prefix '{}' is never used", shown(&p.name)),
        });
    }
}

/// Variables a `SELECT` projects that its `WHERE` clause never binds. Bound
/// means: in a triple pattern, `GRAPH ?g` / `SERVICE ?s`, `BIND ... AS`,
/// a `VALUES` header (in the `WHERE` or after it), `GROUP BY ... AS`, or a
/// subquery's projection; not in `FILTER`, `MINUS` or `EXISTS`.
fn unbound_projections(text: &str, st: &Structure, out: &mut Vec<Diagnostic>) {
    // Whether group `g` is in a `MINUS` or `EXISTS` inside group `top`:
    // its patterns bind nothing outside it.
    let silenced = |mut g: Option<u32>, top: u32| {
        while let Some(i) = g {
            if i == top {
                return false;
            }
            let grp = &st.groups[i as usize];
            if matches!(grp.kind, GroupKind::Minus | GroupKind::Exists) {
                return true;
            }
            g = grp.parent;
        }
        false
    };
    // Occurrences by name, in text order (the walk records them in order).
    let mut by_name: FxHashMap<&str, Vec<u32>> = FxHashMap::default();
    for (i, v) in st.vars.iter().enumerate() {
        by_name.entry(v.name.as_str()).or_default().push(i as u32);
    }
    for sel in &st.selects {
        if sel.star.is_some() {
            continue;
        }
        let Some(wg) = sel.where_group else { continue };
        let w = st.groups[wg as usize].span;
        // Where the select ends: its subquery's `}` or its operation's end
        // (a trailing `VALUES` or `GROUP BY ... AS` binds too).
        let end = match sel.group {
            Some(g) => st.groups[g as usize].span.end,
            None => st
                .operations
                .iter()
                .find(|o| o.scope == sel.scope)
                .map_or(u32::MAX, |o| o.span.end),
        };
        let mut done: FxHashSet<&str> = FxHashSet::default();
        for item in sel.items.iter().filter(|p| p.expr.is_none()) {
            let v = &st.vars[item.var as usize];
            if !done.insert(v.name.as_str()) {
                continue;
            }
            let occ = &by_name[v.name.as_str()];
            let from = occ.partition_point(|&o| st.vars[o as usize].span.start < w.start);
            let bound = occ[from..].iter().any(|&o| {
                let o = &st.vars[o as usize];
                if o.span.start > end {
                    return false;
                }
                let binds = match o.role {
                    super::VarRole::Projected => o.raw_scope != sel.scope,
                    r => r.binds(),
                };
                let inside = w.start <= o.span.start && o.span.end <= w.end;
                let after = matches!(o.role, super::VarRole::Values | super::VarRole::Assigned)
                    && o.span.start > w.end;
                binds
                    && (inside || after)
                    && !silenced(o.group, wg)
                    && reaches(st, o.raw_scope, sel.scope, &v.name)
            });
            if !bound {
                let written = &text[v.span.start as usize..v.span.end as usize];
                out.push(Diagnostic {
                    span: v.span,
                    level: Level::Warning,
                    code: "unbound-projection",
                    message: format!(
                        "Variable {written} is projected but never bound in the WHERE clause"
                    ),
                });
            }
        }
    }
}
