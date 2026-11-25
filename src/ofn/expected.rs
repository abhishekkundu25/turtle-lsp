//! What the grammar expects where the user types: the keywords and the
//! kinds of IRI that may come at an offset of a Functional document, read
//! from the tokens before it, so that a document half typed (with errors,
//! a construct not closed) is read as far as it goes. And what completion
//! needs to add a prefix: the names in scope, and where a new `Prefix( … )`
//! declaration goes.
//!
//! The tokens are read from the end of the last item mapped whole before
//! the offset ([`Parsed::statements`]), or the last prefix declaration,
//! else from the start: the cost is that of the text since then, not of
//! the document. A paren stack holds the constructs open at the offset;
//! each construct's operands so far fill its signature
//! ([`super::signature`]) as the grammar fills it, and the slots that may
//! come next say what is expected. An axiom keyword inside a construct
//! closes what is open, as the grammar's recovery does.

use super::grammar::STRAY_PREFIX;
use super::lexer::{Kind, Lexer, Token};
use super::signature::{Cat, Entity, Rep, is_axiom, keyword_in, signature};
use super::tree::{Keyword, NodeKind};
use super::{Errors, STANDARD};
use crate::syntax::names::newline;
use crate::syntax::{Parsed, PrefixMap, Span};

/// What may come at an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expected {
    /// The word that ends at the offset (what is being typed), to its end.
    pub(crate) word: Span,
    /// The keywords that may start there, in the order of [`Keyword`].
    pub(crate) keywords: Vec<Keyword>,
    pub(crate) iris: Iris,
}

/// Which IRIs may come at an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Iris {
    None,
    /// Any IRI (an annotation's subject or value, an import, a facet).
    Any,
    /// The IRI of an entity of one of these kinds.
    Of(Vec<Entity>),
}

impl Iris {
    fn with(self, other: Iris) -> Iris {
        match (self, other) {
            (Iris::None, x) | (x, Iris::None) => x,
            (Iris::Any, _) | (_, Iris::Any) => Iris::Any,
            (Iris::Of(mut a), Iris::Of(b)) => {
                for e in b {
                    if !a.contains(&e) {
                        a.push(e);
                    }
                }
                Iris::Of(a)
            }
        }
    }
}

/// Whether `c` ends a word: as the lexer splits words.
fn ends_word(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\n' | b'\r' | b'=' | b'(' | b')' | b'<' | b'>' | b'@' | b'^' | b'"' | b'#'
    )
}

/// The word around `offset`: from the start of the word that ends there
/// to the end of the word it is in.
fn word(text: &str, offset: usize) -> Span {
    let b = text.as_bytes();
    let start = b[..offset]
        .iter()
        .rposition(|&c| ends_word(c))
        .map_or(0, |i| i + 1);
    let end = offset
        + b[offset..]
            .iter()
            .position(|&c| ends_word(c))
            .unwrap_or(b.len() - offset);
    Span::new(start as u32, end as u32)
}

/// What an operand read so far is.
#[derive(Clone, Copy)]
enum Arg {
    Node(NodeKind),
    /// Something the grammar would report: it fills a slot all the same.
    Any,
}

/// A construct open at the offset.
enum Frame {
    /// Outside `Ontology( … )`.
    Document,
    Prefix,
    Ontology,
    Construct {
        k: Keyword,
        /// The slot of its signature the next operand is tried at, and how
        /// many operands that slot took.
        slot: usize,
        taken: u32,
    },
    /// The `( … )` of a `HasKey`, of properties of `each`.
    Group(Cat),
    /// A `(` after no keyword, or after a word that is none.
    Unknown,
}

