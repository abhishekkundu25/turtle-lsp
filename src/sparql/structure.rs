//! The structure of a SPARQL document, built from its tokens by a tolerant
//! recursive descent: it exists for broken queries too (an unclosed group
//! runs to the end of the text, a token nothing expects is skipped). See the
//! [module documentation](super) for what each part means.
//!
//! Every IRI, prefixed name and variable token becomes a [`TermRef`] or a
//! [`VarOcc`], including the ones in parts the walk skips (with
//! [`TermRole::Other`] / [`VarRole::Expression`]), so the prefix lints and
//! rename never miss one.

use super::lexer;
use super::{Token, TokenKind};
use crate::syntax::{PrefixMap, Span};
use rustc_hash::{FxHashMap, FxHashSet};

pub(crate) type GroupId = u32;
pub(crate) type ScopeId = u32;
pub(crate) type SelectId = u32;
pub(crate) type VarId = u32;

/// `PREFIX name: <iri>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrefixDecl {
    /// The prefix name without the colon (`""` for `PREFIX : <...>`).
    pub(crate) name: String,
    /// The IRI as written between `<` and `>`.
    pub(crate) iri: String,
    /// The IRI resolved against the base in effect, if it resolves.
    pub(crate) resolved: Option<String>,
    /// From `PREFIX` to `>`.
    pub(crate) span: Span,
    /// `name:`, colon included.
    pub(crate) name_span: Span,
    /// `<iri>`, brackets included.
    pub(crate) iri_span: Span,
}

/// `BASE <iri>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BaseDecl {
    pub(crate) iri: String,
    pub(crate) span: Span,
    pub(crate) iri_span: Span,
}

/// `VERSION "1.2"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VersionDecl {
    pub(crate) span: Span,
    /// The string token, quotes included.
    pub(crate) value_span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationKind {
    Select,
    Construct,
    Describe,
    Ask,
    Load,
    Clear,
    Drop,
    Create,
    Add,
    Move,
    Copy,
    InsertData,
    DeleteData,
    DeleteWhere,
    /// `WITH` / `DELETE { }` / `INSERT { }` ... `WHERE { }`.
    Modify,
}

