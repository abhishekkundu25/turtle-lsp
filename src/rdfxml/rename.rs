//! Rename in an RDF/XML document: the edits that make it name one IRI
//! instead of another, placed at the spans the grammar reads
//! ([`grammar::uses`]), or why they cannot be made.
//!
//! [`plan`] edits each use of the old IRI in its own form: an attribute
//! value as it was written (relative against the base in scope when the new
//! IRI is under it, `&e;local` when the new IRI extends the entity), a name
//! with a prefix in scope, `rdf:ID` by its fragment. It refuses a use that
//! no edit of its own text can change: one read from an entity's
//! replacement text, one written with character references, `rdf:li`, a
//! name whose new namespace has no prefix in scope, and the IRIs a
//! production implies. Then it checks its work: the edited text, parsed
//! again, must give the same graph with the old IRI read as the new one,
//! and no error; else it refuses. A document with a syntax error, or with
//! default attribute values in its DOCTYPE, is refused whole, and
//! [`blocked`] refuses it for any rename of an IRI its text may spell, as
//! the grammar may not read every use there. [`at`] says what a rename
//! started at an offset renames, or why every rename from there is
//! refused.

use super::grammar::{self, Base, Use, UseKind, Want, read_iri, resolves};
use super::xml::{Refs, is_nc_name};
use crate::syntax::{Iri, Node, Parsed, PrefixMap, Span, Triple};
use std::cmp::Reverse;

/// Why a rename cannot edit a document, and the line (from 1) of the use
/// that stops it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) line: u32,
    pub(crate) reason: String,
}

impl Refusal {
    fn at(text: &str, offset: u32, reason: String) -> Refusal {
        Refusal {
            line: line(text, offset),
            reason,
        }
    }
}

/// The line (from 1) of `offset` in `text`. An edit writes no line break,
/// so a line of the edited text is the same line of the text.
fn line(text: &str, offset: u32) -> u32 {
    let end = (offset as usize).min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&c| c == b'\n')
        .count() as u32
        + 1
}

const FROM_ENTITY: &str = "the IRI is read from an entity's replacement text: editing the declaration would change every use of the entity";
const CHAR_REFS: &str =
    "the value is written with character references, which a rename does not rewrite";
const ENTITY_REFS: &str =
    "the value is written with entity references that a rename cannot rewrite one by one";
const ATTLIST: &str = "the DOCTYPE gives attributes default values (<!ATTLIST>), which this reader does not add: the document is not edited";

/// Why no rename edits `text` (parsed as `parsed`), whatever it names: its
/// first syntax error, as a broken text may hide a use from the grammar,
/// or a default attribute value its DOCTYPE declares, which this reader
/// does not add to the elements.
fn unedited(text: &str, parsed: &Parsed) -> Option<Refusal> {
    if let Some(e) = parsed.errors.iter().min_by_key(|e| e.span.start) {
        return Some(Refusal::at(
            text,
            e.span.start,
            format!(
                "the document has a syntax error ({}): it is not edited until it is fixed",
                e.message()
            ),
        ));
    }
    attlist_default(text).map(|at| Refusal::at(text, at, ATTLIST.into()))
}

/// Where `text` declares an attribute list with a default value (a quoted
/// value before the declaration's `>`); `#IMPLIED` and `#REQUIRED` add
/// nothing.
fn attlist_default(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("<!ATTLIST") {
        let start = from + i;
        let mut j = start + "<!ATTLIST".len();
        while j < b.len() && b[j] != b'>' {
            if matches!(b[j], b'"' | b'\'') {
                return Some(start as u32);
            }
            j += 1;
        }
        from = j;
    }
    None
}

/// Why a rename of `old` refuses an RDF/XML document that the grammar may
/// not read in full ([`unedited`]) and whose text names `old` in some
/// spelling ([`names`]): every use there or none. `None` for a text in an
/// OWL syntax that is not RDF, which holds no RDF/XML use.
pub(crate) fn blocked(text: &str, parsed: &Parsed, old: &str) -> Option<Refusal> {
    if super::unread(text).is_some() {
        return None;
    }
    let r = unedited(text, parsed)?;
    let base = parsed.document_base.as_deref().map(|b| b.split('#').next());
    (base == Some(old.split('#').next()) || names(text, old)).then_some(r)
}

/// Whether `text` may spell `old`, over-counting: the IRI itself, or its
/// local name standing alone (after the last `#`, `/` or `:`, or after a
/// quoted namespace or entity value the text writes), in the text or in it
/// with character references and predefined entities expanded. An IRI
/// with no local name (a namespace) may be spelled by any relative value.
fn names(text: &str, old: &str) -> bool {
    let cut = old.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
    if cut == old.len() {
        return true;
    }
    let expanded = text.contains('&').then(|| unescaped(text));
    let spells = |t: &str| {
        t.contains(old)
            || alone(t, &old[cut..])
            || old[cut..]
                .char_indices()
                .skip(1)
                .map(|(i, _)| cut + i)
                .any(|i| {
                    (t.contains(&format!("\"{}\"", &old[..i]))
                        || t.contains(&format!("'{}'", &old[..i])))
                        && alone(t, &old[i..])
                })
    };
    spells(text) || expanded.as_deref().is_some_and(spells)
}

/// Whether `word` occurs in `t` with no name character on either side.
fn alone(t: &str, word: &str) -> bool {
    let name_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\u{b7}');
    t.match_indices(word).any(|(i, _)| {
        !t[..i].chars().next_back().is_some_and(name_char)
            && !t[i + word.len()..].chars().next().is_some_and(name_char)
    })
}

/// `text` with its character references and predefined entities expanded.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let semi = rest[..rest.len().min(12)].find(';');
        let c = semi.and_then(|e| {
            let name = &rest[1..e];
            match name.strip_prefix('#') {
                Some(n) => match n.strip_prefix('x') {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                }
                .and_then(char::from_u32),
                None => predefined(name),
            }
        });
        match (c, semi) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character a predefined entity (`amp`, `lt`, `gt`, `apos`, `quot`)
/// stands for.
fn predefined(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "quot" => '"',
        _ => return None,
    })
}