/// Whether `cat`, having taken `taken`, takes `arg`.
fn accepts(cat: Cat, taken: u32, arg: Arg) -> bool {
    let Arg::Node(kind) = arg else {
        return true;
    };
    let iri = matches!(kind, NodeKind::Iri { .. });
    match cat {
        Cat::DataPropertiesThenRange => {
            iri || matches!(kind, NodeKind::Construct(k) if keyword_in(k, Cat::DataRange))
        }
        Cat::Facets if taken.is_multiple_of(2) => iri,
        Cat::Facets => matches!(kind, NodeKind::Literal { .. }),
        Cat::ObjectKeys | Cat::DataKeys => kind == NodeKind::Group,
        _ => super::signature::fits(kind, cat),
    }
}

/// The construct `k`'s state after one more operand, `arg`: the slots it
/// fits in none of are passed, as the grammar would report them.
fn advance(k: Keyword, slot: &mut usize, taken: &mut u32, arg: Arg) {
    let sig = signature(k);
    while let Some(&(cat, rep)) = sig.get(*slot) {
        if accepts(cat, *taken, arg) {
            *taken += 1;
            let done = match (cat, rep) {
                (Cat::Facets, _) => false,
                (Cat::DataPropertiesThenRange, _) => {
                    matches!(arg, Arg::Node(NodeKind::Construct(_)))
                }
                (_, Rep::One | Rep::Optional) => true,
                (_, Rep::AtLeast(_)) => false,
            };
            if done {
                *slot += 1;
                *taken = 0;
            }
            return;
        }
        *slot += 1;
        *taken = 0;
    }
}

/// The keywords and IRIs one slot of `cat` takes, having taken `taken`.
fn slot_expects(cat: Cat, taken: u32, keywords: &mut Vec<Keyword>) -> Iris {
    let iris = match cat {
        Cat::Iri(None) | Cat::AnnotationSubject | Cat::AnnotationValue => Iris::Any,
        Cat::Facets if taken.is_multiple_of(2) => Iris::Any,
        Cat::Facets => Iris::None,
        Cat::DataPropertiesThenRange if taken > 0 => {
            Iris::Of(vec![Entity::DataProperty, Entity::Datatype])
        }
        _ => cat.entity().map_or(Iris::None, |e| Iris::Of(vec![e])),
    };
    let ranges = !(cat == Cat::DataPropertiesThenRange && taken == 0);
    for &k in Keyword::ALL {
        if ranges && keyword_in(k, cat) && !keywords.contains(&k) {
            keywords.push(k);
        }
    }
    iris
}

/// What the construct `k` takes next, at `slot` having taken `taken`: the
/// slot's operands and, while the slots before may be left as they are,
/// the next slot's.
fn construct_expects(k: Keyword, slot: usize, taken: u32) -> (Vec<Keyword>, Iris) {
    let mut keywords = Vec::new();
    let mut iris = Iris::None;
    for (s, &(cat, rep)) in signature(k).iter().enumerate().skip(slot) {
        let t = if s == slot { taken } else { 0 };
        iris = iris.with(slot_expects(cat, t, &mut keywords));
        let may_stop = match (cat, rep) {
            (Cat::Facets, _) => t >= 2 && t.is_multiple_of(2),
            (Cat::DataPropertiesThenRange, _) | (_, Rep::One) => false,
            (_, Rep::Optional) => true,
            (_, Rep::AtLeast(n)) => t >= u32::from(n),
        };
        if !may_stop {
            break;
        }
    }
    keywords.sort_by_key(|k| Keyword::ALL.iter().position(|x| x == k));
    (keywords, iris)
}

/// The keywords that start an item of the ontology.
fn items() -> Vec<Keyword> {
    Keyword::ALL
        .iter()
        .copied()
        .filter(|&k| is_axiom(k) || matches!(k, Keyword::Import | Keyword::Annotation))
        .collect()
}