/// A query (a document holds one) or one update operation.
#[derive(Clone, Debug)]
pub(crate) struct Operation {
    pub(crate) kind: OperationKind,
    /// The first keyword (`SELECT`, `INSERT` of `INSERT DATA`, `WITH`, ...).
    pub(crate) keyword: Span,
    /// From the keyword to the last token before the `;` that ends it (or the
    /// end of the document).
    pub(crate) span: Span,
    pub(crate) clauses: Vec<Clause>,
    /// The operation's top scope.
    pub(crate) scope: ScopeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClauseKind {
    /// `SELECT ...`, `CONSTRUCT { }`, `DESCRIBE ...`, `ASK`: the form and its
    /// projection or template.
    Form,
    /// `FROM` / `FROM NAMED`.
    Dataset,
    Where,
    GroupBy,
    Having,
    OrderBy,
    Limit,
    Offset,
    /// A trailing `VALUES` block.
    Values,
    With,
    Delete,
    Insert,
    Using,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Clause {
    pub(crate) kind: ClauseKind,
    /// The first keyword of the clause (`GROUP` of `GROUP BY`).
    pub(crate) keyword: Span,
    /// From the keyword to the last token of the clause.
    pub(crate) span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupKind {
    /// The `WHERE` clause's group (also `DELETE WHERE { }`, and the group of
    /// a query that leaves out the `WHERE` keyword).
    Where,
    /// A nested `{ }`.
    Group,
    /// The group after `UNION`.
    Union,
    Optional,
    Minus,
    Graph,
    Service,
    Lateral,
    /// The group of `EXISTS` / `NOT EXISTS`.
    Exists,
    /// `{ SELECT ... }`.
    Subquery,
    /// The template of `CONSTRUCT`, `INSERT` or `DELETE`.
    Template,
    /// `INSERT DATA { }` / `DELETE DATA { }`.
    Data,
    /// The rows of `VALUES ... { }`.
    Values,
}

/// A `{ }` pair. `{| |}` annotation blocks are not groups.
#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub(crate) kind: GroupKind,
    /// The keyword that introduced it (`OPTIONAL`, `GRAPH`, `WHERE`, `UNION`,
    /// `VALUES`, `DATA`, ...), if any.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
    pub(crate) keyword: Option<Span>,
    /// The `{` token.
    pub(crate) open: Span,
    /// The `}` token; `None` when the group is not closed.
    pub(crate) close: Option<Span>,
    /// From `{` to `}` (or to the last token inside when unclosed).
    pub(crate) span: Span,
    /// The innermost group around it.
    pub(crate) parent: Option<GroupId>,
    /// Number of groups around it (0 for an outermost group).
    pub(crate) depth: u32,
    /// The scope of what it holds directly (a subquery's group is in the
    /// subquery's own scope).
    pub(crate) scope: ScopeId,
}

/// A variable scope: an operation's top scope or a subquery's. Scope 0 is
/// the first operation's, and also holds what is written before it.
#[derive(Clone, Debug)]
pub(crate) struct Scope {
    /// The enclosing scope of a subquery; `None` for an operation's.
    pub(crate) parent: Option<ScopeId>,
    /// The `SELECT` that makes it (top-level or subquery), if any.
    pub(crate) select: Option<SelectId>,
}

/// A projected variable: `?v`, or `(expression AS ?v)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Projected {
    /// The variable (in [`Structure::vars`]).
    pub(crate) var: VarId,
    /// The whole `( ... AS ?v )` when projected through an expression.
    pub(crate) expr: Option<Span>,
}

/// A `SELECT`: the query's or a subquery's.
#[derive(Clone, Debug)]
pub(crate) struct Select {
    /// `DISTINCT` / `REDUCED`.
    pub(crate) modifier: Option<Span>,
    /// `*`.
    pub(crate) star: Option<Span>,
    pub(crate) items: Vec<Projected>,
    /// From `SELECT` to the end of the projection.
    pub(crate) span: Span,
    /// The scope it makes.
    pub(crate) scope: ScopeId,
    /// Its `WHERE` group.
    pub(crate) where_group: Option<GroupId>,
    /// The `{ }` of a subquery; `None` for the query's own `SELECT`.
    pub(crate) group: Option<GroupId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Position {
    Subject,
    Predicate,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VarRole {
    /// In a triple pattern or template (subject, predicate incl. a reifier's
    /// annotations, object; a reifier `~ ?r` is a subject).
    Pattern(Position),
    /// The name of `GRAPH ?g` or `SERVICE ?s`.
    Graph,
    /// Listed in a `SELECT` (or `DESCRIBE`) projection.
    Projected,
    /// Assigned with `AS` (in a projection, `BIND` or `GROUP BY`).
    Assigned,
    /// Listed in a `VALUES` header.
    Values,
    /// Read in an expression (`FILTER`, `ORDER BY`, a projection expression,
    /// a triple term inside an expression, ...), or in a part of a broken
    /// document the walk skipped.
    Expression,
}

impl VarRole {
    /// Whether the occurrence binds the variable when it is in a `WHERE`.
    pub(crate) fn binds(self) -> bool {
        matches!(
            self,
            VarRole::Pattern(_) | VarRole::Graph | VarRole::Assigned | VarRole::Values
        )
    }
}

/// One occurrence of a variable.
#[derive(Clone, Debug)]
pub(crate) struct VarOcc {
    /// Sigil (`?` or `$`) included.
    pub(crate) span: Span,
    /// The name without its sigil.
    pub(crate) name: String,
    /// The scope the variable belongs to: the innermost subquery (or the
    /// operation) it is in, moved out to the enclosing scope for each
    /// subquery that projects it. Two occurrences with the same name and
    /// scope are the same variable.
    pub(crate) scope: ScopeId,
    pub(crate) role: VarRole,
    /// The innermost group it is in.
    pub(crate) group: Option<GroupId>,
    /// The innermost subquery (or operation) scope it is written in, before
    /// projection moves it out.
    pub(crate) raw_scope: ScopeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    /// `<...>`.
    Iri,
    /// `prefix:local`; `prefix_len` is the byte length of `prefix` (the
    /// colon follows it).
    PName { prefix_len: u32 },
    /// The `a` keyword (`rdf:type`).
    A,
}

/// Where a prefixed name's prefix comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrefixSource {
    /// A `PREFIX` line of the document (index in [`Structure::prefixes`]).
    Declared(u32),
    /// The `prefixes` given to [`Document::parse`](super::Document::parse)
    /// (project and setting prefixes).
    External,
    /// Nowhere: reported as `undeclared-prefix`.
    Undeclared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TermRole {
    Subject,
    /// A predicate, or any IRI inside a property path.
    Predicate,
    Object,
    /// A graph or dataset IRI: `GRAPH`, `FROM`, `USING`, `WITH`, `INTO`,
    /// `SERVICE`, `LOAD`, `CLEAR`, `ADD` / `COPY` / `MOVE` ...
    Graph,
    /// After `^^`.
    Datatype,
    /// A function call's name (`ex:fn(...)`).
    Function,
    /// A value in `VALUES`, an expression, a `DESCRIBE` list.
    Value,
    /// In a part of a broken document the walk skipped.
    Other,
}

/// One occurrence of an IRI: `<...>`, a prefixed name or `a`.
#[derive(Clone, Debug)]
pub(crate) struct TermRef {
    pub(crate) span: Span,
    pub(crate) form: Form,
    /// The IRI it stands for (resolved against the base, escapes undone), or
    /// `None` when its prefix is undeclared or a relative IRI has no base.
    pub(crate) iri: Option<String>,
    pub(crate) role: TermRole,
    /// Inside a triple term `<<( )>>` or a reified triple `<< >>`.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
    pub(crate) quoted: bool,
    /// For a prefixed name: where its prefix is declared.
    pub(crate) prefix: Option<PrefixSource>,
    /// For a predicate or object: the span of its triple's subject term.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
    pub(crate) subject: Option<Span>,
}

impl TermRef {
    /// `prefix:` of a prefixed name, colon included.
    pub(crate) fn prefix_span(&self) -> Option<Span> {
        match self.form {
            Form::PName { prefix_len } => {
                Some(Span::new(self.span.start, self.span.start + prefix_len + 1))
            }
            _ => None,
        }
    }
}

/// One triple of a pattern or template, as far as it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TriplePattern {
    /// The subject term (a whole `[ ]`, `( )` or `<< >>` included).
    pub(crate) subject: Span,
    /// The verb (a whole property path), if written.
    pub(crate) predicate: Option<Span>,
    /// The object term, if written.
    pub(crate) object: Option<Span>,
    pub(crate) group: Option<GroupId>,
    /// Inside a triple term or reified triple.
    pub(crate) quoted: bool,
}

/// What [`build`] finds. See the module documentation.
#[derive(Clone, Debug, Default)]
pub(crate) struct Structure {
    /// An update (by the first query or update keyword), else a query.
    pub(crate) is_update: bool,
    pub(crate) prefixes: Vec<PrefixDecl>,
    pub(crate) bases: Vec<BaseDecl>,
    pub(crate) versions: Vec<VersionDecl>,
    pub(crate) operations: Vec<Operation>,
    pub(crate) groups: Vec<Group>,
    pub(crate) scopes: Vec<Scope>,
    pub(crate) selects: Vec<Select>,
    pub(crate) vars: Vec<VarOcc>,
    pub(crate) refs: Vec<TermRef>,
    pub(crate) triples: Vec<TriplePattern>,
    /// Errors the grammar catches but `spargebra` does not (a triple term or
    /// literal as a triple term's subject, an aggregate in an aggregate).
    pub(crate) errors: Vec<(Span, String)>,
}

impl Structure {
    /// The innermost group whose span contains `offset`.
    #[cfg(test)]
    pub(crate) fn group_at(&self, offset: u32) -> Option<GroupId> {
        // Groups are in order of their `{`; an inner group comes after its
        // parent, so the last containing one is the innermost.
        let end = self.groups.partition_point(|g| g.open.start <= offset);
        (0..end)
            .rev()
            .find(|&i| {
                let s = self.groups[i].span;
                s.start <= offset && offset <= s.end
            })
            .map(|i| i as GroupId)
    }

    /// The occurrences of the variable at `offset` (the one whose span
    /// contains it) in its scope, in text order.
    pub(crate) fn var_occurrences(&self, offset: u32) -> Vec<&VarOcc> {
        let Some(v) = self.vars.iter().find(|v| v.span.contains(offset)) else {
            return Vec::new();
        };
        self.vars
            .iter()
            .filter(|o| o.scope == v.scope && o.name == v.name)
            .collect()
    }

    /// Whether `select` projects variable `name` (`*` projects every one).
    pub(crate) fn projects(&self, select: SelectId, name: &str) -> bool {
        let s = &self.selects[select as usize];
        s.star.is_some()
            || s.items
                .iter()
                .any(|p| self.vars[p.var as usize].name == name)
    }
}

/// Recursion limit: deeper nesting is skipped token by token.
const MAX_DEPTH: u32 = 128;

/// Where a term is, for the roles its IRIs and variables get.
#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    S,
    O,
    /// In `VALUES` rows.
    Values,
    /// In an expression.
    Expr,
}

struct B<'a> {
    src: &'a str,
    toks: &'a [Token],
    /// Indices of the tokens that are not comments.
    sig: Vec<u32>,
    pos: usize,
    s: Structure,
    /// Prefixes in effect: name -> (namespace, source).
    ns: FxHashMap<String, (Option<String>, PrefixSource)>,
    base: Option<oxiri::Iri<String>>,
    scope: ScopeId,
    group: Option<GroupId>,
    depth: u32,
    quoted: u32,
    /// Inside an expression or `VALUES`: variables are read, not bound.
    in_expr: u32,
    /// Inside an aggregate's parentheses.
    aggregate: u32,
    /// Projected names per select, for resolving scopes.
    projected: Vec<FxHashSet<String>>,
    /// End of the last token consumed.
    last_end: u32,
    /// An operation has started (the first one takes scope 0).
    started: bool,
}

