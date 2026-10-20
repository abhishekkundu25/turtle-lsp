//! Adding a member to the nearest `@context` of a place in a JSON-LD text,
//! as completion does when it accepts a term of a prefix the context lacks
//! and the `jsonld-undefined-prefix` quick fix does. The text is scanned as
//! JSON that may be half typed (an unclosed string ends at its line), so
//! this works on text that does not parse.

/// Where a string starting with the `"` at `open` ends: its closing quote,
/// or (unclosed) the end of its line. The flag says whether it closed.
pub(crate) fn string_end(b: &[u8], open: usize) -> (usize, bool) {
    let mut j = open + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return (j, true),
            b'\n' | b'\r' => return (j, false),
            _ => j += 1,
        }
    }
    (b.len(), false)
}

/// Stands for the `"prefix": "namespace"` member in [`context_edit`]'s text.
pub const MEMBER: &str = "\u{0}member\u{0}";

/// The edit adding a member to the nearest `@context` object, as (offset,
/// length replaced, text with [`MEMBER`] for the member): the `@context` of
/// the innermost object around the cursor that has one, else a new
/// `@context` in the outermost object.
pub fn context_edit(text: &str, objects: &[usize], offset: usize) -> (usize, usize, String) {
    let b = text.as_bytes();
    for &o in objects.iter().rev() {
        let Some(v) = member_value(b, o, "@context") else {
            continue;
        };
        match b[v] {
            b'{' => return insert_member(b, v, MEMBER),
            b'[' => {
                let (objs, close) = array_elements(b, v);
                if let Some(&last) = objs.last() {
                    return insert_member(b, last, MEMBER);
                }
                let sep = if has_element(b, v, close) { ", " } else { "" };
                return (close, 0, format!("{sep}{{ {MEMBER} }}"));
            }
            b'"' => {
                let (end, closed) = string_end(b, v);
                let end = if closed { end + 1 } else { end };
                return (v, end - v, format!("[{}, {{ {MEMBER} }}]", &text[v..end]));
            }
            _ if text[v..].starts_with("null") => {
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

/// Where the value of the member `name` of the object at `brace` starts.
fn member_value(b: &[u8], brace: usize, name: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = brace + 1;
    let mut last_key: Option<(usize, usize)> = None;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let (end, _) = string_end(b, i);
                if depth == 0 {
                    last_key = Some((i + 1, end));
                }
                i = end + 1;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            b':' if depth == 0 => {
                if last_key.is_some_and(|(s, e)| &b[s..e] == name.as_bytes()) {
                    let mut v = i + 1;
                    while v < b.len() && b[v].is_ascii_whitespace() {
                        v += 1;
                    }
                    return (v < b.len()).then_some(v);
                }
            }
            b',' if depth == 0 => last_key = None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The objects directly in the array at `open`, and where it closes (its
/// `]`, or the end of the text).
fn array_elements(b: &[u8], open: usize) -> (Vec<usize>, usize) {
    let mut objs = Vec::new();
    let mut depth = 0usize;
    let mut i = open + 1;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = string_end(b, i).0 + 1;
                continue;
            }
            b'{' | b'[' => {
                if depth == 0 && b[i] == b'{' {
                    objs.push(i);
                }
                depth += 1;
            }
            b'}' | b']' => {
                if depth == 0 {
                    return (objs, i);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    (objs, b.len())
}

fn has_element(b: &[u8], open: usize, close: usize) -> bool {
    b[open + 1..close].iter().any(|c| !c.is_ascii_whitespace())
}

/// The `{` of each object around `offset`, outermost first.
pub fn objects_around(text: &str, offset: usize) -> Vec<usize> {
    let b = text.as_bytes();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut i = 0;
    while i < offset.min(b.len()) {
        match b[i] {
            b'"' => {
                i = string_end(b, i).0 + 1;
                continue;
            }
            c @ (b'{' | b'[') => stack.push((c, i)),
            b'}' | b']' => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
    stack
        .into_iter()
        .filter(|(c, _)| *c == b'{')
        .map(|(_, i)| i)
        .collect()
}

/// [`context_edit`] for the place at `offset` with `member` (`"name":
/// value`), as (offset, length replaced, new text).
pub fn add_member(text: &str, offset: usize, member: &str) -> (usize, usize, String) {
    let (at, del, new_text) = context_edit(text, &objects_around(text, offset), offset);
    (at, del, new_text.replace(MEMBER, member))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(text: &str) -> String {
        let offset = text.find('|').unwrap();
        let text = text.replace('|', "");
        let (at, del, new) = add_member(&text, offset, r#""p": "n""#);
        format!("{}{}{}", &text[..at], new, &text[at + del..])
    }

    #[test]
    fn adds_to_the_context_around_a_string() {
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
    }
}