/// Where the tokens before `before` are read from: the end of the last
/// item mapped whole, or of the last prefix declaration, that ends by
/// then, and whether that is inside `Ontology( … )`.
fn restart(text: &str, parsed: &Parsed, before: u32) -> (u32, bool) {
    let statements = &parsed.statements;
    let item = statements[..statements.partition_point(|s| s.span.end <= before)]
        .iter()
        .rev()
        // The header is not an item: it may end before its `(`.
        .find(|s| !text[s.span.start as usize..].starts_with("Ontology"))
        .map(|s| s.span.end);
    // A declaration among the items is no restart outside the ontology.
    let prefix = parsed
        .prefixes
        .iter()
        .rev()
        .find(|p| p.span.end <= before && !stray(parsed, p.span.start))
        .map(|p| p.span.end);
    match (item, prefix) {
        (Some(i), Some(p)) if p > i => (p, false),
        (Some(i), _) => (i, true),
        (None, Some(p)) => (p, false),
        (None, None) => (0, false),
    }
}

/// Whether the declaration that starts at `start` is one the grammar read
/// among the items (and reported there).
fn stray(parsed: &Parsed, start: u32) -> bool {
    let errors = &parsed.errors;
    errors[errors.partition_point(|e| e.span.start < start)..]
        .iter()
        .take_while(|e| e.span.start == start)
        .any(|e| e.message() == STRAY_PREFIX)
}

/// What may come at `offset` of `text`, whose parse is `parsed`. `None`
/// where nothing is typed: in a string, a comment, a full IRI, a language
/// tag or a node ID, inside a `Prefix( … )`, after a keyword that waits
/// for its `(`.
pub(crate) fn at(text: &str, parsed: &Parsed, offset: u32) -> Option<Expected> {
    let offset = (offset as usize).min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let word = word(text, offset);
    if text[word.start as usize..offset].starts_with("_:") {
        return None;
    }
    let (from, in_ontology) = restart(text, parsed, word.start);
    let mut lexer = Lexer::new(text);
    lexer.seek(from as usize);
    let mut errors = Errors::default();
    let mut stack = vec![Frame::Document];
    if in_ontology {
        stack.push(Frame::Ontology);
    }
    // A keyword read, waiting for its `(`.
    let mut keyword: Option<Option<Keyword>> = None;
    // After a literal's string, and after its `^^`.
    let (mut literal, mut carets) = (false, false);
    loop {
        let t: Token = lexer.next(&mut errors);
        if t.kind == Kind::Eof || t.start >= word.start {
            break;
        }
        if t.end > word.start {
            // The word is inside this token.
            return None;
        }
        let waiting = keyword.take();
        let was_literal = std::mem::take(&mut literal);
        if std::mem::take(&mut carets) && matches!(t.kind, Kind::FullIri | Kind::PName) {
            continue;
        }
        let top = stack.last_mut().expect("the document");
        let mut arg = None;
        match t.kind {
            Kind::Keyword => {
                keyword = Some(Keyword::of(&text[t.start as usize..t.end as usize]));
            }
            Kind::LParen => match waiting {
                Some(Some(Keyword::Prefix)) => stack.push(Frame::Prefix),
                Some(Some(Keyword::Ontology)) => {
                    stack.truncate(1);
                    stack.push(Frame::Ontology);
                }
                Some(Some(k)) => {
                    // An item starts: what is open was broken off.
                    if is_axiom(k) || k == Keyword::Import {
                        while matches!(
                            stack.last(),
                            Some(Frame::Construct { .. } | Frame::Group(_) | Frame::Unknown)
                        ) {
                            stack.pop();
                        }
                    }
                    stack.push(Frame::Construct {
                        k,
                        slot: 0,
                        taken: 0,
                    });
                }
                Some(None) => stack.push(Frame::Unknown),
                None => {
                    let keys = match top {
                        Frame::Construct { k, slot, .. } => signature(*k)
                            .get(*slot)
                            .map(|s| s.0)
                            .filter(|c| matches!(c, Cat::ObjectKeys | Cat::DataKeys)),
                        _ => None,
                    };
                    stack.push(match keys {
                        Some(Cat::ObjectKeys) => Frame::Group(Cat::ObjectProperty),
                        Some(_) => Frame::Group(Cat::DataProperty),
                        None => Frame::Unknown,
                    });
                }
            },
            Kind::RParen => {
                if stack.len() > 1 {
                    arg = match stack.pop() {
                        Some(Frame::Construct { k, .. }) => Some(Arg::Node(NodeKind::Construct(k))),
                        Some(Frame::Group(_)) => Some(Arg::Node(NodeKind::Group)),
                        Some(Frame::Unknown) => Some(Arg::Any),
                        _ => None,
                    };
                }
            }
            Kind::FullIri | Kind::PName => {
                arg = Some(Arg::Node(NodeKind::Iri { prefix_len: None }))
            }
            Kind::NodeId => arg = Some(Arg::Node(NodeKind::NodeId)),
            Kind::Integer => arg = Some(Arg::Node(NodeKind::Integer)),
            Kind::String => {
                literal = true;
                arg = Some(Arg::Node(NodeKind::Literal { string_end: 0 }));
            }
            Kind::LangTag => {}
            Kind::Carets => carets = was_literal,
            Kind::Bad => arg = Some(Arg::Any),
            Kind::Equals | Kind::Word | Kind::Eof => {}
        }
        if let Some(arg) = arg
            && let Some(Frame::Construct { k, slot, taken }) = stack.last_mut()
        {
            advance(*k, slot, taken, arg);
        }
    }
    // A comment the word is in: the last that starts before it (the lexer
    // reads every comment line before the next token).
    let comments = &lexer.comments;
    if comments[..comments.partition_point(|c| c.start < word.start)]
        .last()
        .is_some_and(|c| word.start <= c.end)
    {
        return None;
    }
    if keyword.is_some() {
        return None;
    }
    let (keywords, iris) = if carets {
        (Vec::new(), Iris::Of(vec![Entity::Datatype]))
    } else {
        match stack.last().expect("the document") {
            Frame::Document => (vec![Keyword::Prefix, Keyword::Ontology], Iris::None),
            Frame::Ontology => (items(), Iris::None),
            Frame::Construct { k, slot, taken } => construct_expects(*k, *slot, *taken),
            Frame::Group(each) => {
                let mut keywords = Vec::new();
                let iris = slot_expects(*each, 0, &mut keywords);
                (keywords, iris)
            }
            Frame::Prefix | Frame::Unknown => return None,
        }
    };
    Some(Expected {
        word,
        keywords,
        iris,
    })
}