/// Builds the structure of `src` from its `tokens`, with the `prefixes` it
/// does not declare and the base it is resolved against.
pub(crate) fn build(
    src: &str,
    tokens: &[Token],
    prefixes: &PrefixMap,
    base: Option<&str>,
) -> Structure {
    let sig = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind != TokenKind::Comment)
        .map(|(i, _)| i as u32)
        .collect();
    let ns = prefixes
        .iter()
        .map(|(k, v)| (k.clone(), (Some(v.clone()), PrefixSource::External)))
        .collect();
    let mut b = B {
        src,
        toks: tokens,
        sig,
        pos: 0,
        s: Structure::default(),
        ns,
        base: base.and_then(|b| oxiri::Iri::parse(b.to_owned()).ok()),
        scope: 0,
        group: None,
        depth: 0,
        quoted: 0,
        in_expr: 0,
        aggregate: 0,
        projected: Vec::new(),
        last_end: 0,
        started: false,
    };
    // Scope 0: the first operation's, and that of anything before it.
    b.s.scopes.push(Scope {
        parent: None,
        select: None,
    });
    b.document();
    b.resolve_scopes();
    b.s
}

impl<'a> B<'a> {
    // --- tokens ------------------------------------------------------------

    fn tok(&self, n: usize) -> Option<&'a Token> {
        let toks = self.toks;
        self.sig.get(self.pos + n).map(|&i| &toks[i as usize])
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tok(0)
    }

    fn text(&self, t: &Token) -> &'a str {
        let src = self.src;
        &src[t.span.start as usize..t.span.end as usize]
    }

    fn is_kw_at(&self, n: usize, kw: &str) -> bool {
        self.tok(n)
            .is_some_and(|t| t.kind == TokenKind::Keyword && self.text(t).eq_ignore_ascii_case(kw))
    }

    fn is_kw(&self, kw: &str) -> bool {
        self.is_kw_at(0, kw)
    }

    fn is_punct_at(&self, n: usize, p: &str) -> bool {
        self.tok(n)
            .is_some_and(|t| t.kind == TokenKind::Punct && self.text(t) == p)
    }

    fn is_punct(&self, p: &str) -> bool {
        self.is_punct_at(0, p)
    }

    fn kind(&self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    /// The upper-case keyword at the current token, if it is one.
    fn keyword(&self) -> Option<&'static str> {
        let t = self.peek()?;
        (t.kind == TokenKind::Keyword)
            .then(|| lexer::keyword(self.text(t)))
            .flatten()
    }

    fn bump(&mut self) -> Option<&'a Token> {
        let t = self.peek()?;
        self.pos += 1;
        self.last_end = t.span.end;
        Some(t)
    }

    /// Consumes `p` if it is next.
    fn eat(&mut self, p: &str) -> Option<Span> {
        if self.is_punct(p) {
            self.bump().map(|t| t.span)
        } else {
            None
        }
    }

    fn eat_kw(&mut self, kw: &str) -> Option<Span> {
        if self.is_kw(kw) {
            self.bump().map(|t| t.span)
        } else {
            None
        }
    }

    /// Skips one token that nothing expects, recording what it names.
    fn skip(&mut self) {
        let Some(t) = self.peek() else { return };
        match t.kind {
            TokenKind::Keyword if matches!(self.keyword(), Some("PREFIX" | "BASE" | "VERSION")) => {
                self.prologue();
            }
            TokenKind::Iri | TokenKind::PName | TokenKind::A => {
                self.bump();
                self.add_ref(t, TermRole::Other, None);
            }
            TokenKind::Var => {
                self.bump();
                self.add_var(t, VarRole::Expression);
            }
            _ => {
                self.bump();
            }
        }
    }

    // --- recording -----------------------------------------------------------

    fn add_var(&mut self, t: &Token, role: VarRole) -> VarId {
        let role = if self.in_expr > 0 && matches!(role, VarRole::Pattern(_)) {
            VarRole::Expression
        } else {
            role
        };
        let name = self.text(t)[1..].to_owned();
        self.s.vars.push(VarOcc {
            span: t.span,
            name,
            scope: self.scope,
            role,
            group: self.group,
            raw_scope: self.scope,
        });
        (self.s.vars.len() - 1) as VarId
    }

    fn add_ref(&mut self, t: &Token, role: TermRole, subject: Option<Span>) {
        let text = self.text(t);
        let (form, iri, prefix) = match t.kind {
            TokenKind::A => (Form::A, Some(crate::syntax::RDF_TYPE.to_owned()), None),
            TokenKind::Iri => (Form::Iri, self.resolve(&text[1..text.len() - 1]), None),
            _ => {
                let colon = text.find(':').unwrap_or(text.len());
                let (name, local) = (&text[..colon], text.get(colon + 1..).unwrap_or(""));
                let (iri, source) = match self.ns.get(name) {
                    Some((ns, source)) => (
                        ns.as_ref()
                            .map(|ns| format!("{ns}{}", local.replace('\\', ""))),
                        *source,
                    ),
                    None => (None, PrefixSource::Undeclared),
                };
                (
                    Form::PName {
                        prefix_len: colon as u32,
                    },
                    iri,
                    Some(source),
                )
            }
        };
        self.s.refs.push(TermRef {
            span: t.span,
            form,
            iri,
            role,
            quoted: self.quoted > 0,
            prefix,
            subject,
        });
    }

    /// An IRI reference's text resolved against the base, escapes undone.
    fn resolve(&self, raw: &str) -> Option<String> {
        let raw = unescape_uchar(raw);
        match &self.base {
            Some(b) => {
                let r = oxiri::IriRef::parse(&*raw).ok()?;
                b.resolve(&r).ok().map(|i| i.into_inner())
            }
            None => oxiri::Iri::parse(raw.into_owned())
                .ok()
                .map(|i| i.into_inner()),
        }
    }

    fn error(&mut self, span: Span, msg: &str) {
        self.s.errors.push((span, msg.to_owned()));
    }

    // --- document ------------------------------------------------------------

    fn document(&mut self) {
        let mut decided = false;
        let mut junk = false;
        loop {
            self.prologue();
            if self.peek().is_none() {
                break;
            }
            match self.keyword() {
                Some(kw @ ("SELECT" | "CONSTRUCT" | "DESCRIBE" | "ASK")) => {
                    if !decided {
                        decided = true;
                        self.s.is_update = false;
                    }
                    self.query(kw);
                }
                Some(
                    "LOAD" | "CLEAR" | "DROP" | "CREATE" | "ADD" | "MOVE" | "COPY" | "INSERT"
                    | "DELETE" | "WITH",
                ) => {
                    if !decided {
                        decided = true;
                        self.s.is_update = true;
                    }
                    self.update();
                }
                _ => {
                    junk = true;
                    self.skip();
                }
            }
        }
        if !decided {
            // No operation at all: an empty update (a prologue alone is one),
            // unless something else is written.
            self.s.is_update = !junk;
        }
    }

    /// `BASE`, `PREFIX` and `VERSION` declarations at the current position.
    fn prologue(&mut self) {
        loop {
            match self.keyword() {
                Some("BASE") => {
                    let kw = self.bump().unwrap().span;
                    if self.kind() == Some(TokenKind::Iri) {
                        let t = self.bump().unwrap();
                        let text = self.text(t);
                        let raw = &text[1..text.len() - 1];
                        if let Some(r) = self.resolve(raw)
                            && let Ok(i) = oxiri::Iri::parse(r)
                        {
                            self.base = Some(i);
                        }
                        self.s.bases.push(BaseDecl {
                            iri: raw.to_owned(),
                            span: Span::new(kw.start, t.span.end),
                            iri_span: t.span,
                        });
                    }
                }
                Some("PREFIX") => {
                    let kw = self.bump().unwrap().span;
                    let Some(n) = self
                        .peek()
                        .filter(|t| t.kind == TokenKind::PName && self.text(t).ends_with(':'))
                    else {
                        continue;
                    };
                    let name_text = self.text(n);
                    // `PREFIX a:b: <...>` is not a declaration.
                    if name_text[..name_text.len() - 1].contains(':') {
                        continue;
                    }
                    self.bump();
                    if self.kind() != Some(TokenKind::Iri) {
                        continue;
                    }
                    let t = self.bump().unwrap();
                    let text = self.text(t);
                    let raw = &text[1..text.len() - 1];
                    let name = name_text[..name_text.len() - 1].to_owned();
                    let resolved = self.resolve(raw);
                    let idx = self.s.prefixes.len() as u32;
                    self.ns.insert(
                        name.clone(),
                        (resolved.clone(), PrefixSource::Declared(idx)),
                    );
                    self.s.prefixes.push(PrefixDecl {
                        name,
                        iri: raw.to_owned(),
                        resolved,
                        span: Span::new(kw.start, t.span.end),
                        name_span: n.span,
                        iri_span: t.span,
                    });
                }
                Some("VERSION") => {
                    let kw = self.bump().unwrap().span;
                    if self.kind() == Some(TokenKind::String) {
                        let t = self.bump().unwrap();
                        self.s.versions.push(VersionDecl {
                            span: Span::new(kw.start, t.span.end),
                            value_span: t.span,
                        });
                    }
                }
                _ => return,
            }
        }
    }

    /// The top scope of a new operation.
    fn operation_scope(&mut self) -> ScopeId {
        if std::mem::replace(&mut self.started, true) {
            self.new_scope(None, None)
        } else {
            0
        }
    }

    fn new_scope(&mut self, parent: Option<ScopeId>, select: Option<SelectId>) -> ScopeId {
        self.s.scopes.push(Scope { parent, select });
        (self.s.scopes.len() - 1) as ScopeId
    }

    fn start_clause(&mut self, op: &mut Operation, kind: ClauseKind, keyword: Span) {
        self.end_clause(op);
        op.clauses.push(Clause {
            kind,
            keyword,
            span: keyword,
        });
    }

    fn end_clause(&mut self, op: &mut Operation) {
        if let Some(c) = op.clauses.last_mut() {
            c.span.end = c.span.end.max(self.last_end);
        }
    }

    // --- queries -------------------------------------------------------------

    fn query(&mut self, kw: &str) {
        let kind = match kw {
            "SELECT" => OperationKind::Select,
            "CONSTRUCT" => OperationKind::Construct,
            "DESCRIBE" => OperationKind::Describe,
            _ => OperationKind::Ask,
        };
        let keyword = self.peek().unwrap().span;
        let scope = self.operation_scope();
        self.scope = scope;
        self.group = None;
        let mut op = Operation {
            kind,
            keyword,
            span: keyword,
            clauses: Vec::new(),
            scope,
        };
        self.start_clause(&mut op, ClauseKind::Form, keyword);
        let mut select = None;
        match kind {
            OperationKind::Select => {
                select = Some(self.select(scope, None));
            }
            OperationKind::Construct => {
                self.bump();
                if self.is_punct("{") {
                    self.group(GroupKind::Template, None);
                }
            }
            OperationKind::Describe => {
                self.bump();
                loop {
                    match self.kind() {
                        Some(TokenKind::Var) => {
                            let t = self.bump().unwrap();
                            self.add_var(t, VarRole::Projected);
                        }
                        Some(TokenKind::Iri | TokenKind::PName) => {
                            let t = self.bump().unwrap();
                            self.add_ref(t, TermRole::Value, None);
                        }
                        Some(TokenKind::Punct) if self.is_punct("*") => {
                            self.bump();
                        }
                        _ => break,
                    }
                }
            }
            _ => {
                self.bump();
            }
        }
        let where_group = self.query_rest(&mut op, false);
        if let Some(sel) = select {
            self.s.selects[sel as usize].where_group = where_group;
        }
        self.end_clause(&mut op);
        op.span.end = self.last_end;
        self.s.operations.push(op);
    }

    /// `SELECT` and its projection, making a scope under `parent` (`None`
    /// for the query's own) whose select it is.
    fn select(&mut self, scope: ScopeId, group: Option<GroupId>) -> SelectId {
        let keyword = self.bump().unwrap().span;
        let id = self.s.selects.len() as SelectId;
        self.s.scopes[scope as usize].select = Some(id);
        self.s.selects.push(Select {
            modifier: None,
            star: None,
            items: Vec::new(),
            span: keyword,
            scope,
            where_group: None,
            group,
        });
        self.projected.push(FxHashSet::default());
        let modifier = self.eat_kw("DISTINCT").or_else(|| self.eat_kw("REDUCED"));
        let mut star = None;
        let mut items = Vec::new();
        loop {
            if let Some(s) = self.eat("*") {
                star = Some(s);
            } else if self.kind() == Some(TokenKind::Var) {
                let t = self.bump().unwrap();
                let var = self.add_var(t, VarRole::Projected);
                items.push(Projected { var, expr: None });
            } else if self.is_punct("(") {
                let start = self.peek().unwrap().span.start;
                if let Some(var) = self.paren_expr() {
                    items.push(Projected {
                        var,
                        expr: Some(Span::new(start, self.last_end)),
                    });
                }
            } else {
                break;
            }
        }
        let names: FxHashSet<String> = items
            .iter()
            .map(|p| self.s.vars[p.var as usize].name.clone())
            .collect();
        self.projected[id as usize] = names;
        let sel = &mut self.s.selects[id as usize];
        sel.modifier = modifier;
        sel.star = star;
        sel.items = items;
        sel.span = Span::new(keyword.start, self.last_end);
        id
    }

    /// Dataset clauses, `WHERE`, solution modifiers and `VALUES`; stops at
    /// what cannot continue the query (a `}` closing a subquery, the next
    /// operation's keyword). Returns the `WHERE` group.
    fn query_rest(&mut self, op: &mut Operation, subquery: bool) -> Option<GroupId> {
        let mut where_group = None;
        while let Some(t) = self.peek() {
            if self.is_punct("}") {
                if subquery {
                    break;
                }
                self.skip();
                continue;
            }
            if self.is_punct("{") {
                if where_group.is_some() {
                    if subquery {
                        break;
                    }
                    self.skip();
                    continue;
                }
                if !op
                    .clauses
                    .last()
                    .is_some_and(|c| c.kind == ClauseKind::Where)
                {
                    self.start_clause(op, ClauseKind::Where, t.span);
                }
                where_group = self.group(GroupKind::Where, None);
                continue;
            }
            match self.keyword() {
                Some("FROM") => {
                    self.start_clause(op, ClauseKind::Dataset, t.span);
                    self.bump();
                    self.eat_kw("NAMED");
                    self.graph_iri();
                }
                Some("WHERE") => {
                    self.start_clause(op, ClauseKind::Where, t.span);
                    self.bump();
                    if self.is_punct("{") && where_group.is_none() {
                        let kw = Some(t.span);
                        where_group = self.group(GroupKind::Where, kw);
                    }
                }
                Some("GROUP") => {
                    self.start_clause(op, ClauseKind::GroupBy, t.span);
                    self.bump();
                    self.eat_kw("BY");
                    self.conditions();
                }
                Some("HAVING") => {
                    self.start_clause(op, ClauseKind::Having, t.span);
                    self.bump();
                    self.conditions();
                }
                Some("ORDER") => {
                    self.start_clause(op, ClauseKind::OrderBy, t.span);
                    self.bump();
                    self.eat_kw("BY");
                    self.conditions();
                }
                Some(kw @ ("LIMIT" | "OFFSET")) => {
                    let kind = if kw == "LIMIT" {
                        ClauseKind::Limit
                    } else {
                        ClauseKind::Offset
                    };
                    self.start_clause(op, kind, t.span);
                    self.bump();
                    if self.kind() == Some(TokenKind::Number) {
                        self.bump();
                    }
                }
                Some("VALUES") => {
                    self.start_clause(op, ClauseKind::Values, t.span);
                    self.bump();
                    self.data_block(t.span);
                }
                Some(
                    "SELECT" | "CONSTRUCT" | "DESCRIBE" | "ASK" | "LOAD" | "CLEAR" | "DROP"
                    | "CREATE" | "ADD" | "MOVE" | "COPY" | "INSERT" | "DELETE" | "WITH",
                ) => break,
                Some("PREFIX" | "BASE" | "VERSION") if !subquery => break,
                _ => self.skip(),
            }
        }
        where_group
    }

    /// `GROUP BY`, `HAVING` and `ORDER BY` conditions.
    fn conditions(&mut self) {
        loop {
            match self.kind() {
                Some(TokenKind::Var) => {
                    let t = self.bump().unwrap();
                    self.add_var(t, VarRole::Expression);
                }
                Some(TokenKind::Punct) if self.is_punct("(") => {
                    self.paren_expr();
                }
                Some(TokenKind::Keyword)
                    if matches!(self.keyword(), Some("ASC" | "DESC" | "NOT" | "EXISTS")) =>
                {
                    if self.is_kw("ASC") || self.is_kw("DESC") {
                        self.bump();
                    } else {
                        self.call();
                    }
                }
                Some(TokenKind::Function | TokenKind::Iri | TokenKind::PName) => self.call(),
                _ => return,
            }
        }
    }

    /// The IRI of `FROM`, `USING`, `WITH`, `INTO`, `GRAPH`, ... if next.
    fn graph_iri(&mut self) {
        if matches!(self.kind(), Some(TokenKind::Iri | TokenKind::PName)) {
            let t = self.bump().unwrap();
            self.add_ref(t, TermRole::Graph, None);
        }
    }

    // --- updates -------------------------------------------------------------

    fn update(&mut self) {
        let first = self.peek().unwrap();
        let keyword = first.span;
        let kind = match self.keyword() {
            Some("LOAD") => OperationKind::Load,
            Some("CLEAR") => OperationKind::Clear,
            Some("DROP") => OperationKind::Drop,
            Some("CREATE") => OperationKind::Create,
            Some("ADD") => OperationKind::Add,
            Some("MOVE") => OperationKind::Move,
            Some("COPY") => OperationKind::Copy,
            Some("INSERT") if self.is_kw_at(1, "DATA") => OperationKind::InsertData,
            Some("DELETE") if self.is_kw_at(1, "DATA") => OperationKind::DeleteData,
            Some("DELETE") if self.is_kw_at(1, "WHERE") => OperationKind::DeleteWhere,
            _ => OperationKind::Modify,
        };
        let scope = self.operation_scope();
        self.scope = scope;
        self.group = None;
        let mut op = Operation {
            kind,
            keyword,
            span: keyword,
            clauses: Vec::new(),
            scope,
        };
        // The keyword before the next `{`, which decides the group's kind.
        let mut last_kw: Option<&'static str> = None;
        let mut last_kw_span = None;
        while let Some(t) = self.peek() {
            if self.is_punct(";") {
                self.end_clause(&mut op);
                op.span.end = self.last_end.max(keyword.end);
                self.bump();
                self.s.operations.push(op);
                return;
            }
            if self.is_punct("{") {
                let kind = match last_kw {
                    Some("DATA") => GroupKind::Data,
                    Some("INSERT" | "DELETE") => GroupKind::Template,
                    Some("WHERE") => GroupKind::Where,
                    _ => GroupKind::Group,
                };
                self.group(kind, last_kw_span);
                last_kw = None;
                last_kw_span = None;
                continue;
            }
            let kw = self.keyword();
            match kw {
                Some("PREFIX" | "BASE" | "VERSION") => break,
                Some(
                    "SELECT" | "CONSTRUCT" | "DESCRIBE" | "ASK" | "LOAD" | "CLEAR" | "DROP"
                    | "CREATE" | "ADD" | "MOVE" | "COPY",
                ) if t.span != keyword => break,
                Some("WITH") if t.span != keyword => break,
                Some("WITH") => {
                    self.start_clause(&mut op, ClauseKind::With, t.span);
                    self.bump();
                    self.graph_iri();
                }
                Some("USING") => {
                    self.start_clause(&mut op, ClauseKind::Using, t.span);
                    self.bump();
                    self.eat_kw("NAMED");
                    self.graph_iri();
                }
                Some(k @ ("INSERT" | "DELETE" | "WHERE")) => {
                    // `INSERT` / `DELETE` after a complete operation: a
                    // missing `;`, so the next operation.
                    if t.span != keyword && k != "WHERE" {
                        let had_group = self
                            .s
                            .groups
                            .last()
                            .is_some_and(|g| g.open.start > keyword.start);
                        let after_where = op.clauses.iter().any(|c| c.kind == ClauseKind::Where);
                        if (op.kind != OperationKind::Modify && had_group) || after_where {
                            break;
                        }
                    }
                    let clause = match k {
                        "INSERT" => ClauseKind::Insert,
                        "DELETE" => ClauseKind::Delete,
                        _ => ClauseKind::Where,
                    };
                    let starts_clause = op.kind == OperationKind::Modify
                        || (k == "WHERE" && op.kind != OperationKind::DeleteWhere);
                    if starts_clause {
                        self.start_clause(&mut op, clause, t.span);
                    }
                    last_kw = Some(k);
                    last_kw_span = Some(t.span);
                    self.bump();
                }
                Some("DATA") => {
                    last_kw = Some("DATA");
                    last_kw_span = Some(t.span);
                    self.bump();
                }
                Some("INTO" | "TO" | "GRAPH") => {
                    self.bump();
                    self.eat_kw("GRAPH");
                    self.graph_iri();
                }
                _ => match t.kind {
                    TokenKind::Iri | TokenKind::PName => {
                        self.bump();
                        self.add_ref(t, TermRole::Graph, None);
                    }
                    _ => self.skip(),
                },
            }
        }
        self.end_clause(&mut op);
        op.span.end = self.last_end.max(keyword.end);
        self.s.operations.push(op);
    }

    // --- groups --------------------------------------------------------------

    /// A `{ }` group at the current `{`, of `kind`.
    fn group(&mut self, kind: GroupKind, keyword: Option<Span>) -> Option<GroupId> {
        let open = self.peek().filter(|_| self.is_punct("{"))?.span;
        let id = self.s.groups.len() as GroupId;
        let depth = self
            .group
            .map_or(0, |g| self.s.groups[g as usize].depth + 1);
        self.s.groups.push(Group {
            kind,
            keyword,
            open,
            close: None,
            span: open,
            parent: self.group,
            depth,
            scope: self.scope,
        });
        self.bump();
        let outer = self.group;
        let (in_expr, aggregate) = (self.in_expr, self.aggregate);
        self.group = Some(id);
        self.in_expr = 0;
        self.aggregate = 0;
        self.depth += 1;
        if kind == GroupKind::Values {
            self.in_expr = 1;
        }
        let close = if self.depth > MAX_DEPTH {
            self.skip_to_close()
        } else if kind == GroupKind::Values {
            self.values_rows()
        } else {
            self.group_body()
        };
        self.depth -= 1;
        self.group = outer;
        self.in_expr = in_expr;
        self.aggregate = aggregate;
        let g = &mut self.s.groups[id as usize];
        g.close = close;
        g.span = Span::new(open.start, close.map_or(self.last_end, |c| c.end));
        Some(id)
    }

    /// Skips to the `}` closing the current group (nesting counted).
    fn skip_to_close(&mut self) -> Option<Span> {
        let mut depth = 0u32;
        while let Some(t) = self.peek() {
            if self.is_punct("{") {
                depth += 1;
            } else if self.is_punct("}") {
                if depth == 0 {
                    self.bump();
                    return Some(t.span);
                }
                depth -= 1;
            }
            self.skip();
        }
        None
    }

    fn group_body(&mut self) -> Option<Span> {
        let mut union = None;
        loop {
            let t = self.peek()?;
            if self.is_punct("}") {
                self.bump();
                return Some(t.span);
            }
            if self.is_punct("{") {
                if self.is_kw_at(1, "SELECT") {
                    self.subquery();
                } else {
                    let kind = if union.is_some() {
                        GroupKind::Union
                    } else {
                        GroupKind::Group
                    };
                    self.group(kind, union);
                }
                union = None;
                continue;
            }
            union = None;
            match self.keyword() {
                Some("UNION") => {
                    union = Some(t.span);
                    self.bump();
                }
                Some(kw @ ("OPTIONAL" | "MINUS" | "LATERAL")) => {
                    self.bump();
                    let kind = match kw {
                        "OPTIONAL" => GroupKind::Optional,
                        "MINUS" => GroupKind::Minus,
                        _ => GroupKind::Lateral,
                    };
                    self.group(kind, Some(t.span));
                }
                Some(kw @ ("GRAPH" | "SERVICE")) => {
                    self.bump();
                    self.eat_kw("SILENT");
                    match self.kind() {
                        Some(TokenKind::Var) => {
                            let v = self.bump().unwrap();
                            self.add_var(v, VarRole::Graph);
                        }
                        Some(TokenKind::Iri | TokenKind::PName) => self.graph_iri(),
                        _ => {}
                    }
                    let kind = if kw == "GRAPH" {
                        GroupKind::Graph
                    } else {
                        GroupKind::Service
                    };
                    self.group(kind, Some(t.span));
                }
                Some("FILTER") => {
                    self.bump();
                    self.in_expr += 1;
                    if self.is_punct("(") {
                        self.paren_expr();
                    } else {
                        self.call();
                    }
                    self.in_expr -= 1;
                }
                Some("BIND") => {
                    self.bump();
                    self.paren_expr();
                }
                Some("VALUES") => {
                    self.bump();
                    self.data_block(t.span);
                }
                _ => {
                    if self.term_start() {
                        self.triples();
                    } else {
                        self.skip();
                    }
                }
            }
        }
    }

    fn subquery(&mut self) {
        let open = self.peek().unwrap().span;
        let id = self.s.groups.len() as GroupId;
        let depth = self
            .group
            .map_or(0, |g| self.s.groups[g as usize].depth + 1);
        let outer_scope = self.scope;
        let scope = self.new_scope(Some(outer_scope), None);
        self.s.groups.push(Group {
            kind: GroupKind::Subquery,
            keyword: None,
            open,
            close: None,
            span: open,
            parent: self.group,
            depth,
            scope,
        });
        self.bump();
        let outer = self.group;
        self.group = Some(id);
        self.scope = scope;
        self.depth += 1;
        let sel = self.select(scope, Some(id));
        // Clauses of a subquery are not recorded on the operation.
        let mut scratch = Operation {
            kind: OperationKind::Select,
            keyword: open,
            span: open,
            clauses: Vec::new(),
            scope,
        };
        let where_group = if self.depth > MAX_DEPTH {
            None
        } else {
            self.query_rest(&mut scratch, true)
        };
        self.s.selects[sel as usize].where_group = where_group;
        let close = self.skip_to_close();
        self.depth -= 1;
        self.scope = outer_scope;
        self.group = outer;
        let g = &mut self.s.groups[id as usize];
        g.close = close;
        g.span = Span::new(open.start, close.map_or(self.last_end, |c| c.end));
    }

    /// `VALUES ?x { ... }` or `VALUES (?x ?y) { (...) ... }`.
    fn data_block(&mut self, keyword: Span) {
        if self.kind() == Some(TokenKind::Var) {
            let t = self.bump().unwrap();
            self.add_var(t, VarRole::Values);
        } else if self.eat("(").is_some() {
            while self.kind() == Some(TokenKind::Var) {
                let t = self.bump().unwrap();
                self.add_var(t, VarRole::Values);
            }
            self.eat(")");
        }
        if self.is_punct("{") {
            self.group(GroupKind::Values, Some(keyword));
        }
    }

    fn values_rows(&mut self) -> Option<Span> {
        loop {
            let t = self.peek()?;
            if self.is_punct("}") {
                self.bump();
                return Some(t.span);
            }
            if self.is_punct("(") || self.is_punct(")") || self.is_kw("UNDEF") {
                self.bump();
            } else if self.is_punct("{") {
                // Broken: a group where a row belongs.
                self.group(GroupKind::Group, None);
            } else if self.term_start() {
                self.term(At::Values, None);
            } else {
                self.skip();
            }
        }
    }

    // --- expressions -----------------------------------------------------------

    /// A parenthesised expression at the current `(`. Returns the variable
    /// assigned with `AS` directly inside it (projection, `BIND`, `GROUP BY`).
    fn paren_expr(&mut self) -> Option<VarId> {
        self.eat("(")?;
        self.in_expr += 1;
        self.depth += 1;
        let mut assigned = None;
        let mut depth = 1u32;
        while let Some(t) = self.peek() {
            if self.is_punct(")") {
                self.bump();
                depth -= 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            if self.is_punct("(") {
                self.bump();
                depth += 1;
                continue;
            }
            if self.is_punct("}") {
                // Broken: leave the `}` to its group.
                break;
            }
            if depth == 1 && self.is_kw("AS") {
                self.bump();
                if self.kind() == Some(TokenKind::Var) {
                    let v = self.bump().unwrap();
                    assigned = Some(self.add_var(v, VarRole::Assigned));
                }
                continue;
            }
            if self.depth > MAX_DEPTH {
                self.skip();
                continue;
            }
            match t.kind {
                TokenKind::Var => {
                    self.bump();
                    self.add_var(t, VarRole::Expression);
                }
                TokenKind::Function | TokenKind::Iri | TokenKind::PName => self.call(),
                TokenKind::Keyword if self.is_kw("EXISTS") || self.is_kw("NOT") => self.call(),
                TokenKind::Punct if self.is_punct("{") => {
                    self.group(GroupKind::Group, None);
                }
                TokenKind::Punct if self.is_punct("^^") => {
                    self.bump();
                    if matches!(self.kind(), Some(TokenKind::Iri | TokenKind::PName)) {
                        let d = self.bump().unwrap();
                        self.add_ref(d, TermRole::Datatype, None);
                    }
                }
                TokenKind::Punct if self.is_punct("<<(") || self.is_punct("<<") => {
                    self.term(At::Expr, None);
                }
                _ => self.skip(),
            }
        }
        self.depth -= 1;
        self.in_expr -= 1;
        assigned
    }

    /// A function call, `EXISTS { }` / `NOT EXISTS { }`, or a bare IRI in an
    /// expression.
    fn call(&mut self) {
        let Some(t) = self.peek() else { return };
        match t.kind {
            TokenKind::Keyword if self.is_kw("NOT") => {
                self.bump();
                if self.is_kw("EXISTS") {
                    self.call();
                }
            }
            TokenKind::Keyword if self.is_kw("EXISTS") => {
                self.bump();
                if self.is_punct("{") {
                    self.group(GroupKind::Exists, Some(t.span));
                }
            }
            TokenKind::Function => {
                self.bump();
                let aggregate = lexer::is_aggregate(self.text(t));
                if aggregate {
                    if self.aggregate > 0 {
                        self.error(t.span, "An aggregate cannot contain another aggregate");
                    }
                    self.aggregate += 1;
                }
                if self.is_punct("(") {
                    self.in_expr += 1;
                    self.paren_expr();
                    self.in_expr -= 1;
                }
                if aggregate {
                    self.aggregate -= 1;
                }
            }
            TokenKind::Iri | TokenKind::PName => {
                self.bump();
                let call = self.is_punct("(");
                let role = if call {
                    TermRole::Function
                } else {
                    TermRole::Value
                };
                self.add_ref(t, role, None);
                if call {
                    self.in_expr += 1;
                    self.paren_expr();
                    self.in_expr -= 1;
                }
            }
            _ => self.skip(),
        }
    }

    // --- triples ---------------------------------------------------------------

    fn term_start(&self) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::Var
            | TokenKind::Iri
            | TokenKind::PName
            | TokenKind::String
            | TokenKind::Number
            | TokenKind::Boolean
            | TokenKind::BlankNode => true,
            TokenKind::Punct => match self.text(t) {
                "[" | "(" | "<<" | "<<(" => true,
                "+" | "-" => self.tok(1).is_some_and(|n| n.kind == TokenKind::Number),
                _ => false,
            },
            _ => false,
        }
    }

    fn verb_start(&self) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::A | TokenKind::Var | TokenKind::Iri | TokenKind::PName => true,
            TokenKind::Punct => matches!(self.text(t), "^" | "!" | "("),
            _ => false,
        }
    }

    /// A subject and its property list.
    fn triples(&mut self) {
        let group = self.group;
        let Some(subject) = self.term(At::S, None) else {
            self.skip();
            return;
        };
        let before = self.s.triples.len();
        self.property_list(subject);
        // A subject with nothing after it (`?s }`, or a `[ ... ]` alone).
        if self.s.triples.len() == before && !self.text_at(subject).starts_with('[') {
            self.s.triples.push(TriplePattern {
                subject,
                predicate: None,
                object: None,
                group,
                quoted: self.quoted > 0,
            });
        }
    }

    fn text_at(&self, s: Span) -> &'a str {
        let src = self.src;
        &src[s.start as usize..s.end as usize]
    }

    fn property_list(&mut self, subject: Span) {
        loop {
            if !self.verb_start() {
                return;
            }
            let p = self.verb(subject);
            self.object_list(subject, p);
            let mut semi = false;
            while self.eat(";").is_some() {
                semi = true;
            }
            if !semi {
                return;
            }
        }
    }

    fn verb(&mut self, subject: Span) -> Span {
        let start = self.peek().unwrap().span.start;
        if self.kind() == Some(TokenKind::Var) {
            let t = self.bump().unwrap();
            self.add_var(t, VarRole::Pattern(Position::Predicate));
        } else {
            self.path(subject);
        }
        Span::new(start, self.last_end.max(start))
    }

    /// A property path (alternatives of sequences of elements).
    fn path(&mut self, subject: Span) {
        loop {
            self.path_elt(subject);
            if self.is_punct("|") || self.is_punct("/") {
                self.bump();
                continue;
            }
            return;
        }
    }

    fn path_elt(&mut self, subject: Span) {
        self.eat("^");
        let Some(t) = self.peek() else { return };
        match t.kind {
            TokenKind::A | TokenKind::Iri | TokenKind::PName => {
                self.bump();
                self.add_ref(t, TermRole::Predicate, Some(subject));
            }
            TokenKind::Var => {
                self.bump();
                self.add_var(t, VarRole::Pattern(Position::Predicate));
            }
            TokenKind::Punct if self.is_punct("!") => {
                self.bump();
                if self.eat("(").is_some() {
                    while !self.is_punct(")") && self.peek().is_some() {
                        if self.is_punct("|") || self.is_punct("^") {
                            self.bump();
                        } else if matches!(
                            self.kind(),
                            Some(TokenKind::A | TokenKind::Iri | TokenKind::PName)
                        ) {
                            let t = self.bump().unwrap();
                            self.add_ref(t, TermRole::Predicate, Some(subject));
                        } else {
                            break;
                        }
                    }
                    self.eat(")");
                } else {
                    self.path_elt(subject);
                }
            }
            TokenKind::Punct if self.is_punct("(") => {
                self.bump();
                if self.depth < MAX_DEPTH {
                    self.depth += 1;
                    self.path(subject);
                    self.depth -= 1;
                }
                self.eat(")");
            }
            _ => return,
        }
        // Modifiers: `*`, `+`, `?`.
        if self.is_punct("*") || self.is_punct("+") || self.is_punct("?") {
            self.bump();
        }
    }

    fn object_list(&mut self, subject: Span, predicate: Span) {
        let group = self.group;
        let quoted = self.quoted > 0;
        let mut any = false;
        loop {
            if !self.term_start() {
                break;
            }
            let object = self.term(At::O, Some(subject));
            any = true;
            self.s.triples.push(TriplePattern {
                subject,
                predicate: Some(predicate),
                object,
                group,
                quoted,
            });
            self.annotations(object.unwrap_or(subject));
            if self.eat(",").is_none() {
                break;
            }
        }
        if !any {
            self.s.triples.push(TriplePattern {
                subject,
                predicate: Some(predicate),
                object: None,
                group,
                quoted,
            });
        }
    }

    /// Reifiers (`~ ?r`) and annotation blocks (`{| ... |}`) after an object.
    fn annotations(&mut self, object: Span) {
        let mut node = object;
        loop {
            if self.eat("~").is_some() {
                let reifier = matches!(
                    self.kind(),
                    Some(TokenKind::Var | TokenKind::Iri | TokenKind::PName | TokenKind::BlankNode)
                ) || self.is_punct("[");
                if reifier && let Some(r) = self.term(At::S, None) {
                    node = r;
                }
            } else if self.eat("{|").is_some() {
                if self.depth < MAX_DEPTH {
                    self.depth += 1;
                    self.property_list(node);
                    self.depth -= 1;
                }
                self.eat("|}");
            } else {
                return;
            }
        }
    }

    /// A term at `at`; returns its span (a whole `[ ]`, `( )`, `<< >>`).
    fn term(&mut self, at: At, subject: Option<Span>) -> Option<Span> {
        let t = self.peek()?;
        let start = t.span.start;
        let (var_role, ref_role) = match at {
            At::S => (VarRole::Pattern(Position::Subject), TermRole::Subject),
            At::O => (VarRole::Pattern(Position::Object), TermRole::Object),
            At::Values => (VarRole::Expression, TermRole::Value),
            At::Expr => (VarRole::Expression, TermRole::Value),
        };
        if self.depth > MAX_DEPTH {
            self.skip();
            return Some(t.span);
        }
        match t.kind {
            TokenKind::Var => {
                self.bump();
                self.add_var(t, var_role);
            }
            TokenKind::Iri | TokenKind::PName => {
                self.bump();
                self.add_ref(t, ref_role, subject);
            }
            TokenKind::String => {
                self.bump();
                if self.kind() == Some(TokenKind::LangTag) {
                    self.bump();
                } else if self.eat("^^").is_some()
                    && matches!(self.kind(), Some(TokenKind::Iri | TokenKind::PName))
                {
                    let d = self.bump().unwrap();
                    self.add_ref(d, TermRole::Datatype, None);
                }
            }
            TokenKind::Number | TokenKind::Boolean | TokenKind::BlankNode => {
                self.bump();
            }
            TokenKind::Punct => {
                self.depth += 1;
                match self.text(t) {
                    "+" | "-" => {
                        self.bump();
                        self.bump();
                    }
                    "[" => {
                        self.bump();
                        let node = Span::new(start, t.span.end);
                        if at == At::Values || at == At::Expr {
                            // `[]` only; anything else is broken.
                        } else {
                            self.property_list(node);
                        }
                        self.eat("]");
                    }
                    "(" => {
                        self.bump();
                        let item = if matches!(at, At::Values | At::Expr) {
                            at
                        } else {
                            At::O
                        };
                        while self.term_start() {
                            self.term(item, None);
                        }
                        self.eat(")");
                    }
                    "<<(" => {
                        self.bump();
                        self.quoted += 1;
                        // In patterns a triple term may be the subject;
                        // in expressions and `VALUES` only an IRI or variable.
                        if let Some(s) = self.peek().filter(|_| self.in_expr > 0) {
                            let bad = s.kind == TokenKind::String
                                || s.kind == TokenKind::Number
                                || s.kind == TokenKind::Boolean
                                || (s.kind == TokenKind::Punct
                                    && matches!(self.text(s), "<<(" | "+" | "-"));
                            if bad {
                                self.error(
                                    s.span,
                                    "The subject of a triple term cannot be a triple term or a literal",
                                );
                            }
                        }
                        self.quoted_triple();
                        self.quoted -= 1;
                        self.eat(")>>");
                    }
                    "<<" => {
                        self.bump();
                        self.quoted += 1;
                        self.quoted_triple();
                        if self.eat("~").is_some()
                            && matches!(
                                self.kind(),
                                Some(
                                    TokenKind::Var
                                        | TokenKind::Iri
                                        | TokenKind::PName
                                        | TokenKind::BlankNode
                                )
                            )
                        {
                            self.term(At::S, None);
                        }
                        self.quoted -= 1;
                        self.eat(">>");
                    }
                    _ => {
                        self.depth -= 1;
                        return None;
                    }
                }
                self.depth -= 1;
            }
            _ => return None,
        }
        Some(Span::new(start, self.last_end.max(t.span.end)))
    }

    /// The subject, verb and object of `<<( )>>` or `<< >>`.
    fn quoted_triple(&mut self) {
        let group = self.group;
        let Some(s) = self.term(At::S, None) else {
            return;
        };
        let p = if self.verb_start() {
            Some(self.verb(s))
        } else {
            None
        };
        let o = if self.term_start() {
            self.term(At::O, Some(s))
        } else {
            None
        };
        self.s.triples.push(TriplePattern {
            subject: s,
            predicate: p,
            object: o,
            group,
            quoted: true,
        });
    }

    // --- scopes ----------------------------------------------------------------

    /// Moves each variable out of every subquery that projects it.
    fn resolve_scopes(&mut self) {
        for i in 0..self.s.vars.len() {
            let mut scope = self.s.vars[i].raw_scope;
            loop {
                let sc = &self.s.scopes[scope as usize];
                let (Some(parent), Some(sel)) = (sc.parent, sc.select) else {
                    break;
                };
                let s = &self.s.selects[sel as usize];
                if s.star.is_some() || self.projected[sel as usize].contains(&self.s.vars[i].name) {
                    scope = parent;
                } else {
                    break;
                }
            }
            self.s.vars[i].scope = scope;
        }
    }
}

/// Whether the variable occurrence written in `raw` reaches scope `target`
/// through the subqueries that project `name`.
pub(crate) fn reaches(s: &Structure, mut raw: ScopeId, target: ScopeId, name: &str) -> bool {
    loop {
        if raw == target {
            return true;
        }
        let sc = &s.scopes[raw as usize];
        match (sc.parent, sc.select) {
            (Some(parent), Some(sel)) if s.projects(sel, name) => raw = parent,
            _ => return false,
        }
    }
}

/// `\uXXXX` and `\UXXXXXXXX` undone (IRIs allow no other escapes).
fn unescape_uchar(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\\') {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let n = match tail.as_bytes().get(1) {
            Some(b'u') => 4,
            Some(b'U') => 8,
            _ => 0,
        };
        let c = (n > 0)
            .then(|| tail.get(2..2 + n))
            .flatten()
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32);
        match c {
            Some(c) => {
                out.push(c);
                rest = &tail[2 + n..];
            }
            None => {
                out.push('\\');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out.into()
}
