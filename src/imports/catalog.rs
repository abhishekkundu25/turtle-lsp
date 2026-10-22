//! `catalog-v001.xml` (Protégé convention): IRI to file or URL mappings.

use super::load::names_host;
use super::*;

// ---- catalogs ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Url(String),
}

/// `<uri name=... uri=...>` entries of every catalog-v001.xml under the roots;
/// the first catalog (by path) that maps an IRI wins.
#[derive(Default)]
pub struct Catalog {
    map: FxHashMap<String, (Target, PathBuf)>,
}

impl Catalog {
    pub fn scan(roots: &[PathBuf]) -> Catalog {
        let mut files = Vec::new();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        let mut seen = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                seen += 1;
                if seen > MAX_FILES * 4 {
                    stack.clear();
                    break;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push(e.path());
                    }
                } else if ft.is_file() && name == CATALOG_FILE {
                    files.push(e.path());
                }
            }
        }
        files.sort();
        let mut c = Catalog::default();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let dir = f.parent().unwrap_or(Path::new(""));
            for (name, target) in parse_catalog(&text, dir) {
                c.map
                    .entry(norm(&name).to_string())
                    .or_insert((target, f.clone()));
            }
        }
        c
    }

    pub(super) fn lookup(&self, iri: &str) -> Option<(&Target, &PathBuf)> {
        self.map.get(norm(iri)).map(|(t, f)| (t, f))
    }
}

/// The `<uri name="..." uri="..."/>` entries of an XML catalog, with targets
/// resolved against `dir`. Comments are skipped; anything else is ignored.
pub fn parse_catalog(text: &str, dir: &Path) -> Vec<(String, Target)> {
    let mut out = Vec::new();
    let base = crate::workspace::file_url_of(dir, true);
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix("!--") {
            rest = r.find("-->").map_or("", |j| &r[j + 3..]);
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = &rest[end..];
        let Some(attrs) = tag.strip_prefix("uri") else {
            continue;
        };
        if !attrs.starts_with(char::is_whitespace) {
            continue;
        }
        let attrs = attributes(attrs);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let (Some(name), Some(uri)) = (get("name"), get("uri")) else {
            continue;
        };
        let target = match url::Url::parse(&uri) {
            Ok(u) if u.scheme() == "file" => file_target(&u, &uri),
            // A one-letter scheme is a Windows drive.
            Ok(u) if u.scheme().len() > 1 => Some(Target::Url(uri)),
            _ => match base.as_ref().and_then(|b| b.join(&uri).ok()) {
                // `//server/share/x.ttl` is a reference to a host.
                Some(u) if u.scheme() == "file" && names_host(&u).is_some() => {
                    Some(Target::Url(u.to_string()))
                }
                joined => joined
                    .and_then(|u| crate::workspace::file_path_of(&u))
                    .or_else(|| Some(dir.join(&uri)))
                    .map(Target::File),
            },
        };
        if let Some(t) = target {
            out.push((name, t));
        }
    }
    out
}

/// The target of a `file:` URL: its file, or, when it names a host, the URL
/// itself, which loading refuses (see `fs_policy`) with an answer rather
/// than a mapping that silently goes nowhere, or, on Windows, to a share.
fn file_target(u: &url::Url, uri: &str) -> Option<Target> {
    if names_host(u).is_some() {
        return Some(Target::Url(uri.to_string()));
    }
    crate::workspace::file_path_of(u).map(Target::File)
}

/// Length of a tag's content up to its closing `>` (quotes respected).
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if q == c => quote = None,
            (None, '>') => return i,
            _ => {}
        }
    }
    s.len()
}

/// `name="value"` pairs of a tag, values unescaped.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(close) = after[1..].find(q) else {
            break;
        };
        out.push((name, unescape(&after[1..1 + close])));
        rest = &after[close + 2..];
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_resolve_relative_to_the_catalog() {
        let dir = Path::new(if cfg!(windows) { "C:\\p\\q" } else { "/p/q" });
        let xml = r#"<?xml version="1.0"?>
<catalog prefer="public" xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <!-- <uri name="http://ex.com/hidden" uri="no.ttl"/> -->
  <uri id="Imports Wizard Entry" name="http://ex.com/a" uri="vendor/a%20b.ttl"/>
  <uri name='http://ex.com/b?x=1&amp;y=2' uri="../b.owl" />
  <uri name="http://ex.com/c" uri="https://mirror.example.com/c.ttl"/>
  <uriSuffix uriSuffix="x" uri="y"/>
</catalog>"#;
        let got = parse_catalog(xml, dir);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].0, "http://ex.com/a");
        assert_eq!(got[0].1, Target::File(dir.join("vendor").join("a b.ttl")));
        assert_eq!(got[1].0, "http://ex.com/b?x=1&y=2");
        assert_eq!(got[1].1, Target::File(dir.parent().unwrap().join("b.owl")));
        assert_eq!(
            got[2].1,
            Target::Url("https://mirror.example.com/c.ttl".into())
        );
        // Garbage never panics.
        for s in [
            "<uri",
            "<uri name=\"a",
            "<uri name='a' uri=",
            "&#xZZ;<uri name=\"&#x41;\" uri=\"x\">",
        ] {
            let _ = parse_catalog(s, dir);
        }
        assert_eq!(unescape("a&lt;b&#65;&#x42;&bogus;"), "a<bAB&bogus;");
    }
}