/// The operands of `k` that a call of it is written with, by name: its
/// signature's slots without its annotations and optional ones, a slot of
/// at least n operands n times.
pub(crate) fn operands(k: Keyword) -> Vec<&'static str> {
    let mut out = Vec::new();
    for &(cat, rep) in signature(k) {
        let n = match rep {
            Rep::One => 1,
            Rep::Optional => 0,
            Rep::AtLeast(n) => n,
        };
        let names: &[&str] = match cat {
            Cat::Annotation => &[],
            Cat::Iri(Some(e)) => &[entity_name(e)],
            Cat::Iri(None) => &["iri"],
            Cat::DataProperty => &["dataProperty"],
            Cat::ClassExpression => &["class"],
            Cat::ObjectProperty | Cat::SubObjectProperty => &["objectProperty"],
            Cat::DataRange => &["dataRange"],
            Cat::Individual | Cat::IArg => &["individual"],
            Cat::Literal => &["literal"],
            Cat::AnnotationSubject => &["subject"],
            Cat::AnnotationValue | Cat::DArg => &["value"],
            Cat::Integer => &["n"],
            Cat::Entity => &["entity"],
            Cat::ObjectKeys => &["(objectProperties)"],
            Cat::DataKeys => &["(dataProperties)"],
            Cat::DataPropertiesThenRange => &["dataProperty", "dataRange"],
            Cat::Facets => &["facet", "value"],
            Cat::Body => &["Body()"],
            Cat::Head => &["Head()"],
            Cat::Atom => &["atom"],
        };
        for _ in 0..n {
            out.extend_from_slice(names);
        }
    }
    out
}

