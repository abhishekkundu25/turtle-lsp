//! Adding a member to the nearest `@context` of a place in a JSON-LD text,
//! as completion does when it accepts a term of a prefix the context lacks
//! and the `jsonld-undefined-prefix` quick fix does.
//!
//! The objects around the place come from the document's [`SourceIndex`]
//! ([`add_member_in`]): the JSON objects expansion read, so the inside of a
//! `@json` literal is never a target. Only text that did not parse into any
//! object has no index to ask, and [`add_member`] (and [`objects_around`])
//! then scan the text's [`json::tokens`], so this works on text that is
//! half typed. Both read strings as the parser does (an unclosed string ends
//! at its line; a backslash does not take a line break with it).

use super::json::{self, Token, TokenKind};
use super::source::SourceIndex;

pub(crate) use super::json::string_end;

/// Stands for the `"prefix": "namespace"` member in [`context_edit`]'s text.
pub const MEMBER: &str = "\u{0}member\u{0}";

/// The tokens of a text (comments left out) with the scans over them.
struct Tokens<'t> {
    text: &'t str,
    toks: Vec<Token>,
}

impl<'t> Tokens<'t> {
    fn new(text: &'t str) -> Tokens<'t> {
        let toks = json::tokens(text)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Comment)
            .collect();
        Tokens { text, toks }
    }

    fn start(&self, i: usize) -> usize {
        self.toks[i].span.start as usize
    }

    /// The token that starts at `offset`.
    fn at(&self, offset: usize) -> Option<usize> {
        self.toks
            .binary_search_by_key(&(offset as u32), |t| t.span.start)
            .ok()
    }

