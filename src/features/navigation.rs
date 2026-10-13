//! Definition, references, rename and document highlight, all keyed by IRI.
//! Cursor on the prefix part of `ex:a` (or a prefix declaration) works on the
//! prefix instead.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::{Form, Iri, Role, Span};
use crate::workspace::{PREFIX_FILE, path_to_uri};
use lsp_types::*;
use rustc_hash::FxHashSet;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

pub enum Target {
    Iri(Iri),
    /// A prefix name, as used or declared in this document.
    Prefix(String),
}

pub fn target_at(a: &Analysis, offset: u32) -> Option<(Target, Span)> {
    if let Some(p) = a
        .parsed
        .prefixes
        .iter()
        .find(|p| p.name_span.contains(offset))
    {
        return Some((Target::Prefix(p.name.clone()), p.name_span));
    }
    let r = a.ref_at(offset)?;
    if let Form::PName { prefix_len } = r.form
        && offset <= r.span.start + prefix_len
    {
        let name = a
            .parsed
            .prefix_uses
            .iter()
            .find(|u| u.span == r.span)?
            .name
            .to_string();
        return Some((
            Target::Prefix(name),
            Span::new(r.span.start, r.span.start + prefix_len),
        ));
    }
    Some((Target::Iri(r.iri), r.span))
}

pub fn uri(s: &str) -> Uri {
    Uri::from_str(s).expect("valid uri")
}

fn loc(a: &Analysis, enc: Encoding, span: Span) -> Location {
    Location {
        uri: uri(&a.uri),
        range: a.range(enc, span),
    }
}

pub fn definition(
    a: &Analysis,
    all: &[Arc<Analysis>],
    enc: Encoding,
    pos: Position,
    prefix_file: Option<Arc<Analysis>>,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    match target_at(a, offset) {
        Some((Target::Prefix(name), _)) => {
            if let Some(p) = a.parsed.prefixes.iter().find(|p| p.name == name) {
                return vec![loc(a, enc, p.name_span)];
            }
            prefix_file
                .and_then(|pf| {
                    pf.parsed
                        .prefixes
                        .iter()
                        .find(|p| p.name == name)
                        .map(|p| loc(&pf, enc, p.name_span))
                })
                .into_iter()
                .collect()
        }
        Some((Target::Iri(iri), _)) => {
            let mut out = Vec::new();
            for other in all {
                for span in other.defs.get(&iri).into_iter().flatten() {
                    out.push(loc(other, enc, *span));
                }
            }
            out.sort_by(|x, y| {
                (
                    x.uri.as_str() != a.uri.as_str(),
                    x.uri.as_str(),
                    x.range.start.line,
                )
                    .cmp(&(
                        y.uri.as_str() != a.uri.as_str(),
                        y.uri.as_str(),
                        y.range.start.line,
                    ))
            });
            out
        }
        None => vec![],
    }
}

/// Where the resource's classes are described (`gy` in Neovim).
pub fn type_definition(
    a: &Analysis,
    all: &[Arc<Analysis>],
    enc: Encoding,
    pos: Position,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let Some((Target::Iri(iri), _)) = target_at(a, offset) else {
        return vec![];
    };
    let types: FxHashSet<Iri> = all
        .iter()
        .filter_map(|o| o.types.get(&iri))
        .flatten()
        .copied()
        .collect();
    let mut out = Vec::new();
    for other in all {
        for ty in &types {
            for span in other.defs.get(ty).into_iter().flatten() {
                out.push(loc(other, enc, *span));
            }
        }
    }
    out
}

/// Instances, subclasses and subproperties of the IRI under the cursor.
pub fn implementations(
    a: &Analysis,
    all: &[Arc<Analysis>],
    enc: Encoding,
    pos: Position,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let Some((Target::Iri(iri), _)) = target_at(a, offset) else {
        return vec![];
    };
    let mut out = Vec::new();
    for other in all {
        let related = other
            .types
            .iter()
            .chain(other.supers.iter())
            .filter(|(_, objs)| objs.contains(&iri))
            .map(|(s, _)| s);
        let mut seen = FxHashSet::default();
        for s in related {
            if !seen.insert(s) {
                continue;
            }
            for span in other.defs.get(s).into_iter().flatten() {
                out.push(loc(other, enc, *span));
            }
        }
    }
    out
}

pub fn references(
    a: &Analysis,
    all: &[Arc<Analysis>],
    enc: Encoding,
    pos: Position,
    include_decl: bool,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let mut out = Vec::new();
    match target_at(a, offset) {
        Some((Target::Prefix(name), _)) => {
            if include_decl {
                out.extend(
                    a.parsed
                        .prefixes
                        .iter()
                        .filter(|p| p.name == name)
                        .map(|p| loc(a, enc, p.name_span)),
                );
            }
            out.extend(
                a.parsed
                    .prefix_uses
                    .iter()
                    .filter(|u| u.name == name)
                    .map(|u| {
                        loc(
                            a,
                            enc,
                            Span::new(u.span.start, u.span.start + name.len() as u32),
                        )
                    }),
            );
        }
        Some((Target::Iri(iri), _)) => {
            for other in all {
                let mut seen = FxHashSet::default();
                for r in other.refs_of(&iri) {
                    if (include_decl || r.role != Role::Subject) && seen.insert(r.span) {
                        out.push(loc(other, enc, r.span));
                    }
                }
            }
        }
        None => {}
    }
    out
}

