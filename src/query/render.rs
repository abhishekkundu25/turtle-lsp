//! Query results as text: pure functions over an [`Answer`].
//!
//! - [`table`]: an aligned text table, for terminals;
//! - [`markdown`]: a Markdown document (a table, a boolean or a Turtle block),
//!   for editors;
//! - [`json`]: SPARQL 1.1 Query Results JSON (`application/sparql-results+json`),
//!   for SELECT and ASK;
//! - [`csv`] and [`tsv`]: the SPARQL 1.1 Query Results CSV and TSV formats, for
//!   SELECT;
//! - [`turtle`]: the triples of a CONSTRUCT or DESCRIBE as formatted Turtle.
//!
//! The table, Markdown and Turtle compact IRIs with the given prefixes (the
//! table and Markdown also with the well-known vocabularies' prefixes, as
//! hover does); JSON, CSV and TSV give full IRIs.

use super::{Answer, Solutions};
use crate::features::format::{Options, format};
use crate::features::hover::short;
use crate::syntax::{PrefixMap, Syntax, XSD, parse};
use serde_json::{Map, Value, json};
use spargebra::term::{Literal, NamedOrBlankNode, Term, Triple};
use std::fmt::Write;

/// `t` as written in Turtle or SPARQL, IRIs compacted where a prefix of
/// `prefixes` (or of a well-known vocabulary) applies.
pub fn term(t: &Term, prefixes: &PrefixMap) -> String {
    match t {
        Term::NamedNode(n) => short(n.as_str(), prefixes),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => literal(l, &mut |dt| short(dt, prefixes)),
        Term::Triple(t) => format!(
            "<<( {} {} {} )>>",
            subject_term(&t.subject, prefixes),
            short(t.predicate.as_str(), prefixes),
            term(&t.object, prefixes)
        ),
    }
}

fn subject_term(s: &NamedOrBlankNode, prefixes: &PrefixMap) -> String {
    term(&s.clone().into(), prefixes)
}

/// A literal in Turtle: strings quoted, integers, decimals and booleans bare,
/// other datatypes written by `datatype`.
fn literal(l: &Literal, datatype: &mut dyn FnMut(&str) -> String) -> String {
    let quoted = quote(l.value());
    if let Some(lang) = l.language() {
        return match l.direction() {
            Some(d) => format!("{quoted}@{lang}--{d}"),
            None => format!("{quoted}@{lang}"),
        };
    }
    let dt = l.datatype().as_str();
    let bare = match dt.strip_prefix(XSD).unwrap_or("") {
        "string" => return quoted,
        "boolean" => matches!(l.value(), "true" | "false"),
        "integer" => is_integer(l.value()),
        "decimal" => is_decimal(l.value()),
        _ => false,
    };
    if bare {
        l.value().to_owned()
    } else {
        format!("{quoted}^^{}", datatype(dt))
    }
}

fn is_integer(s: &str) -> bool {
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
}

fn is_decimal(s: &str) -> bool {
    s.split_once('.').is_some_and(|(i, f)| {
        (matches!(i, "" | "+" | "-") || is_integer(i))
            && !f.is_empty()
            && f.bytes().all(|b| b.is_ascii_digit())
    })
}

/// `s` as a double-quoted Turtle / SPARQL string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            '\0'..='\u{1F}' | '\u{7F}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// "N row(s)" / "N triple(s)".
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// The rows shown, of `items` kept out of `total`, and, when some are not,
/// the note saying so.
fn cut<T>(items: &[T], total: usize, max: Option<usize>, what: &str) -> (usize, Option<String>) {
    let shown = max.map_or(items.len(), |m| m.min(items.len()));
    let note = (shown < total).then(|| format!("showing {shown} of {}", count(total, what)));
    (shown, note)
}

/// The most characters a cell of the table or the Markdown shows. A
/// `GROUP_CONCAT` or a long literal can be megabytes; a table of them is
/// unreadable and a Markdown document of them opens slowly.
pub const MAX_CELL_CHARS: usize = 1000;

/// `s` cut to [`MAX_CELL_CHARS`] characters, with `…` where it was cut and
/// `cut` raised by one.
fn clip(s: String, cut: &mut usize) -> String {
    // Bytes are never fewer than characters: short strings need no count.
    if s.len() <= MAX_CELL_CHARS {
        return s;
    }
    match s.char_indices().nth(MAX_CELL_CHARS) {
        Some((end, _)) => {
            *cut += 1;
            format!("{}…", &s[..end])
        }
        None => s,
    }
}