    /// The content of the string token `i`, without its quotes.
    fn content(&self, i: usize) -> &'t str {
        let start = self.start(i);
        let (end, _) = string_end(self.text.as_bytes(), start);
        self.text.get(start + 1..end).unwrap_or_default()
    }

    /// The token of the value of the member `name` of the object whose `{`
    /// is token `open`.
    fn member_value(&self, open: usize, name: &str) -> Option<usize> {
        let mut depth = 0usize;
        let mut last_key: Option<usize> = None;
        for i in open + 1..self.toks.len() {
            match self.toks[i].kind {
                TokenKind::String if depth == 0 => last_key = Some(i),
                TokenKind::LBrace | TokenKind::LBracket => depth += 1,
                TokenKind::RBrace | TokenKind::RBracket => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                TokenKind::Colon if depth == 0 => {
                    if last_key.is_some_and(|k| self.content(k) == name) {
                        return (i + 1 < self.toks.len()).then_some(i + 1);
                    }
                }
                TokenKind::Comma if depth == 0 => last_key = None,
                _ => {}
            }
        }
        None
    }

    /// The objects directly in the array at token `open`, and where it
    /// closes (its `]`, or the end of the text).
    fn array_elements(&self, open: usize) -> (Vec<usize>, usize) {
        let mut objs = Vec::new();
        let mut depth = 0usize;
        for i in open + 1..self.toks.len() {
            match self.toks[i].kind {
                TokenKind::LBrace | TokenKind::LBracket => {
                    if depth == 0 && self.toks[i].kind == TokenKind::LBrace {
                        objs.push(self.start(i));
                    }
                    depth += 1;
                }
                TokenKind::RBrace | TokenKind::RBracket => {
                    if depth == 0 {
                        return (objs, self.start(i));
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        (objs, self.text.len())
    }

    /// The `{` of each object around `offset`, outermost first.
    fn objects_around(&self, offset: usize) -> Vec<usize> {
        let mut stack: Vec<(TokenKind, usize)> = Vec::new();
        for (i, t) in self.toks.iter().enumerate() {
            if self.start(i) >= offset {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LBracket => stack.push((t.kind, self.start(i))),
                TokenKind::RBrace | TokenKind::RBracket => {
                    stack.pop();
                }
                _ => {}
            }
        }
        stack
            .into_iter()
            .filter(|(k, _)| *k == TokenKind::LBrace)
            .map(|(_, i)| i)
            .collect()
    }

    fn context_edit(&self, objects: &[usize], offset: usize) -> (usize, usize, String) {
        let (text, b) = (self.text, self.text.as_bytes());
        for &o in objects.iter().rev() {
            let Some(open) = self.at(o) else { continue };
            let Some(vi) = self.member_value(open, "@context") else {
                continue;
            };
            let v = self.start(vi);
            match self.toks[vi].kind {
                TokenKind::LBrace => return insert_member(b, v, MEMBER),
                TokenKind::LBracket => {
                    let (objs, close) = self.array_elements(vi);
                    if let Some(&last) = objs.last() {
                        return insert_member(b, last, MEMBER);
                    }
                    let sep = if has_element(b, v, close) { ", " } else { "" };
                    return (close, 0, format!("{sep}{{ {MEMBER} }}"));
                }
                TokenKind::String => {
                    let (end, closed) = string_end(b, v);
                    let end = if closed { end + 1 } else { end };
                    return (v, end - v, format!("[{}, {{ {MEMBER} }}]", &text[v..end]));
                }
                TokenKind::Literal if text[v..].starts_with("null") => {
                    return (v, 4, format!("{{ {MEMBER} }}"));
                }
                _ => {}
            }
        }
        match objects.first() {
            Some(&o) => insert_member(b, o, &format!("\"@context\": {{ {MEMBER} }}")),
            None => (offset, 0, format!("\"@context\": {{ {MEMBER} }}")),
        }
    }
}

/// The edit adding a member to the nearest `@context` object, as (offset,
/// length replaced, text with [`MEMBER`] for the member): the `@context` of
/// the innermost object of `objects` (the `{` of each object around the
/// cursor, outermost first) that has one, else a new `@context` in the
/// outermost object.
pub fn context_edit(text: &str, objects: &[usize], offset: usize) -> (usize, usize, String) {
    Tokens::new(text).context_edit(objects, offset)
}

/// Inserts `member` first in the object at `brace`, separated from the next
/// member the way the first one is from the brace.
fn insert_member(b: &[u8], brace: usize, member: &str) -> (usize, usize, String) {
    let mut j = brace + 1;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= b.len() || b[j] == b'}' {
        return (brace + 1, 0, member.to_string());
    }
    let ws = std::str::from_utf8(&b[brace + 1..j]).unwrap_or(" ");
    let ws = if ws.is_empty() { " " } else { ws };
    (j, 0, format!("{member},{ws}"))
}

fn has_element(b: &[u8], open: usize, close: usize) -> bool {
    b[open + 1..close].iter().any(|c| !c.is_ascii_whitespace())
}

/// The `{` of each object around `offset` found by reading the text, outermost
/// first. For text with no [`SourceIndex`] scopes; [`objects_in`] asks the
/// index first.
pub fn objects_around(text: &str, offset: usize) -> Vec<usize> {
    Tokens::new(text).objects_around(offset)
}

/// The `{` of each JSON object expansion read that is around `offset`,
/// outermost first: the scopes of `index` ([`SourceIndex::scope_at`]), which
/// never include the inside of a `@json` literal. When expansion read no
/// object at all (text that is only half typed), the objects the text's
/// tokens show.
pub fn objects_in(index: &SourceIndex, text: &str, offset: usize) -> Vec<usize> {
    if index.scopes().is_empty() {
        return objects_around(text, offset);
    }
    let mut out = Vec::new();
    let mut scope = index.scope_at(offset as u32);
    while let Some(s) = scope {
        out.push(s.span.start as usize);
        scope = index.parent(s);
    }
    out.reverse();
    out
}

/// [`context_edit`] for the place at `offset` with `member` (`"name":
/// value`), as (offset, length replaced, new text), from the objects the
/// text's tokens show around it. Prefer [`add_member_in`] where the
/// document's index is at hand.
pub fn add_member(text: &str, offset: usize, member: &str) -> (usize, usize, String) {
    let toks = Tokens::new(text);
    let (at, del, new_text) = toks.context_edit(&toks.objects_around(offset), offset);
    (at, del, new_text.replace(MEMBER, member))
}

/// [`add_member`] with the objects around `offset` from `index`, the
/// [`SourceIndex`] of `text`.
pub fn add_member_in(
    index: &SourceIndex,
    text: &str,
    offset: usize,
    member: &str,
) -> (usize, usize, String) {
    let (at, del, new_text) = context_edit(text, &objects_in(index, text, offset), offset);
    (at, del, new_text.replace(MEMBER, member))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonld::{NoNetwork, parse_document};

    fn add(text: &str) -> String {
        let offset = text.find('|').unwrap();
        let text = text.replace('|', "");
        let (at, del, new) = add_member(&text, offset, r#""p": "n""#);
        format!("{}{}{}", &text[..at], new, &text[at + del..])
    }

    /// [`add`] with the objects from the document's index.
    fn add_in(text: &str) -> String {
        let offset = text.find('|').unwrap();
        let text = text.replace('|', "");
        let index = parse_document(&text, Some("file:///x/a.jsonld"), &NoNetwork::default()).index;
        let (at, del, new) = add_member_in(&index, &text, offset, r#""p": "n""#);
        format!("{}{}{}", &text[..at], new, &text[at + del..])
    }

    #[test]
    fn adds_to_the_context_around_a_string() {
        for add in [add, add_in] {
            // A key, then an IRI written in the context itself.
            assert_eq!(
                add(r#"{ "@context": { "a": "b" }, "y": { "|x:y": 1 } }"#),
                r#"{ "@context": { "p": "n", "a": "b" }, "y": { "x:y": 1 } }"#
            );
            assert_eq!(
                add(r#"{ "@context": { "a": "|x:y", "t": { "@id": "x:z" } } }"#),
                r#"{ "@context": { "p": "n", "a": "x:y", "t": { "@id": "x:z" } } }"#
            );
            assert_eq!(
                add(r#"{ "@context": { "a": "b", "t": { "@id": "|x:z" } } }"#),
                r#"{ "@context": { "p": "n", "a": "b", "t": { "@id": "x:z" } } }"#
            );
            // Strings with brackets before the place are skipped.
            assert_eq!(
                add(r#"{ "@context": ["{", { "a": "]" }], "k": "}", "|x:y": 1 }"#),
                r#"{ "@context": ["{", { "p": "n", "a": "]" }], "k": "}", "x:y": 1 }"#
            );
            // No context yet, a URL, null and an array of URLs.
            assert_eq!(
                add(r#"{ "|x:y": 1 }"#),
                r#"{ "@context": { "p": "n" }, "x:y": 1 }"#
            );
            assert_eq!(
                add(r#"{ "@context": "c.jsonld", "|x:y": 1 }"#),
                r#"{ "@context": ["c.jsonld", { "p": "n" }], "x:y": 1 }"#
            );
            assert_eq!(
                add(r#"{ "@context": null, "|x:y": 1 }"#),
                r#"{ "@context": { "p": "n" }, "x:y": 1 }"#
            );
            assert_eq!(
                add(r#"{ "@context": ["a.jsonld", "b.jsonld"], "|x:y": 1 }"#),
                r#"{ "@context": ["a.jsonld", "b.jsonld", { "p": "n" }], "x:y": 1 }"#
            );
            // A comment between the colon and the value.
            assert_eq!(
                add("{ \"@context\": /* c */ { \"a\": 1 }, \"|x:y\": 1 }"),
                "{ \"@context\": /* c */ { \"p\": \"n\", \"a\": 1 }, \"x:y\": 1 }"
            );
        }
    }

    #[test]
    fn a_backslash_before_a_line_break_does_not_hide_the_next_line() {
        // The parser ends the unclosed string at its line; the scan must
        // too, or the quotes after it pair up wrongly and the existing
        // @context is not found (a second one would be added).
        let text = "{\n  \"a\": \"x\\\n  \"@context\": { \"k\": \"v\" },\n  \"|b\": 1\n}";
        let want =
            "{\n  \"a\": \"x\\\n  \"@context\": { \"p\": \"n\", \"k\": \"v\" },\n  \"b\": 1\n}";
        assert_eq!(add(text), want);
        assert_eq!(add_in(text), want);
        // The same in a key and with a carriage return.
        let text = "{\r\n  \"a\\\r\n  \"@context\": {},\r\n  \"|b\": 1\r\n}";
        assert_eq!(add(text).matches("@context").count(), 1, "{}", add(text));
        // The shared scan agrees with the parser's tokens.
        let b = "\"x\\\n\"y\"".as_bytes();
        assert_eq!(string_end(b, 0), (3, false));
    }

    #[test]
    fn the_index_never_targets_the_inside_of_a_json_literal() {
        // The literal holds a `@context` of its own: data, not a context.
        let text = r#"{"@context": {"j": {"@id": "http://ex.org/j", "@type": "@json"}}, "j": {"@context": {"k": "v"}, "|x:y": 1}}"#;
        // Read as text, the literal's `@context` is the nearest.
        assert_eq!(
            add(text),
            r#"{"@context": {"j": {"@id": "http://ex.org/j", "@type": "@json"}}, "j": {"@context": {"p": "n", "k": "v"}, "x:y": 1}}"#
        );
        // From the index, it is the document's.
        assert_eq!(
            add_in(text),
            r#"{"@context": {"p": "n", "j": {"@id": "http://ex.org/j", "@type": "@json"}}, "j": {"@context": {"k": "v"}, "x:y": 1}}"#
        );
        // A value object's literal too.
        let text = r#"{"@context": {"j": {"@id": "http://ex.org/j", "@type": "@json"}}, "j": {"@value": {"@context": {"k": "v"}, "|x:y": 1}, "@type": "@json"}}"#;
        assert!(add_in(text).starts_with(r#"{"@context": {"p": "n", "j""#));
    }

    #[test]
    fn text_with_no_object_to_ask_is_read_by_its_tokens() {
        // Nothing expansion could read: the scan finds the object.
        let text = r#"{ "@context": { "a": "b" }, "y": { "|x:y": "#;
        assert_eq!(
            add_in(text),
            r#"{ "@context": { "p": "n", "a": "b" }, "y": { "x:y": "#
        );
        assert_eq!(add_in(text), add(text));
        assert_eq!(objects_around("[{\"a\": [{", 9), [1, 8]);
    }
}