/// The edits that make `text` (whose parse the workspace holds as
/// `parsed`) name `new` wherever it names `old`, sorted, or why the
/// document cannot be edited so: the first use that stops it.
pub(crate) fn plan(
    text: &str,
    parsed: &Parsed,
    old: Iri,
    new: &str,
) -> Result<Vec<(Span, String)>, Refusal> {
    // No IRI is used where no text is read (OWL/XML).
    if old == new || super::unread(text).is_some() {
        return Ok(Vec::new());
    }
    let base = parsed.document_base.as_deref();
    let (fresh, uses) = grammar::uses(text, base, Want::Iri(old));
    if let Some(r) = unedited(text, &fresh) {
        return Err(r);
    }
    // (line, written: 0 before implied: 1, reason)
    let mut refused: Vec<(u32, u8, String)> = Vec::new();
    let mut edits = Vec::new();
    for u in &uses {
        match edit(text, u, new) {
            Ok(e) => edits.extend(e),
            Err(reason) => refused.push((line(text, u.span.start), 0, reason)),
        }
    }
    edits.sort_by_key(|(s, _)| (s.start, s.end));
    edits.dedup_by_key(|(s, _)| *s);
    let after = apply(text, &edits);
    let reparsed = grammar::parse(&after, base);
    // What still names the old IRI: a production implies it.
    if let Some(at) = reparsed
        .triples
        .iter()
        .filter_map(|t| naming(t, old))
        .min_by_key(|s| s.start)
    {
        refused.push((
            line(&after, at.start),
            1,
            format!(
                "the RDF/XML syntax implies <{old}> here, with no text of its own: the document cannot be rewritten to say <{new}>"
            ),
        ));
    }
    if let Some((line, _, reason)) = refused.into_iter().min_by_key(|r| (r.0, r.1)) {
        return Err(Refusal { line, reason });
    }
    if let Some(e) = reparsed.errors.first() {
        return Err(Refusal::at(
            &after,
            e.span.start,
            format!(
                "the new name would make the document wrong here ({})",
                e.message()
            ),
        ));
    }
    let new_iri = Iri::intern(new);
    let changed = parsed
        .triples
        .iter()
        .map(|t| renamed(t, old, new_iri))
        .zip(reparsed.triples.iter().map(|t| renamed(t, old, old)))
        .position(|(want, got)| want != got);
    if changed.is_some() || parsed.triples.len() != reparsed.triples.len() {
        let at = changed
            .and_then(|i| reparsed.triples.get(i))
            .map_or(after.len() as u32, |t| t.p_span.start);
        return Err(Refusal::at(
            &after,
            at,
            format!(
                "with <{new}> written here, the document would say something else than the rename means"
            ),
        ));
    }
    Ok(edits)
}

