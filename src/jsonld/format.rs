//! The JSON-LD formatter: a pretty-printer over [`json`]'s parse tree with the
//! layout of `JSON.stringify(value, null, indent)`. Scalars and keys are
//! copied from the source text (number spellings, escapes), key order is kept,
//! and only whitespace changes.

use super::json::{self, Json, Value};
use crate::features::format::Options;
use crate::syntax::Span;

/// The formatted text, or `None` when the document has JSON syntax errors, is
/// empty, or, as a safety net, when the result would not parse to the same
/// JSON value.
pub fn format(text: &str, opts: &Options) -> Option<String> {
    let doc = json::parse(text);
    if !doc.errors.is_empty() {
        return None;
    }
    let root = doc.root.as_ref()?;
    let nl = match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    };
    let unit = if opts.insert_spaces {
        " ".repeat(opts.tab_size.clamp(1, 16) as usize)
    } else {
        "\t".to_string()
    };
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    write(&mut out, text, root, 0, &unit, nl)?;
    out.push_str(nl);

    let again = json::parse(&out);
    if !again.errors.is_empty() || !same(again.root.as_ref()?, root) {
        eprintln!(
            "turtle-lsp: formatter bug: the formatted JSON differs from the original; no edits applied"
        );
        return None;
    }
    Some(out)
}

fn slice(text: &str, s: Span) -> &str {
    &text[s.start as usize..s.end as usize]
}

fn pad(out: &mut String, unit: &str, level: usize) {
    for _ in 0..level {
        out.push_str(unit);
    }
}

fn write(out: &mut String, text: &str, j: &Json, level: usize, unit: &str, nl: &str) -> Option<()> {
    match &j.value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            out.push_str(slice(text, j.span));
        }
        Value::Array(a) => {
            if a.items.is_empty() {
                out.push_str("[]");
            } else {
                out.push('[');
                for (i, item) in a.items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(nl);
                    pad(out, unit, level + 1);
                    write(out, text, item, level + 1, unit, nl)?;
                }
                out.push_str(nl);
                pad(out, unit, level);
                out.push(']');
            }
        }
        Value::Object(o) => {
            if o.members.is_empty() {
                out.push_str("{}");
            } else {
                out.push('{');
                for (i, m) in o.members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(nl);
                    pad(out, unit, level + 1);
                    out.push_str(slice(text, m.key_span));
                    out.push_str(": ");
                    write(out, text, &m.value, level + 1, unit, nl)?;
                }
                out.push_str(nl);
                pad(out, unit, level);
                out.push('}');
            }
        }
        Value::Invalid => return None,
    }
    Some(())
}

/// Equal JSON values, ignoring spans.
fn same(a: &Json, b: &Json) -> bool {
    match (&a.value, &b.value) {
        (Value::Array(x), Value::Array(y)) => {
            x.items.len() == y.items.len() && x.items.iter().zip(&y.items).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.members.len() == y.members.len()
                && x.members
                    .iter()
                    .zip(&y.members)
                    .all(|(p, q)| p.key == q.key && same(&p.value, &q.value))
        }
        (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => false,
        (x, y) => x == y,
    }
}