/// "N cell(s) shortened to 1000 characters", for the note under a table.
fn clipped_note(cut: usize) -> Option<String> {
    (cut > 0).then(|| {
        format!(
            "{} shortened to {MAX_CELL_CHARS} characters",
            count(cut, "cell")
        )
    })
}

/// An aligned text table: a header of `?variables`, a rule, one line per row
/// (unbound cells empty), at most `max_rows` rows and then a line "showing N
/// of M rows". Widths count characters. A cell longer than
/// [`MAX_CELL_CHARS`] is cut with `…`, and a line says how many were.
pub fn table(s: &Solutions, prefixes: &PrefixMap, max_rows: Option<usize>) -> String {
    let (shown, note) = cut(&s.rows, s.total(), max_rows, "row");
    let header: Vec<String> = s.variables.iter().map(|v| format!("?{v}")).collect();
    let mut cut = 0;
    let cells: Vec<Vec<String>> = s.rows[..shown]
        .iter()
        .map(|r| {
            r.iter()
                .map(|t| {
                    t.as_ref().map_or_else(String::new, |t| {
                        clip(one_line(&term(t, prefixes)), &mut cut)
                    })
                })
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in &cells {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |row: &[String]| {
        let mut out = String::new();
        for (i, (c, w)) in row.iter().zip(&widths).enumerate() {
            if i > 0 {
                out.push_str(" | ");
            }
            out.push_str(c);
            if i + 1 < row.len() {
                out.extend(std::iter::repeat_n(' ', w - c.chars().count()));
            }
        }
        out.push('\n');
        out
    };
    let mut out = line(&header);
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&rule.join("-+-"));
    out.push('\n');
    for row in &cells {
        out.push_str(&line(row));
    }
    for note in note.into_iter().chain(clipped_note(cut)) {
        out.push_str(&note);
        out.push('\n');
    }
    out
}

/// `s` with line breaks shown as `\n` (terms never have raw ones; a guard).
fn one_line(s: &str) -> String {
    s.replace('\r', "\\r").replace('\n', "\\n")
}

/// The answer as Markdown: a table of solutions (terms as code, at most
/// `max_rows` rows, then "showing N of M rows"), `true` or `false`, or the
/// triples (at most `max_rows`) in a Turtle code block. A table cell longer
/// than [`MAX_CELL_CHARS`] is cut with `…`, and a line says how many were.
pub fn markdown(a: &Answer, prefixes: &PrefixMap, max_rows: Option<usize>) -> String {
    match a {
        Answer::Boolean(b) => format!("**{b}**\n"),
        Answer::Solutions(s) => {
            if s.total() == 0 {
                return "No results.\n".into();
            }
            let (shown, note) = cut(&s.rows, s.total(), max_rows, "row");
            let mut out = String::new();
            let _ = writeln!(out, "{}\n", count(s.total(), "row"));
            let header: Vec<String> = s.variables.iter().map(|v| format!("?{v}")).collect();
            let _ = writeln!(out, "| {} |", header.join(" | "));
            let _ = writeln!(out, "|{}", "---|".repeat(header.len().max(1)));
            let mut cut = 0;
            for r in &s.rows[..shown] {
                let cells: Vec<String> = r
                    .iter()
                    .map(|t| {
                        t.as_ref()
                            .map_or_else(String::new, |t| code(&clip(term(t, prefixes), &mut cut)))
                    })
                    .collect();
                let _ = writeln!(out, "| {} |", cells.join(" | "));
            }
            for note in note.into_iter().chain(clipped_note(cut)) {
                let _ = write!(out, "\n_{}._\n", capitalize(&note));
            }
            out
        }
        Answer::Triples(ts) => {
            if ts.total() == 0 {
                return "No triples.\n".into();
            }
            let (shown, note) = cut(ts, ts.total(), max_rows, "triple");
            let mut out = format!("{}\n\n", count(ts.total(), "triple"));
            let body = turtle(&ts[..shown], prefixes);
            let fence = fence_for(&body);
            let _ = write!(out, "{fence}turtle\n{body}");
            if !body.ends_with('\n') {
                out.push('\n');
            }
            let _ = writeln!(out, "{fence}");
            if let Some(note) = note {
                let _ = write!(out, "\n_{}._\n", capitalize(&note));
            }
            out
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `s` as an inline code span in a table cell: pipes escaped (GitHub reads
/// `\|` as `|` inside a cell, code included), line breaks as `\n`, a fence
/// longer than any run of backticks inside.
fn code(s: &str) -> String {
    let s = one_line(s).replace('|', "\\|");
    let longest = s.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    if longest > 0 || s.starts_with(' ') || s.ends_with(' ') {
        format!("{fence} {s} {fence}")
    } else {
        format!("{fence}{s}{fence}")
    }
}

/// A code fence longer than any run of backticks in `body` (at least 3).
fn fence_for(body: &str) -> String {
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

/// SPARQL 1.1 Query Results JSON for solutions and booleans; `None` for
/// triples (not a format for graphs). Literals carry `xml:lang` (and
/// `its:dir` for a base direction) or a `datatype` other than `xsd:string`;
/// triple terms are SPARQL 1.2 `"type": "triple"`.
pub fn json(a: &Answer) -> Option<String> {
    let v = match a {
        Answer::Boolean(b) => json!({ "head": {}, "boolean": b }),
        Answer::Solutions(s) => {
            let bindings: Vec<Value> = s
                .rows
                .iter()
                .map(|r| {
                    let mut m = Map::new();
                    for (v, t) in s.variables.iter().zip(r) {
                        if let Some(t) = t {
                            m.insert(v.clone(), json_term(t));
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            json!({ "head": { "vars": s.variables }, "results": { "bindings": bindings } })
        }
        Answer::Triples(_) => return None,
    };
    let mut out = serde_json::to_string_pretty(&v).ok()?;
    out.push('\n');
    Some(out)
}

fn json_term(t: &Term) -> Value {
    match t {
        Term::NamedNode(n) => json!({ "type": "uri", "value": n.as_str() }),
        Term::BlankNode(b) => json!({ "type": "bnode", "value": b.as_str() }),
        Term::Literal(l) => {
            let mut m = Map::new();
            m.insert("type".into(), "literal".into());
            m.insert("value".into(), l.value().into());
            if let Some(lang) = l.language() {
                m.insert("xml:lang".into(), lang.into());
                if let Some(d) = l.direction() {
                    m.insert("its:dir".into(), d.to_string().into());
                }
            } else if l.datatype().as_str() != format!("{XSD}string") {
                m.insert("datatype".into(), l.datatype().as_str().into());
            }
            Value::Object(m)
        }
        Term::Triple(t) => json!({
            "type": "triple",
            "value": {
                "subject": json_term(&t.subject.clone().into()),
                "predicate": json_term(&t.predicate.clone().into()),
                "object": json_term(&t.object),
            }
        }),
    }
}

/// SPARQL 1.1 Query Results CSV: a header of variable names, then a line per
/// row, CRLF line ends. IRIs in full, literals as their lexical form, blank
/// nodes as `_:label`; a field with `"`, `,`, CR or LF is quoted, its `"`
/// doubled.
pub fn csv(s: &Solutions) -> String {
    let mut out = String::new();
    let field = |v: &str| {
        if v.contains(['"', ',', '\n', '\r']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_owned()
        }
    };
    let header: Vec<String> = s.variables.iter().map(|v| field(v)).collect();
    out.push_str(&header.join(","));
    out.push_str("\r\n");
    for r in &s.rows {
        let cells: Vec<String> = r
            .iter()
            .map(|t| match t {
                None => String::new(),
                Some(Term::NamedNode(n)) => field(n.as_str()),
                Some(Term::BlankNode(b)) => field(&format!("_:{}", b.as_str())),
                Some(Term::Literal(l)) => field(l.value()),
                Some(t @ Term::Triple(_)) => field(&t.to_string()),
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

/// SPARQL 1.1 Query Results TSV: a header of `?variables`, then a line per
/// row, LF line ends. Terms in SPARQL syntax with full IRIs (`<iri>`,
/// `"text"@en`, `"v"^^<datatype>`, `_:label`), integers and decimals bare;
/// tabs and line breaks in strings escaped.
pub fn tsv(s: &Solutions) -> String {
    let mut out = String::new();
    let header: Vec<String> = s.variables.iter().map(|v| format!("?{v}")).collect();
    out.push_str(&header.join("\t"));
    out.push('\n');
    for r in &s.rows {
        let cells: Vec<String> = r
            .iter()
            .map(|t| t.as_ref().map_or_else(String::new, full_term))
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    out
}

/// `t` in SPARQL syntax with full IRIs, numbers bare.
fn full_term(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => format!("<{}>", n.as_str()),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => {
            let dt = l.datatype().as_str();
            let numeric = l.language().is_none()
                && match dt.strip_prefix(XSD) {
                    Some("integer") => is_integer(l.value()),
                    Some("decimal") => is_decimal(l.value()),
                    _ => false,
                };
            if numeric {
                l.value().to_owned()
            } else {
                literal(l, &mut |dt| format!("<{dt}>"))
            }
        }
        Term::Triple(t) => format!(
            "<<( {} <{}> {} )>>",
            full_term(&t.subject.clone().into()),
            t.predicate.as_str(),
            full_term(&t.object)
        ),
    }
}

/// A predicate and its objects, for one subject.
type Objects<'t> = (&'t str, Vec<&'t Term>);

/// `triples` as Turtle laid out by the document formatter: `@prefix` lines
/// for the prefixes of `prefixes` it uses, then the triples grouped by
/// subject (in the order subjects first appear).
pub fn turtle(triples: &[Triple], prefixes: &PrefixMap) -> String {
    let mut used: Vec<(&str, &str)> = Vec::new();
    let mut iri = |i: &str| match turtle_name(i, prefixes) {
        Some((p, ns, name)) => {
            if !used.iter().any(|u| u.0 == p) {
                used.push((p, ns));
            }
            name
        }
        None => format!("<{i}>"),
    };
    // Subjects in first-seen order, each with its predicates and objects.
    let mut subjects: Vec<(&NamedOrBlankNode, Vec<Objects>)> = Vec::new();
    let mut index = rustc_hash::FxHashMap::default();
    for t in triples {
        let i = *index.entry(&t.subject).or_insert_with(|| {
            subjects.push((&t.subject, Vec::new()));
            subjects.len() - 1
        });
        let preds = &mut subjects[i].1;
        match preds.iter_mut().find(|p| p.0 == t.predicate.as_str()) {
            Some(p) => p.1.push(&t.object),
            None => preds.push((t.predicate.as_str(), vec![&t.object])),
        }
    }
    let mut body = String::new();
    for (s, preds) in &subjects {
        body.push_str(&turtle_term(&(*s).clone().into(), &mut iri));
        for (k, (p, objects)) in preds.iter().enumerate() {
            body.push_str(if k == 0 { " " } else { " ;\n    " });
            if *p == crate::syntax::RDF_TYPE {
                body.push('a');
            } else {
                body.push_str(&iri(p));
            }
            for (j, o) in objects.iter().enumerate() {
                body.push_str(if j == 0 { " " } else { ", " });
                body.push_str(&turtle_term(o, &mut iri));
            }
        }
        body.push_str(" .\n\n");
    }
    used.sort();
    let mut text = String::new();
    for (p, ns) in &used {
        let _ = writeln!(text, "@prefix {p}: <{ns}> .");
    }
    if !used.is_empty() {
        text.push('\n');
    }
    text.push_str(body.trim_end());
    text.push('\n');
    let parsed = parse(&text, &PrefixMap::default(), Syntax::Turtle);
    format(&text, &parsed, &PrefixMap::default(), &Options::default()).unwrap_or(text)
}

fn turtle_term(t: &Term, iri: &mut dyn FnMut(&str) -> String) -> String {
    match t {
        Term::NamedNode(n) => iri(n.as_str()),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => literal(l, iri),
        Term::Triple(t) => {
            let s = turtle_term(&t.subject.clone().into(), iri);
            let p = iri(t.predicate.as_str());
            let o = turtle_term(&t.object, iri);
            format!("<<( {s} {p} {o} )>>")
        }
    }
}

/// `iri` as a prefixed name with `prefixes` that Turtle reads back as `iri`:
/// (prefix, namespace, name).
fn turtle_name<'p>(iri: &str, prefixes: &'p PrefixMap) -> Option<(&'p str, &'p str, String)> {
    let name = crate::syntax::names::shorten(iri, prefixes)?;
    let (p, _) = name.split_once(':')?;
    let (p, ns) = prefixes.get_key_value(p)?;
    Some((p.as_str(), ns.as_str(), name))
}