/// `text` with `edits` (sorted, apart) applied.
pub(crate) fn apply(text: &str, edits: &[(Span, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (span, new) in edits {
        out.push_str(&text[at..span.start as usize]);
        out.push_str(new);
        at = span.end as usize;
    }
    out.push_str(&text[at..]);
    out
}

/// A triple's terms with `from` read as `to`, for comparing graphs.
fn renamed(t: &Triple, from: Iri, to: Iri) -> (Node, Iri, Node, Option<crate::syntax::GraphName>) {
    fn node(n: &Node, from: Iri, to: Iri) -> Node {
        match n {
            Node::Iri(i) if *i == from => Node::Iri(to),
            Node::Literal {
                value,
                datatype,
                lang,
                dir,
            } if *datatype == from => Node::Literal {
                value: value.clone(),
                datatype: to,
                lang: *lang,
                dir: *dir,
            },
            Node::Triple(t) => Node::Triple(Box::new((
                node(&t.0, from, to),
                if t.1 == from { to } else { t.1 },
                node(&t.2, from, to),
            ))),
            n => n.clone(),
        }
    }
    let p = if t.p == from { to } else { t.p };
    (node(&t.s, from, to), p, node(&t.o, from, to), t.graph)
}

/// Where triple `t` names `iri`, if it does: at its predicate, else at its
/// object (or the predicate's span for its subject).
fn naming(t: &Triple, iri: Iri) -> Option<Span> {
    fn names(n: &Node, iri: Iri) -> bool {
        match n {
            Node::Iri(i) => *i == iri,
            Node::Literal { datatype, .. } => *datatype == iri,
            Node::Triple(t) => names(&t.0, iri) || t.1 == iri || names(&t.2, iri),
            Node::Blank(_) => false,
        }
    }
    if t.p == iri || names(&t.s, iri) {
        Some(t.p_span)
    } else {
        names(&t.o, iri).then_some(t.o_span)
    }
}

/// Why no edit of `u` names another IRI, whatever it is: a use read from
/// an entity's replacement text, `rdf:li`, an `rdf:type` attribute name, a
/// value written with references a rename does not rewrite, an element
/// that is not closed.
fn fixed(text: &str, u: &Use) -> Option<String> {
    match &u.kind {
        UseKind::Value { entity: true, .. }
        | UseKind::Id { entity: true, .. }
        | UseKind::Element { entity: true, .. }
        | UseKind::Attr { entity: true, .. } => Some(FROM_ENTITY.into()),
        UseKind::Li => Some(format!(
            "rdf:li stands for <{}>, which no text names: the document cannot be rewritten to say another IRI there",
            u.iri
        )),
        UseKind::TypeAttr => Some(
            "the attribute rdf:type is RDF/XML syntax: it cannot be written for another IRI".into(),
        ),
        UseKind::Namespace | UseKind::Attr { .. } => None,
        UseKind::Value {
            text: value,
            refs,
            lead,
            ..
        } => match refs {
            Refs::None => None,
            Refs::Chars => Some(CHAR_REFS.into()),
            Refs::Entity => {
                let Some((r, n)) = lead.filter(|(r, _)| r.start == u.span.start) else {
                    return Some(ENTITY_REFS.into());
                };
                let rest = &text[r.end as usize..u.span.end as usize];
                if rest.is_empty() {
                    let name = &text[r.start as usize + 1..r.end as usize - 1];
                    return Some(format!(
                        "the whole IRI comes from the entity &{name};: editing its declaration would change every use of the entity"
                    ));
                }
                (rest.contains('&') || value.get(n as usize..) != Some(rest))
                    .then(|| ENTITY_REFS.into())
            }
        },
        UseKind::Id { refs, .. } => (*refs != Refs::None).then(|| CHAR_REFS.into()),
        UseKind::Element { end, .. } => match end {
            Some(e) if !e.implied => None,
            _ => {
                let raw = &text[u.span.start as usize..u.span.end as usize];
                Some(format!(
                    "<{raw}> is not closed: its end tag cannot change with it"
                ))
            }
        },
    }
}

/// The edits of one use, or why it cannot be edited.
fn edit(text: &str, u: &Use, new: &str) -> Result<Vec<(Span, String)>, String> {
    if let Some(why) = fixed(text, u) {
        return Err(why);
    }
    match &u.kind {
        UseKind::Value {
            base,
            text: value,
            refs,
            lead,
            ..
        } => Ok(value_edit(text, u, base, value, *refs, *lead, new)),
        UseKind::Id { base, .. } => {
            let local = new.rsplit_once('#').map(|(_, l)| l);
            match local {
                Some(l)
                    if is_nc_name(l)
                        && read_iri(base, &format!("#{l}")).as_deref() == Some(new) =>
                {
                    Ok(vec![(u.span, l.to_string())])
                }
                _ => Err(format!(
                    "rdf:ID names an IRI {}… with an XML name after `#`, and <{new}> is none",
                    read_iri(base, "#").unwrap_or_default()
                )),
            }
        }
        UseKind::Element {
            ns,
            prefix_len,
            end,
            ..
        } => {
            let name = qname(text, u, ns, *prefix_len, true, new)?;
            let mut out = vec![(u.span, name.clone())];
            if let Some(n) = end.and_then(|e| e.name) {
                out.push((n, name));
            }
            Ok(out)
        }
        UseKind::Attr { ns, prefix_len, .. } => {
            Ok(vec![(u.span, qname(text, u, ns, *prefix_len, false, new)?)])
        }
        UseKind::Li | UseKind::TypeAttr | UseKind::Namespace => Ok(Vec::new()),
    }
}

/// The edit of an IRI attribute value whose text, references expanded, is
/// `value`, which [`fixed`] passed.
fn value_edit(
    text: &str,
    u: &Use,
    base: &Base,
    value: &str,
    refs: Refs,
    lead: Option<(Span, u32)>,
    new: &str,
) -> Vec<(Span, String)> {
    let span = u.span;
    let quote = (span.start as usize)
        .checked_sub(1)
        .and_then(|i| text.as_bytes().get(i).copied())
        .unwrap_or(b'"');
    let w = written(value, base, u.iri.as_str(), new);
    match (refs, lead) {
        (Refs::Entity, Some((r, n))) => match w.strip_prefix(&value[..n as usize]) {
            Some(local) if !local.is_empty() => {
                vec![(Span::new(r.end, span.end), escaped(local, quote))]
            }
            _ => vec![(span, escaped(new, quote))],
        },
        _ => vec![(span, escaped(&w, quote))],
    }
}

/// The value to write for `new` where `raw` named `old`: relative the same
/// way when `raw` was relative to the base and `new` is under it, else
/// `new` in full.
fn written(raw: &str, base: &Base, old: &str, new: &str) -> String {
    if resolves(base, raw)
        && !raw.is_empty()
        && let Some(stem) = old.strip_suffix(raw)
        && let Some(rel) = new.strip_prefix(stem)
        && !rel.is_empty()
        && read_iri(base, rel).as_deref() == Some(new)
    {
        return rel.to_string();
    }
    new.to_string()
}

/// `s` as attribute value text between `quote`s.
fn escaped(s: &str, quote: u8) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if quote == b'"' => out.push_str("&quot;"),
            '\'' if quote == b'\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The name to write for `new` where a name in namespace `ns` was: with
/// its own prefix when `new` is in that namespace, else with another
/// prefix in scope (the longest namespace first), else (an element's
/// only) in the default namespace.
fn qname(
    text: &str,
    u: &Use,
    ns: &str,
    prefix_len: Option<u32>,
    element: bool,
    new: &str,
) -> Result<String, String> {
    let raw = &text[u.span.start as usize..u.span.end as usize];
    let own = prefix_len.map_or("", |n| &raw[..n as usize]);
    let fits = |p: &str, ns: &str| {
        let local = new.strip_prefix(ns).filter(|l| is_nc_name(l))?;
        Some(if p.is_empty() {
            local.to_string()
        } else {
            format!("{p}:{local}")
        })
    };
    if let Some(q) = fits(own, ns) {
        return Ok(q);
    }
    let mut others: Vec<&(String, std::rc::Rc<str>)> = u
        .scope
        .iter()
        .filter(|(p, _)| p != own && (element || !p.is_empty()))
        .collect();
    others.sort_by_key(|(p, ns)| (p.is_empty(), Reverse(ns.len()), p.clone()));
    if let Some(q) = others.iter().find_map(|(p, ns)| fits(p, ns)) {
        return Ok(q);
    }
    Err(match new.strip_prefix(ns) {
        Some(local) => format!("{local:?} is not an XML name, which the name {raw} must end with"),
        None => format!(
            "no prefix in scope at {raw} is bound to a namespace of <{new}>, and a rename adds no xmlns declaration"
        ),
    })
}

/// What a rename started at `offset` of RDF/XML `text` renames.
pub(crate) enum At {
    /// The IRI written there: the span of its written form, and the
    /// namespaces in scope by prefix (`""` for the default namespace), with
    /// which a new name reads.
    Use {
        span: Span,
        iri: Iri,
        prefixes: PrefixMap,
    },
    /// The name of an `xmlns` declaration: a prefix, which is not renamed
    /// in RDF/XML.
    Namespace,
}

/// What a rename at `offset` of `text`, with document base `base`, starts
/// on: `None` where nothing written names an IRI (text, a comment, the
/// `rdf:RDF` name), and anywhere in a text that [`super::parse`] does not
/// read (OWL/XML). `Err` where every rename from there is refused,
/// whatever the new name: anywhere in a document that no rename edits
/// ([`unedited`]), and on an IRI one of whose uses here no edit can change
/// (`rdf:li`, a name or value read from an entity, `rdf:type` as an
/// attribute name, a value with character references), with the first of
/// those uses.
pub(crate) fn at(text: &str, base: Option<&str>, offset: u32) -> Result<Option<At>, Refusal> {
    if super::unread(text).is_some() {
        return Ok(None);
    }
    let (fresh, uses) = grammar::uses(text, base, Want::At(offset));
    if let Some(r) = unedited(text, &fresh) {
        return Err(r);
    }
    let Some(u) = uses.into_iter().min_by_key(|u| u.span.end - u.span.start) else {
        return Ok(None);
    };
    if let UseKind::Namespace = u.kind {
        return Ok(Some(At::Namespace));
    }
    // A use of this IRI that no edit can change refuses every rename of it.
    let (_, all) = grammar::uses(text, base, Want::Iri(u.iri));
    if let Some(r) = all
        .iter()
        .filter_map(|w| Some((w.span.start, fixed(text, w)?)))
        .min_by_key(|(start, _)| *start)
        .map(|(start, why)| Refusal::at(text, start, why))
    {
        return Err(r);
    }
    Ok(Some(At::Use {
        span: u.span,
        iri: u.iri,
        prefixes: u
            .scope
            .iter()
            .map(|(p, ns)| (p.clone(), ns.to_string()))
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const URI: &str = "file:///w/onto.owl";

    /// `body` in an `rdf:RDF` root with `rdf:` and `ex:` (http://example.org/).
    fn doc(body: &str) -> String {
        format!("<?xml version=\"1.0\"?>\n{}", rooted(body))
    }

    /// [`doc`] without the XML declaration, for a DOCTYPE before it.
    fn rooted(body: &str) -> String {
        format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n{body}\n</rdf:RDF>\n"
        )
    }

    /// The triples of `p` with `from` read as `to`, sorted.
    fn graph(p: &Parsed, from: &str, to: &str) -> Vec<String> {
        let (from, to) = (Iri::intern(from), Iri::intern(to));
        let mut g: Vec<String> = p
            .triples
            .iter()
            .map(|t| format!("{:?}", renamed(t, from, to)))
            .collect();
        g.sort();
        g
    }

    /// `text` (base `base`) after renaming `old` to `new`, checked: the text
    /// parses again without errors to the graph before with `old` read as
    /// `new`, and names `old` nowhere.
    fn renamed_in(text: &str, base: Option<&str>, old: &str, new: &str) -> Result<String, Refusal> {
        let before = grammar::parse(text, base);
        assert!(before.errors.is_empty(), "{:?}", before.errors);
        let edits = plan(text, &before, Iri::intern(old), new)?;
        let out = apply(text, &edits);
        let after = grammar::parse(&out, base);
        assert!(after.errors.is_empty(), "{out}\n{:?}", after.errors);
        assert_eq!(graph(&after, old, new), graph(&before, old, new), "{out}");
        assert!(
            !after
                .triples
                .iter()
                .any(|t| naming(t, Iri::intern(old)).is_some()),
            "{out}"
        );
        Ok(out)
    }

    fn renamed_ok(text: &str, base: Option<&str>, old: &str, new: &str) -> String {
        renamed_in(text, base, old, new).unwrap_or_else(|r| panic!("{r:?}\n{text}"))
    }

    fn refused(text: &str, base: Option<&str>, old: &str, new: &str) -> Refusal {
        match renamed_in(text, base, old, new) {
            Err(r) => r,
            Ok(out) => panic!("renamed:\n{out}"),
        }
    }

    #[test]
    fn a_full_iri_value_is_written_in_full() {
        let text = doc("<rdf:Description rdf:about=\"http://example.org/a\">\n\
             <ex:p rdf:resource=\"http://example.org/a\"/>\n\
             <ex:n rdf:datatype=\"http://example.org/a\">1</ex:n>\n\
             </rdf:Description>");
        let out = renamed_ok(
            &text,
            Some(URI),
            "http://example.org/a",
            "http://example.org/b",
        );
        assert_eq!(out, text.replace("example.org/a\"", "example.org/b\""));
        // Any prefix bound to the RDF namespace.
        let text = format!(
            "<r:RDF xmlns:r=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n\
             <r:Description r:about=\"urn:ex:a\"><ex:p r:resource=\"urn:ex:a\"/></r:Description>\n\
             </r:RDF>\n"
        );
        let out = renamed_ok(&text, None, "urn:ex:a", "urn:ex:z");
        assert_eq!(out, text.replace("urn:ex:a", "urn:ex:z"));
        // The rdf:type and rdf:annotation values are IRI values too.
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\" rdf:type=\"urn:ex:a\"/>");
        let out = renamed_ok(&text, None, "urn:ex:a", "urn:ex:z");
        assert_eq!(out, text.replace("urn:ex:a", "urn:ex:z"));
    }

    #[test]
    fn a_relative_value_stays_relative_under_its_base() {
        // xml:base on the root: `#x` and `x` forms.
        let root = |body: &str| {
            format!(
                "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\" xml:base=\"http://example.org/base/onto\">\n{body}\n</rdf:RDF>\n"
            )
        };
        let text = root("<ex:C rdf:about=\"#a\"/>\n<ex:C rdf:about=\"other\"/>");
        let out = renamed_ok(
            &text,
            Some(URI),
            "http://example.org/base/onto#a",
            "http://example.org/base/onto#b",
        );
        assert_eq!(out, text.replace("\"#a\"", "\"#b\""));
        let out = renamed_ok(
            &text,
            Some(URI),
            "http://example.org/base/other",
            "http://example.org/base/next",
        );
        assert_eq!(out, text.replace("\"other\"", "\"next\""));
        // Not under the same base: in full.
        let out = renamed_ok(
            &text,
            Some(URI),
            "http://example.org/base/onto#a",
            "http://example.org/x#b",
        );
        assert_eq!(out, text.replace("\"#a\"", "\"http://example.org/x#b\""));
        // A nested xml:base.
        let text = root(
            "<rdf:Description xml:base=\"http://example.org/inner/\" rdf:about=\"a\"/>\n<rdf:Description rdf:about=\"a\"/>",
        );
        let out = renamed_ok(
            &text,
            Some(URI),
            "http://example.org/inner/a",
            "http://example.org/inner/b",
        );
        assert_eq!(out, text.replacen("rdf:about=\"a\"", "rdf:about=\"b\"", 1));
        // No base: the document's URI.
        let text = doc("<ex:C rdf:about=\"#a\"/>");
        let out = renamed_ok(&text, Some(URI), &format!("{URI}#a"), &format!("{URI}#b"));
        assert_eq!(out, text.replace("\"#a\"", "\"#b\""));
    }

    #[test]
    fn rdf_id_is_renamed_by_its_fragment_only() {
        let text = doc(
            "<ex:C rdf:ID=\"a\"/>\n<rdf:Description rdf:about=\"#a\"><ex:p rdf:nodeID=\"a\"/></rdf:Description>",
        );
        let (a, b) = (format!("{URI}#a"), format!("{URI}#b"));
        let out = renamed_ok(&text, Some(URI), &a, &b);
        assert_eq!(
            out,
            text.replace("rdf:ID=\"a\"", "rdf:ID=\"b\"")
                .replace("\"#a\"", "\"#b\""),
            "rdf:nodeID names a blank node: it is not edited"
        );
        let r = refused(&text, Some(URI), &a, "http://example.org/b");
        assert_eq!(r.line, 3);
        assert!(r.reason.contains("rdf:ID names an IRI"), "{r:?}");
        // As a reification: the statement and its subject change together.
        let text = doc(
            "<rdf:Description rdf:about=\"urn:ex:s\"><ex:p rdf:ID=\"st\">1</ex:p></rdf:Description>",
        );
        let out = renamed_ok(&text, Some(URI), &format!("{URI}#st"), &format!("{URI}#t2"));
        assert_eq!(out, text.replace("\"st\"", "\"t2\""));
    }

    #[test]
    fn an_element_name_changes_in_both_tags() {
        let text = doc(
            "<ex:A rdf:about=\"urn:ex:s\">\n<ex:p>x</ex:p>\n<ex:p rdf:resource=\"urn:ex:o\"/>\n</ex:A>\n<ex:A/>",
        );
        let out = renamed_ok(&text, None, "http://example.org/A", "http://example.org/B");
        assert_eq!(out, text.replace("ex:A", "ex:B"));
        let out = renamed_ok(&text, None, "http://example.org/p", "http://example.org/q");
        assert_eq!(out, text.replace("ex:p", "ex:q"));
        // Another namespace: its prefix, in scope there.
        let text = doc(
            "<ex:A rdf:about=\"urn:ex:s\" xmlns:o=\"http://example.com/\">\n<ex:p>x</ex:p>\n</ex:A>\n<ex:A/>",
        );
        let out = renamed_ok(&text, None, "http://example.org/p", "http://example.com/q");
        assert_eq!(out, text.replace("ex:p", "o:q"));
        // Out of its scope: refused, at the first use that has none.
        let r = refused(&text, None, "http://example.org/A", "http://example.com/B");
        assert_eq!(r.line, 6, "{r:?}");
        assert!(r.reason.contains("no prefix in scope at ex:A"), "{r:?}");
        assert!(r.reason.contains("adds no xmlns"), "{r:?}");
        // A local part that is no XML name.
        let r = refused(&text, None, "http://example.org/p", "http://example.org/1p");
        assert!(r.reason.contains("\"1p\" is not an XML name"), "{r:?}");
        let r = refused(&text, None, "http://example.org/p", "http://example.org/");
        assert!(r.reason.contains("is not an XML name"), "{r:?}");
    }

    #[test]
    fn a_default_namespace_name_keeps_it() {
        let text = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\">\n<Thing xmlns=\"http://example.org/\" rdf:about=\"urn:ex:s\"><label>x</label></Thing>\n</rdf:RDF>\n"
        );
        let out = renamed_ok(
            &text,
            None,
            "http://example.org/Thing",
            "http://example.org/Item",
        );
        assert_eq!(out, text.replace("Thing", "Item"));
        let out = renamed_ok(
            &text,
            None,
            "http://example.org/label",
            "http://example.org/name",
        );
        assert_eq!(out, text.replace("label>", "name>"));
        let r = refused(
            &text,
            None,
            "http://example.org/label",
            "http://example.com/name",
        );
        assert!(r.reason.contains("no prefix in scope at label"), "{r:?}");
    }

    #[test]
    fn a_property_attribute_name_is_renamed_as_a_name() {
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\" ex:p=\"v\"/>");
        let out = renamed_ok(&text, None, "http://example.org/p", "http://example.org/q");
        assert_eq!(out, text.replace("ex:p=", "ex:q="));
        // An attribute takes no default namespace.
        let text = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns=\"http://example.com/\" xmlns:ex=\"http://example.org/\">\n<rdf:Description rdf:about=\"urn:ex:s\" ex:p=\"v\"/>\n</rdf:RDF>\n"
        );
        let r = refused(&text, None, "http://example.org/p", "http://example.com/q");
        assert!(r.reason.contains("no prefix in scope at ex:p"), "{r:?}");
        // Two attributes with one name after it: refused by the check.
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\" ex:p=\"v\" ex:q=\"w\"/>");
        let r = refused(&text, None, "http://example.org/p", "http://example.org/q");
        assert_eq!(r.line, 3);
    }

    #[test]
    fn namespaces_are_read_in_scope() {
        // A prefix redeclared inside: its names there are another IRI.
        let text = doc(
            "<rdf:Description rdf:about=\"urn:ex:s\"><ex:p>1</ex:p></rdf:Description>\n\
             <rdf:Description rdf:about=\"urn:ex:t\" xmlns:ex=\"http://example.com/\"><ex:p>2</ex:p></rdf:Description>",
        );
        let out = renamed_ok(&text, None, "http://example.org/p", "http://example.org/q");
        assert_eq!(out, text.replacen("ex:p>", "ex:q>", 2));
        // Two prefixes for one namespace: each name keeps its own.
        let text = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:a=\"http://example.org/\" xmlns:b=\"http://example.org/\">\n\
             <rdf:Description rdf:about=\"urn:ex:s\"><a:p>1</a:p><b:p>2</b:p></rdf:Description>\n</rdf:RDF>\n"
        );
        let out = renamed_ok(&text, None, "http://example.org/p", "http://example.org/q");
        assert_eq!(out, text.replace("a:p>", "a:q>").replace("b:p>", "b:q>"));
        // A prefix declared on the element itself is in scope for its name.
        let text = doc(
            "<rdf:Description rdf:about=\"urn:ex:s\"><o:p xmlns:o=\"http://example.com/\">1</o:p><ex:p>2</ex:p></rdf:Description>",
        );
        let r = refused(&text, None, "http://example.org/p", "http://example.com/p");
        assert!(r.reason.contains("no prefix in scope at ex:p"), "{r:?}");
    }

    #[test]
    fn an_entity_value_keeps_its_entity_when_it_can() {
        let text = "<!DOCTYPE rdf:RDF [<!ENTITY ex \"http://example.org/\"> <!ENTITY a \"http://example.org/a\">]>\n"
            .to_string()
            + &rooted("<rdf:Description rdf:about=\"&ex;a\"><ex:p rdf:resource='&ex;a'/></rdf:Description>");
        let out = renamed_ok(&text, None, "http://example.org/a", "http://example.org/b");
        assert_eq!(
            out,
            text.replace("&ex;a\"", "&ex;b\"")
                .replace("&ex;a'", "&ex;b'")
        );
        // Not under the entity's value: in full.
        let out = renamed_ok(&text, None, "http://example.org/a", "urn:ex:it's");
        assert_eq!(
            out,
            text.replace("\"&ex;a\"", "\"urn:ex:it's\"")
                .replace("'&ex;a'", "'urn:ex:it&apos;s'")
        );
        // The whole IRI in the entity: refused, the declaration is not edited.
        let text = text.replace("\"&ex;a\"", "\"&a;\"");
        let r = refused(&text, None, "http://example.org/a", "http://example.org/b");
        assert!(
            r.reason.contains("the whole IRI comes from the entity &a;"),
            "{r:?}"
        );
        assert_eq!(r.line, 3);
    }

    #[test]
    fn a_use_from_an_entity_or_with_character_references_is_refused() {
        // A markup entity's use.
        let text = "<!DOCTYPE r [<!ENTITY m \"<ex:p rdf:resource='urn:ex:o'/>\">]>\n".to_string()
            + &rooted("<rdf:Description rdf:about=\"urn:ex:s\">&m;</rdf:Description>");
        for old in ["urn:ex:o", "http://example.org/p"] {
            let r = refused(&text, None, old, "urn:ex:z");
            assert!(r.reason.contains("replacement text"), "{old}: {r:?}");
            assert_eq!(r.line, 3, "{old}");
        }
        // Character references.
        let text = doc("<rdf:Description rdf:about=\"urn&#58;ex:s\"/>");
        let r = refused(&text, None, "urn:ex:s", "urn:ex:t");
        assert!(r.reason.contains("character references"), "{r:?}");
        // An `&` written as `&amp;` is one too; one written into a value is escaped.
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s?a&amp;b\"/>");
        assert!(
            refused(&text, None, "urn:ex:s?a&b", "urn:ex:t")
                .reason
                .contains("character references")
        );
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\"/>");
        let out = renamed_ok(&text, None, "urn:ex:s", "urn:ex:t?a&b");
        assert!(out.contains("rdf:about=\"urn:ex:t?a&amp;b\""), "{out}");
    }

    #[test]
    fn namespace_values_literals_and_comments_are_never_edited() {
        let text = format!(
            "<!DOCTYPE r [<!ENTITY ns \"http://example.org/\">]>\n\
             <rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\" xml:base=\"http://example.org/\">\n\
             <!-- http://example.org/ -->\n\
             <rdf:Description rdf:about=\"http://example.org/\">\n\
             <ex:label>http://example.org/</ex:label>\n\
             <ex:x rdf:parseType=\"Literal\"><ex:label>http://example.org/</ex:label></ex:x>\n\
             <ex:c><![CDATA[http://example.org/]]></ex:c>\n\
             </rdf:Description>\n</rdf:RDF>\n"
        );
        // The term: only its value is edited.
        let out = renamed_ok(
            &text,
            None,
            "http://example.org/",
            "http://example.org/onto",
        );
        assert_eq!(
            out,
            text.replace(
                "rdf:about=\"http://example.org/\"",
                "rdf:about=\"http://example.org/onto\""
            )
        );
        // Inside an XML literal, element names are not uses.
        let out = renamed_ok(
            &text,
            None,
            "http://example.org/label",
            "http://example.org/name",
        );
        assert_eq!(
            out,
            text.replacen(
                "<ex:label>http://example.org/</ex:label>",
                "<ex:name>http://example.org/</ex:name>",
                1
            )
        );
    }

    #[test]
    fn implied_vocabulary_is_refused() {
        let text = doc(
            "<rdf:Seq rdf:about=\"urn:ex:seq\">\n<rdf:li>a</rdf:li>\n</rdf:Seq>\n\
             <rdf:Description rdf:about=\"urn:ex:s\">\n\
             <ex:list rdf:parseType=\"Collection\"><rdf:Description rdf:about=\"urn:ex:i\"/></ex:list>\n\
             <ex:p rdf:ID=\"st\">1</ex:p>\n\
             </rdf:Description>",
        );
        let cases = [
            ("_1", 4, "rdf:li stands for"),
            ("first", 7, "implies"),
            ("nil", 7, "implies"),
            ("Statement", 8, "implies"),
            ("subject", 8, "implies"),
            ("langString", 4, "implies"),
        ];
        for (local, line, why) in cases {
            let old = if local == "langString" {
                // A plain literal: xsd:string.
                "http://www.w3.org/2001/XMLSchema#string".to_string()
            } else {
                format!("{RDF}{local}")
            };
            let r = refused(&text, Some(URI), &old, "urn:ex:z");
            assert!(r.reason.contains(why), "{local}: {r:?}");
            assert_eq!(r.line, line, "{local}: {r:?}");
        }
        // rdf:type: implied by a typed node element; as an attribute, syntax.
        let text = doc("<ex:C rdf:about=\"urn:ex:s\"/>");
        assert!(
            refused(&text, None, &format!("{RDF}type"), "urn:ex:z")
                .reason
                .contains("implies")
        );
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\" rdf:type=\"urn:ex:C\"/>");
        let r = refused(&text, None, &format!("{RDF}type"), "urn:ex:z");
        assert!(r.reason.contains("rdf:type is RDF/XML syntax"), "{r:?}");
        // An element written as rdf:type is a name like any other.
        let text = doc(
            "<rdf:Description rdf:about=\"urn:ex:s\"><rdf:type rdf:resource=\"urn:ex:C\"/></rdf:Description>",
        );
        let out = renamed_ok(
            &text,
            None,
            &format!("{RDF}type"),
            "http://example.org/kind",
        );
        assert!(out.contains("<ex:kind rdf:resource"), "{out}");
    }

    #[test]
    fn a_name_that_is_rdf_syntax_is_refused_by_the_check() {
        let text = doc("<ex:C rdf:about=\"urn:ex:s\"/>");
        let r = refused(
            &text,
            None,
            "http://example.org/C",
            &format!("{RDF}Description"),
        );
        assert!(r.reason.contains("something else"), "{r:?}");
        let text = doc("<rdf:Description rdf:about=\"urn:ex:s\" ex:p=\"v\"/>");
        let r = refused(&text, None, "http://example.org/p", &format!("{RDF}type"));
        assert_eq!(r.line, 3, "{r:?}");
    }

    #[test]
    fn a_document_with_a_syntax_error_is_not_edited() {
        let text = doc("<ex:C rdf:about=\"urn:ex:s\">\n<ex:p>1</ex:q>\n</ex:C>");
        let p = grammar::parse(&text, None);
        assert!(!p.errors.is_empty());
        let r = plan(&text, &p, Iri::intern("urn:ex:s"), "urn:ex:t").unwrap_err();
        assert!(
            r.reason.starts_with("the document has a syntax error"),
            "{r:?}"
        );
        assert_eq!(r.line, 4);
    }

    /// The line (from 1) of the first syntax error of `text`.
    fn first_error_line(text: &str) -> u32 {
        let p = grammar::parse(text, Some(URI));
        let at = p.errors.iter().map(|e| e.span.start).min().unwrap();
        line(text, at)
    }

    /// A document with a syntax error may hide a use from the grammar (an
    /// element read as something else after an unclosed tag, a declaration
    /// that is not closed, a broken attribute name): when its text names
    /// the old IRI in any spelling, it blocks the rename, at its first
    /// error. One that does not name it is no reason to refuse.
    #[test]
    fn a_broken_document_that_names_the_iri_blocks_a_rename() {
        let foo = "http://example.org/Foo";
        let cases = [
            // A broken attribute name that holds the use.
            (
                format!(
                    "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"{RDF}\">\n  <rdf:Description rdf:about=\"http://example.org/a\" rdf:typ'e=\"{foo}\"/>\n</rdf:RDF>\n"
                ),
                3,
            ),
            // An element not closed before the use.
            (
                doc(&format!(
                    "<rdf:Description rdf:about=\"http://example.org/s\">\n<rdf:Description rdf:about=\"{foo}\"/>"
                )),
                3,
            ),
            (
                doc(&format!(
                    "<rdf:Description rdf:about=\"http://example.org/s\">\n<rdf:Description rdf:about=\"http://example.org/t\"><ex:p rdf:resource=\"{foo}\"/></rdf:Description>"
                )),
                3,
            ),
            // An XML declaration that is not closed.
            (
                format!(
                    "<?xml version=\"1.0\n<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n  <rdf:Description rdf:about=\"{foo}\"/>\n</rdf:RDF>\n"
                ),
                1,
            ),
            // An RDF/XML error: rdf:RDF inside rdf:RDF.
            (
                doc(&format!(
                    "<rdf:RDF>\n<rdf:Description rdf:about=\"{foo}\"/>\n</rdf:RDF>"
                )),
                3,
            ),
            // Only the prefixed name.
            (
                doc("<rdf:Description rdf:about=\"http://example.org/s\">\n<ex:Foo/>"),
                3,
            ),
            // Only an entity spelling, written with a character reference.
            (
                "<!DOCTYPE r [<!ENTITY ex \"http://example.org/\">]>\n".to_string()
                    + &rooted(
                        "<rdf:Description rdf:about=\"http://example.org/s\">\n<ex:p rdf:resource=\"&ex;F&#111;o\"/>",
                    ),
                3,
            ),
        ];
        for (text, line) in &cases {
            let p = grammar::parse(text, Some(URI));
            assert!(!p.errors.is_empty(), "{text}");
            let r = blocked(text, &p, foo).unwrap_or_else(|| panic!("not blocked:\n{text}"));
            assert_eq!(r.line, *line, "{text}\n{r:?}");
            assert_eq!(r.line, first_error_line(text), "{text}");
            assert!(
                r.reason.starts_with("the document has a syntax error ("),
                "{r:?}"
            );
            // Not named there: no reason to refuse.
            assert_eq!(
                blocked(text, &p, "http://example.org/Other"),
                None,
                "{text}"
            );
            // A rename started anywhere in it is refused the same way.
            for offset in [0, text.find(">\n").unwrap() as u32 + 3] {
                match at(text, Some(URI), offset) {
                    Err(got) => assert_eq!(got, r, "{text}"),
                    Ok(_) => panic!("{offset}: not refused:\n{text}"),
                }
            }
        }
        // The local name stands alone: `Foobar` and `xFoo` are not it.
        let text =
            doc("<rdf:Description rdf:about=\"http://example.org/s\">\n<ex:Foobar/><ex:xFoo/>");
        let p = grammar::parse(&text, Some(URI));
        assert_eq!(blocked(&text, &p, foo), None);
        // An IRI with no local name (a namespace): any broken document.
        assert!(blocked(&text, &p, "http://example.org/other/").is_some());
        // A document without an error is not blocked: it is planned.
        let text = doc(&format!("<rdf:Description rdf:about=\"{foo}\"/>"));
        assert_eq!(blocked(&text, &grammar::parse(&text, Some(URI)), foo), None);
        // An OWL/XML text is not read, so it has no RDF/XML use to hide.
        let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"><Declaration><Class IRI=\"http://example.org/Foo\"/></Declaration>\n";
        let p = super::super::parse(text, Some(URI));
        assert!(!p.errors.is_empty());
        assert_eq!(blocked(text, &p, foo), None);
    }

    /// An `ATTLIST` default in the DOCTYPE gives elements attributes this
    /// reader does not add (an `rdf:type`, an `xml:base`): a document that
    /// declares one is not edited, and blocks a rename it names.
    #[test]
    fn attribute_defaults_block_a_rename() {
        let foo = "http://example.org/Foo";
        let text = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [ <!ATTLIST ex:p rdf:type CDATA \"{foo}\"> ]>\n{}",
            rooted(&format!(
                "<rdf:Description rdf:about=\"{foo}\"/>\n<ex:p rdf:about=\"http://example.org/k\"/>"
            ))
        );
        let p = grammar::parse(&text, Some(URI));
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        for old in [foo, "http://example.org/p", "http://example.org/k"] {
            let r = blocked(&text, &p, old).unwrap_or_else(|| panic!("{old}"));
            assert_eq!(r.line, 2, "{old}");
            assert!(r.reason.contains("<!ATTLIST"), "{r:?}");
            assert_eq!(plan(&text, &p, Iri::intern(old), "urn:ex:z"), Err(r));
        }
        assert_eq!(blocked(&text, &p, "urn:ex:other"), None);
        assert!(matches!(at(&text, Some(URI), 0), Err(r) if r.line == 2));
        // Without a default value, an ATTLIST adds nothing.
        for decl in ["#IMPLIED", "#REQUIRED"] {
            let text = text.replace(&format!("CDATA \"{foo}\""), &format!("CDATA {decl}"));
            let p = grammar::parse(&text, Some(URI));
            assert_eq!(blocked(&text, &p, foo), None, "{decl}");
            assert!(
                plan(&text, &p, Iri::intern(foo), "urn:ex:z").is_ok(),
                "{decl}"
            );
        }
    }

    /// Where every rename of the IRI at the cursor is refused, whatever the
    /// new name, a rename is refused where it starts, with the reason of
    /// the first such use of that IRI in the document: `rdf:type` gives
    /// one answer as an element and as an attribute.
    #[test]
    fn a_rename_that_cannot_succeed_is_refused_where_it_starts() {
        let refused_at = |text: &str, needle: &str, plus: usize| {
            let offset = (text.find(needle).unwrap() + plus) as u32;
            match at(text, Some(URI), offset) {
                Err(r) => r,
                Ok(_) => panic!("{needle}: not refused"),
            }
        };
        let text = doc("<rdf:Description rdf:about=\"http://example.org/a\">\n\
             <rdf:type rdf:resource=\"http://example.org/B\"/>\n\
             <ex:p rdf:nodeID=\"b\" rdf:type=\"http://example.org/B\"/>\n\
             </rdf:Description>");
        let element = refused_at(&text, "<rdf:type", 3);
        let attribute = refused_at(&text, "rdf:type=", 3);
        assert_eq!(element, attribute);
        assert_eq!(element.line, 5);
        assert!(
            element.reason.contains("rdf:type is RDF/XML syntax"),
            "{element:?}"
        );
        // Character references, the whole IRI from an entity, a markup entity.
        let text = "<!DOCTYPE r [<!ENTITY a \"http://example.org/a\"> <!ENTITY m \"<ex:q rdf:resource='urn:ex:o'/>\">]>\n"
            .to_string()
            + &rooted(
                "<rdf:Description rdf:about=\"urn&#58;ex:s\">\n<ex:p rdf:resource=\"&a;\"/>&m;</rdf:Description>",
            );
        let r = refused_at(&text, "urn&#58;", 2);
        assert!(r.reason.contains("character references"), "{r:?}");
        let r = refused_at(&text, "&a;", 1);
        assert!(
            r.reason.contains("the whole IRI comes from the entity &a;"),
            "{r:?}"
        );
        let r = refused_at(&text, "&m;", 1);
        assert!(r.reason.contains("replacement text"), "{r:?}");
        // rdf:li stands for rdf:_n.
        let text = doc("<rdf:Seq rdf:about=\"urn:ex:q\"><rdf:li>1</rdf:li></rdf:Seq>");
        let r = refused_at(&text, "rdf:li", 2);
        assert!(r.reason.contains("rdf:li stands for"), "{r:?}");
    }

    /// A parse the workspace holds that is not this text's (a Turtle text
    /// called RDF/XML): its uses are refused, never left out.
    #[test]
    fn a_parse_that_is_not_the_texts_is_refused() {
        let text = doc("<ex:C rdf:about=\"urn:ex:s\"/>");
        let mut p = grammar::parse(&text, None);
        p.triples[0].o = Node::Iri(Iri::intern("urn:ex:other"));
        let r = plan(&text, &p, Iri::intern("urn:ex:s"), "urn:ex:t").unwrap_err();
        assert!(r.reason.contains("something else"), "{r:?}");
    }

    #[test]
    fn a_rename_starts_at_a_written_use() {
        let text = format!(
            "<!DOCTYPE r [<!ENTITY m \"<ex:q rdf:resource='urn:ex:o'/>\">]>\n\
             <rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://example.org/\">\n\
             <ex:C rdf:about=\"urn:ex:s\" ex:p=\"v\"><ex:p>1</ex:p>&m;<rdf:li>2</rdf:li><ex:r rdf:ID=\"i\">3</ex:r></ex:C>\n\
             </rdf:RDF>\n"
        );
        let on = |needle: &str, plus: usize| {
            let offset = (text.find(needle).unwrap() + plus) as u32;
            match at(&text, Some(URI), offset) {
                Ok(Some(At::Use {
                    span,
                    iri,
                    prefixes,
                })) => Some((
                    text[span.start as usize..span.end as usize].to_string(),
                    iri.to_string(),
                    prefixes.get("ex").cloned(),
                )),
                Ok(Some(At::Namespace)) => Some(("xmlns".into(), String::new(), None)),
                Ok(None) => None,
                Err(r) => Some(("refused".into(), r.reason, None)),
            }
        };
        let ex = Some("http://example.org/".to_string());
        assert_eq!(
            on("ex:C", 1),
            Some(("ex:C".into(), "http://example.org/C".into(), ex.clone()))
        );
        assert_eq!(
            on("ex:C", 4),
            Some(("ex:C".into(), "http://example.org/C".into(), ex.clone()))
        );
        assert_eq!(
            on("urn:ex:s", 3),
            Some(("urn:ex:s".into(), "urn:ex:s".into(), ex.clone()))
        );
        assert_eq!(
            on("ex:p=", 0),
            Some(("ex:p".into(), "http://example.org/p".into(), ex.clone()))
        );
        assert_eq!(
            on("\"i\"", 1),
            Some(("i".into(), format!("{URI}#i"), ex.clone()))
        );
        assert_eq!(
            on("<ex:p>", 2),
            Some(("ex:p".into(), "http://example.org/p".into(), ex.clone()))
        );
        assert_eq!(
            on("xmlns:ex", 7),
            Some(("xmlns".into(), String::new(), None))
        );
        // An entity's use and rdf:li: refused there; text and the rdf:RDF
        // name: nothing to rename.
        let refused = |needle: &str, plus: usize| match on(needle, plus) {
            Some((r, reason, None)) if r == "refused" => reason,
            got => panic!("{needle}: {got:?}"),
        };
        assert_eq!(refused("&m;", 1), FROM_ENTITY);
        assert!(refused("rdf:li", 2).starts_with("rdf:li stands for"));
        assert_eq!(on(">1<", 1), None);
        assert_eq!(on("rdf:RDF", 2), None);
    }

    /// A text in an OWL syntax that is not RDF reads as no RDF/XML here
    /// either, though its XML would give names.
    #[test]
    fn an_owl_xml_text_has_no_use() {
        let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"><Declaration><Class IRI=\"#A\"/></Declaration></Ontology>\n";
        assert!(matches!(at(text, Some(URI), 2), Ok(None)));
        let p = super::super::parse(text, Some(URI));
        let owl = Iri::intern("http://www.w3.org/2002/07/owl#Ontology");
        assert_eq!(plan(text, &p, owl, "urn:ex:z"), Ok(Vec::new()));
    }

    #[test]
    fn a_plan_writes_nothing_for_the_same_iri() {
        let text = doc("<ex:C rdf:about=\"urn:ex:s\"/>");
        let p = grammar::parse(&text, None);
        assert_eq!(
            plan(&text, &p, Iri::intern("urn:ex:s"), "urn:ex:s"),
            Ok(Vec::new())
        );
    }
}