fn entity_name(e: Entity) -> &'static str {
    match e {
        Entity::Class => "class",
        Entity::Datatype => "datatype",
        Entity::ObjectProperty => "objectProperty",
        Entity::DataProperty => "dataProperty",
        Entity::AnnotationProperty => "annotationProperty",
        Entity::NamedIndividual => "individual",
    }
}

/// The prefixes every IRI of a document of `parsed` is written with: the
/// names of Table 2, then its declarations (the first of a name binds it).
pub(crate) fn prefixes(parsed: &Parsed) -> PrefixMap {
    let mut out: PrefixMap = STANDARD
        .iter()
        .map(|&(p, ns)| (p.to_string(), ns.to_string()))
        .collect();
    let mut declared = rustc_hash::FxHashSet::default();
    for p in &parsed.prefixes {
        if declared.insert(p.name.as_str()) {
            out.insert(p.name.clone(), p.iri.to_string());
        }
    }
    out
}

/// The prefix declarations at the start of a document.
pub(crate) struct Header {
    /// Every name a `Prefix( … )` before the first other keyword declares,
    /// its IRI valid or not.
    pub(crate) names: Vec<String>,
    /// The end of the last declaration the grammar read before the items,
    /// if there is one.
    end: Option<u32>,
    /// The start of the first token, or the end of the text.
    first: u32,
}

impl Header {
    /// The declarations of `text`, whose parse is `parsed`: where the
    /// grammar read them (a declaration among the items, which it reports,
    /// is not one of them), and the names written before the first other
    /// keyword.
    pub(crate) fn of(text: &str, parsed: &Parsed) -> Header {
        let end = parsed
            .prefixes
            .iter()
            .rev()
            .find(|p| !stray(parsed, p.span.start))
            .map(|p| p.span.end);
        let mut lexer = Lexer::new(text);
        let mut errors = Errors::default();
        let mut t = lexer.next(&mut errors);
        let mut header = Header {
            names: Vec::new(),
            end,
            first: t.start,
        };
        let word = |t: Token| &text[t.start as usize..t.end as usize];
        while t.kind == Kind::Keyword && word(t) == "Prefix" {
            let mut depth = 0u32;
            loop {
                t = lexer.next(&mut errors);
                match t.kind {
                    Kind::LParen => depth += 1,
                    Kind::PName if depth == 1 => {
                        header.names.push(word(t).trim_end_matches(':').to_string());
                    }
                    Kind::RParen => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            t = lexer.next(&mut errors);
                            break;
                        }
                    }
                    Kind::Eof => break,
                    // An item: the declaration is broken off.
                    Kind::Keyword if depth <= 1 && word(t) != "Prefix" => break,
                    _ => {}
                }
            }
            if t.kind == Kind::Eof {
                break;
            }
        }
        header
    }

    /// The insertion that declares `name` as `ns` after the declarations,
    /// on a line of its own: at the start of the line after the last one
    /// when nothing but a comment follows it there, else right after it;
    /// with none, before the first token.
    pub(crate) fn declare(&self, text: &str, name: &str, ns: &str) -> (u32, String) {
        let nl = newline(text);
        let line = format!("Prefix({name}:=<{ns}>)");
        let Some(end) = self.end else {
            return (self.first, format!("{line}{nl}"));
        };
        let rest = &text[end as usize..];
        let eol = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let tail = rest[..eol].trim_start();
        if !(tail.is_empty() || tail.starts_with('#')) {
            return (end, format!("{nl}{line}"));
        }
        let after = &rest[eol..];
        if after.is_empty() {
            return (text.len() as u32, format!("{nl}{line}"));
        }
        let skip = if after.starts_with("\r\n") { 2 } else { 1 };
        (end + (eol + skip) as u32, format!("{line}{nl}"))
    }
}