pub fn highlights(a: &Analysis, enc: Encoding, pos: Position) -> Vec<DocumentHighlight> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let Some((target, _)) = target_at(a, offset) else {
        return vec![];
    };
    let mk = |span: Span, write: bool| DocumentHighlight {
        range: a.range(enc, span),
        kind: Some(if write {
            DocumentHighlightKind::WRITE
        } else {
            DocumentHighlightKind::READ
        }),
    };
    match target {
        Target::Iri(iri) => a
            .refs_of(&iri)
            .map(|r| mk(r.span, r.role == Role::Subject))
            .collect(),
        Target::Prefix(name) => a
            .parsed
            .prefixes
            .iter()
            .filter(|p| p.name == name)
            .map(|p| mk(p.name_span, true))
            .chain(
                a.parsed
                    .prefix_uses
                    .iter()
                    .filter(|u| u.name == name)
                    .map(|u| {
                        mk(
                            Span::new(u.span.start, u.span.start + name.len() as u32),
                            false,
                        )
                    }),
            )
            .collect(),
    }
}

pub fn prepare_rename(a: &Analysis, enc: Encoding, pos: Position) -> Option<PrepareRenameResponse> {
    let offset = a.offset(enc, pos)?;
    let r = a.ref_at(offset);
    if r.is_some_and(|r| r.form == Form::A) {
        return None;
    }
    let (_, span) = target_at(a, offset)?;
    Some(PrepareRenameResponse::Range(a.range(enc, span)))
}

/// Rename an IRI everywhere it occurs, or a prefix within this document.
/// `new_name` may be a local name (`robert`), a prefixed name (`ex:robert`)
/// or a full IRI (`<http://...>`); each occurrence keeps its file's style.
pub fn rename(
    a: &Analysis,
    all: &[Arc<Analysis>],
    enc: Encoding,
    pos: Position,
    new_name: &str,
) -> Result<WorkspaceEdit, String> {
    let offset = a.offset(enc, pos).ok_or("invalid position")?;
    let (target, _) = target_at(a, offset).ok_or("nothing to rename here")?;
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    match target {
        Target::Prefix(old) => {
            let new = new_name.trim_end_matches(':');
            if new.contains(':') || new.contains(char::is_whitespace) {
                return Err(format!("'{new_name}' is not a valid prefix name"));
            }
            let mut edits: Vec<TextEdit> = a
                .parsed
                .prefixes
                .iter()
                .filter(|p| p.name == old)
                .map(|p| TextEdit {
                    range: a.range(enc, p.name_span),
                    new_text: new.to_string(),
                })
                .collect();
            edits.extend(
                a.parsed
                    .prefix_uses
                    .iter()
                    .filter(|u| u.name == old)
                    .map(|u| TextEdit {
                        range: a.range(
                            enc,
                            Span::new(u.span.start, u.span.start + old.len() as u32),
                        ),
                        new_text: new.to_string(),
                    }),
            );
            changes.insert(uri(&a.uri), edits);
        }
        Target::Iri(old) => {
            let new_iri =
                if let Some(i) = new_name.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                    i.to_string()
                } else if let Some((p, l)) = new_name.split_once(':') {
                    let ns = a
                        .prefix_map()
                        .get(p)
                        .cloned()
                        .ok_or(format!("prefix '{p}:' is not declared"))?;
                    format!("{ns}{l}")
                } else {
                    let cut = old.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
                    format!("{}{new_name}", &old[..cut])
                };
            for other in all {
                let prefixes = other.prefix_map();
                let mut seen = FxHashSet::default();
                let mut edits = Vec::new();
                for r in other.refs_of(&old) {
                    if r.form == Form::A || !seen.insert(r.span) {
                        continue;
                    }
                    let text = match crate::features::completion::shorten(&new_iri, &prefixes) {
                        Some(s) if !matches!(r.form, Form::Iri) => s,
                        _ => format!("<{new_iri}>"),
                    };
                    edits.push(TextEdit {
                        range: other.range(enc, r.span),
                        new_text: text,
                    });
                }
                if !edits.is_empty() {
                    changes.insert(uri(&other.uri), edits);
                }
            }
        }
    }
    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

pub fn prefix_file_uri(path: &std::path::Path) -> Option<String> {
    if path.file_name().is_some_and(|n| n == PREFIX_FILE) {
        path_to_uri(path)
    } else {
        None
    }
}
