//! Navigation on JSON-LD terms: a term or keyword alias, as used or as
//! defined in an `@context`, and the prefix part of a compact IRI. Uses and
//! definitions come from the document's [`SourceIndex`].
//!
//! [`SourceIndex`]: crate::jsonld::SourceIndex

use crate::analysis::{Analysis, Encoding};
use crate::jsonld::source::{DefId, Definition, Origin, UseKind};
use crate::knowledge::Knowledge;
use crate::syntax::Span;
use crate::workspace::{normalize_uri, uri_to_path};
use line_index::LineIndex;
use lsp_types::Location;

/// The JSON-LD term under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermAt {
    pub definition: DefId,
    /// The text under the cursor: a use, a compact IRI's prefix part or a
    /// definition's key.
    pub span: Span,
    /// The cursor is on the definition's key.
    pub on_definition: bool,
}

/// The term (or alias) used or defined at `offset`, or the prefix term of a
/// compact IRI whose prefix part `offset` is on. `None` elsewhere, and in
/// documents other than JSON-LD.
pub fn term_at(a: &Analysis, offset: u32) -> Option<TermAt> {
    let ix = &a.source_index;
    if let Some(id) = ix.definition_at(offset) {
        return Some(TermAt {
            definition: id,
            span: ix.definition(id).key,
            on_definition: true,
        });
    }
    let u = ix.use_at(offset)?;
    let (definition, span) = match u.kind {
        UseKind::Term { definition } | UseKind::Alias { definition } => (definition, u.span),
        UseKind::Compact { prefix_len, prefix } if offset <= u.span.start + prefix_len => {
            (prefix, Span::new(u.span.start, u.span.start + prefix_len))
        }
        _ => return None,
    };
    Some(TermAt {
        definition,
        span,
        on_definition: false,
    })
}

/// The spans in `a` of the uses of definition `id`: the whole string of a
/// term or alias, the prefix part of a compact IRI.
pub fn use_spans(a: &Analysis, id: DefId) -> impl Iterator<Item = Span> + '_ {
    a.source_index.uses_of(id).map(|u| match u.kind {
        UseKind::Compact { prefix_len, .. } => Span::new(u.span.start, u.span.start + prefix_len),
        _ => u.span,
    })
}

/// Where definition `d` of `a`'s index has its key: in `a`, or in a
/// workspace context file (its analysis, else its text on disk). `None` for
/// a bundled or remote context, or a file whose text no longer has the key
/// there.
pub fn definition_location(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    d: &Definition,
) -> Option<Location> {
    let spells = |text: &str| text.get(d.key.start as usize..d.key.end as usize) == Some(&d.term);
    match &d.origin {
        Origin::Document => Some(super::loc(a, enc, d.key)),
        Origin::Remote(_) => None,
        Origin::File(url) => {
            let uri = normalize_uri(url);
            if let Some(file) = k.get(&uri).filter(|f| spells(&f.text)) {
                return Some(super::loc(file, enc, d.key));
            }
            let text = std::fs::read_to_string(uri_to_path(&uri)?).ok()?;
            if !spells(&text) {
                return None;
            }
            let lines = LineIndex::new(&text);
            let pos = |o| crate::analysis::position_in(&lines, enc, o);
            Some(Location {
                uri: super::uri(&uri),
                range: lsp_types::Range::new(pos(d.key.start), pos(d.key.end)),
            })
        }
    }
}
