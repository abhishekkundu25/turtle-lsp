//! Well-known RDF vocabularies, embedded at build time (data/vocab.tsv, generated
//! by tools/build-vocab.mjs from @zazuko/rdf-vocabularies). Vocabularies are
//! looked up by namespace IRI, never by prefix name, so `sd:` bound to your own
//! namespace is never checked against SPARQL Service Description.

use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

static DATA: &str = include_str!("../data/vocab.tsv");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TermKind {
    Class,
    Property,
    Individual,
}

#[derive(Debug)]
pub(crate) struct Term {
    pub(crate) local: String,
    pub(crate) kind: TermKind,
    pub(crate) deprecated: bool,
    pub(crate) label: String,
    pub(crate) comment: String,
    /// Direct super terms (rdfs:subClassOf / rdfs:subPropertyOf), as full IRIs.
    pub(crate) supers: Box<[String]>,
    /// rdfs:domain (schema:domainIncludes for schema.org), as full IRIs.
    pub(crate) domain: Box<[String]>,
    /// rdfs:range (schema:rangeIncludes for schema.org), as full IRIs.
    pub(crate) range: Box<[String]>,
}

pub(crate) struct Vocabulary {
    pub(crate) prefix: &'static str,
    pub(crate) namespace: &'static str,
    body: &'static str,
    terms: OnceLock<Vec<Term>>,
    index: OnceLock<FxHashMap<String, usize>>,
    /// Lowercase local names, parallel to `terms`.
    names: OnceLock<NameTable>,
}

impl Vocabulary {
    pub(crate) fn terms(&self) -> &[Term] {
        self.terms.get_or_init(|| {
            self.body
                .lines()
                .filter_map(|l| parse_term(l, self.namespace))
                .collect()
        })
    }

    pub(crate) fn term(&self, local: &str) -> Option<&Term> {
        let index = self.index.get_or_init(|| {
            self.terms()
                .iter()
                .enumerate()
                .map(|(i, t)| (t.local.clone(), i))
                .collect()
        });
        index.get(local).map(|i| &self.terms()[*i])
    }

    /// Closest defined term by edit distance, for "did you mean" fixes.
    /// Ties go to the shorter name, then to the earlier term.
    pub(crate) fn suggest(&self, local: &str) -> Option<&Term> {
        let max = (local.chars().count() / 3).clamp(1, 3);
        let table = self
            .names
            .get_or_init(|| NameTable::new(self.terms().iter().map(|t| t.local.as_str())));
        let terms = self.terms();
        let mut best: Option<(usize, usize, usize)> = None;
        table.search(local, max, &mut EditBuf::default(), |i, d| {
            let key = (d, terms[i].local.len(), i);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        });
        best.map(|(_, _, i)| &terms[i])
    }
}

pub(crate) struct Vocabularies {
    pub(crate) all: Vec<Vocabulary>,
    by_prefix: FxHashMap<&'static str, usize>,
    by_namespace: FxHashMap<&'static str, usize>,
}

pub(crate) static VOCABS: LazyLock<Vocabularies> = LazyLock::new(load);

impl Vocabularies {
    pub(crate) fn by_prefix(&self, prefix: &str) -> Option<&Vocabulary> {
        self.by_prefix.get(prefix).map(|i| &self.all[*i])
    }

    pub(crate) fn by_namespace(&self, ns: &str) -> Option<&Vocabulary> {
        self.by_namespace.get(ns).map(|i| &self.all[*i])
    }

    /// Split an IRI into (vocabulary, local name) if it falls in a known
    /// namespace (the longest one, see [`split_namespace`]).
    pub(crate) fn lookup<'a>(&self, iri: &'a str) -> Option<(&Vocabulary, &'a str)> {
        let (ns, local) = split_namespace(iri);
        self.by_namespace(ns).map(|v| (v, local))
    }
}

/// How many delimiters, counted from the right, [`split_namespace`] tries.
const SPLIT_TRIES: usize = 4;

/// Split an IRI into (namespace, local name). The namespace is the longest
/// registered vocabulary namespace that ends in `#`, `/` or `:` and starts
/// the IRI, where the local name may hold colons but no `#` or `/`, and does
/// not end in one: Open Graph's `og:audio:album` is `http://ogp.me/ns#audio:album`,
/// but `http://purl.org/dc/terms/language:` and `https://schema.org/docs/collab/X`
/// are not terms of their vocabularies. Else everything up to the last
/// delimiter. Both parts are empty and whole when the IRI has no delimiter.
pub(crate) fn split_namespace(iri: &str) -> (&str, &str) {
    let mut end = iri.len();
    let mut last = None;
    for _ in 0..SPLIT_TRIES {
        let Some(i) = iri[..end].rfind(['#', '/', ':']) else {
            break;
        };
        let (ns, local) = iri.split_at(i + 1);
        // The first try is the last delimiter, whatever follows it.
        if (last.is_none() || !local.ends_with(':')) && VOCABS.by_namespace(ns).is_some() {
            return (ns, local);
        }
        last.get_or_insert(i + 1);
        // Further left, a `#` or `/` would be inside the local name.
        if iri.as_bytes()[i] != b':' {
            break;
        }
        end = i;
    }
    match last {
        Some(cut) => iri.split_at(cut),
        None => ("", iri),
    }
}

/// Other spellings of a registered namespace: (alias, prefix of the vocabulary).
const ALIASES: &[(&str, &str)] = &[("https://schema.org/", "schema")];

fn load() -> Vocabularies {
    let mut all = Vec::new();
    // (body start, body end, prefix, namespace, has a term)
    let mut sections: Vec<(usize, usize, &str, &str, bool)> = Vec::new();
    let mut offset = 0;
    for line in DATA.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("@ns\t") {
            let mut f = rest.trim_end().split('\t');
            let (p, ns) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            if let Some(last) = sections.last_mut() {
                last.1 = offset;
            }
            sections.push((offset + line.len(), DATA.len(), p, ns, false));
        } else if line.starts_with("term\t")
            && let Some(last) = sections.last_mut()
        {
            last.4 = true;
        }
        offset += line.len();
    }
    for (start, end, prefix, namespace, has_terms) in sections {
        // A vocabulary with no terms (GeoSPARQL's functions) would report
        // every term used from it as unknown.
        if !has_terms {
            continue;
        }
        all.push(Vocabulary {
            prefix,
            namespace,
            body: &DATA[start..end],
            terms: OnceLock::new(),
            index: OnceLock::new(),
            names: OnceLock::new(),
        });
    }
    let by_prefix: FxHashMap<&'static str, usize> =
        all.iter().enumerate().map(|(i, v)| (v.prefix, i)).collect();
    let mut by_namespace = FxHashMap::default();
    for (i, v) in all.iter().enumerate() {
        by_namespace.entry(v.namespace).or_insert(i);
    }
    for (alias, prefix) in ALIASES {
        if let Some(&i) = by_prefix.get(prefix) {
            by_namespace.entry(alias).or_insert(i);
        }
    }
    Vocabularies {
        all,
        by_prefix,
        by_namespace,
    }
}

fn parse_term(line: &str, namespace: &str) -> Option<Term> {
    let mut f = line.strip_prefix("term\t")?.split('\t');
    let local = unescape(f.next()?);
    let kind = match f.next()? {
        "C" => TermKind::Class,
        "P" => TermKind::Property,
        _ => TermKind::Individual,
    };
    let deprecated = f.next()? == "D";
    let label = unescape(f.next().unwrap_or(""));
    let comment = unescape(f.next().unwrap_or(""));
    let mut list = || terms_of(f.next().unwrap_or(""), namespace);
    let (supers, domain, range) = (list(), list(), list());
    Some(Term {
        local,
        kind,
        deprecated,
        label,
        comment,
        supers,
        domain,
        range,
    })
}

/// A space-separated term list as written by tools/build-vocab.mjs: `local`
/// (same namespace), `prefix:local` (another vocabulary) or `<iri>`.
fn terms_of(field: &str, namespace: &str) -> Box<[String]> {
    field
        .split(' ')
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            if let Some(iri) = t.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
                return Some(iri.to_string());
            }
            match t.split_once(':') {
                Some((p, local)) => VOCABS
                    .by_prefix(p)
                    .map(|v| format!("{}{local}", v.namespace)),
                None => Some(format!("{namespace}{t}")),
            }
        })
        .collect()
}

fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// [`EditBuf::within`] with no bound, for the tests.
#[cfg(test)]
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    EditBuf::default().within(&a, &b, usize::MAX).unwrap_or(0)
}

/// The rows of the edit-distance table, kept between calls so a search over
/// many names allocates once.
#[derive(Default)]
pub(crate) struct EditBuf {
    before: Vec<usize>,
    prev: Vec<usize>,
    cur: Vec<usize>,
}

impl EditBuf {
    /// The edit distance of `a` and `b` if it is at most `max`, else `None`,
    /// giving up as soon as two rows in a row are all above `max`. The
    /// distance allows adjacent transpositions (optimal string alignment):
    /// insertions, deletions, substitutions and swapping two neighbouring
    /// characters each cost 1, so `lable` is 1 from `label`.
    pub(crate) fn within(&mut self, a: &[char], b: &[char], max: usize) -> Option<usize> {
        // Edit distance is at least the difference in length.
        if a.len().abs_diff(b.len()) > max {
            return None;
        }
        let Self { before, prev, cur } = self;
        before.clear();
        before.resize(b.len() + 1, 0);
        prev.clear();
        prev.extend(0..=b.len());
        cur.clear();
        cur.resize(b.len() + 1, 0);
        let mut prev_min = 0;
        for i in 0..a.len() {
            cur[0] = i + 1;
            let mut row_min = cur[0];
            for j in 0..b.len() {
                let mut d = (prev[j + 1] + 1)
                    .min(cur[j] + 1)
                    .min(prev[j] + usize::from(a[i] != b[j]));
                if i > 0 && j > 0 && a[i] == b[j - 1] && a[i - 1] == b[j] {
                    d = d.min(before[j - 1] + 1);
                }
                cur[j + 1] = d;
                row_min = row_min.min(d);
            }
            // A path to the last cell passes through one of any two
            // neighbouring rows (a swap skips one) and never gets cheaper.
            if row_min > max && prev_min > max {
                return None;
            }
            prev_min = row_min;
            std::mem::swap(before, prev);
            std::mem::swap(prev, cur);
        }
        Some(prev[b.len()]).filter(|d| *d <= max)
    }
}

/// Lowercased names, bucketed by length, to search for the ones near a
/// query. Built once from a set of names; each search then skips every name
/// whose length or letters rule it out before any distance is computed.
pub(crate) struct NameTable {
    chars: Vec<char>,
    /// Per name: start in `chars`, length, and a letter signature.
    spans: Vec<(u32, u32, u64)>,
    by_len: BTreeMap<u32, Vec<u32>>,
}

/// One bit per (hashed) character: names far apart share few bits.
fn signature(chars: &[char]) -> u64 {
    chars.iter().fold(0, |m, c| {
        m | 1 << (u32::from(*c).wrapping_mul(0x9E37_79B1) >> 26)
    })
}

fn lowered(s: &str, out: &mut Vec<char>) {
    if s.is_ascii() {
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
    } else {
        out.extend(s.to_lowercase().chars());
    }
}

impl NameTable {
    /// The table of `names`; a name's number is its position here.
    pub(crate) fn new<'a>(names: impl Iterator<Item = &'a str>) -> Self {
        let mut table = NameTable {
            chars: Vec::new(),
            spans: Vec::new(),
            by_len: BTreeMap::new(),
        };
        for (i, name) in names.enumerate() {
            let start = table.chars.len();
            lowered(name, &mut table.chars);
            let len = table.chars.len() - start;
            let sig = signature(&table.chars[start..]);
            table.spans.push((start as u32, len as u32, sig));
            table.by_len.entry(len as u32).or_default().push(i as u32);
        }
        table
    }

    /// Calls `found(name number, distance)` for every name at edit distance
    /// at most `max` from `query` (compared in lowercase), in no set order.
    pub(crate) fn search(
        &self,
        query: &str,
        max: usize,
        buf: &mut EditBuf,
        mut found: impl FnMut(usize, usize),
    ) {
        let mut q = Vec::with_capacity(query.len());
        lowered(query, &mut q);
        let sig = signature(&q);
        let lo = (q.len().saturating_sub(max)).min(u32::MAX as usize) as u32;
        let hi = (q.len().saturating_add(max)).min(u32::MAX as usize) as u32;
        for ids in self.by_len.range(lo..=hi).map(|(_, ids)| ids) {
            for &i in ids {
                let (start, len, s) = self.spans[i as usize];
                // Each edit removes at most one kind of character from either side.
                if (sig & !s).count_ones() as usize > max || (s & !sig).count_ones() as usize > max
                {
                    continue;
                }
                let name = &self.chars[start as usize..(start + len) as usize];
                if let Some(d) = buf.within(&q, name, max) {
                    found(i as usize, d);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swap_of_neighbours_is_one_edit() {
        assert_eq!(edit_distance("lable", "label"), 1);
        assert_eq!(edit_distance("ab", "ba"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("ca", "abc"), 3);
    }

    #[test]
    fn a_bounded_distance_is_the_distance_or_none() {
        let words = [
            "",
            "a",
            "ab",
            "ba",
            "label",
            "lable",
            "labels",
            "lbl",
            "Étiquette",
            "etiquette",
            "audio:album",
            "audio:albm",
            "pump-12",
            "pump-13",
            "kitten",
            "sitting",
            "ca",
            "abc",
        ];
        let mut buf = EditBuf::default();
        for a in words {
            for b in words {
                let (ca, cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
                let d = edit_distance(a, b);
                for max in 0..5 {
                    assert_eq!(
                        buf.within(&ca, &cb, max),
                        (d <= max).then_some(d),
                        "{a} {b} {max}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_name_table_finds_what_a_full_scan_finds() {
        let names: Vec<String> = VOCABS
            .by_prefix("foaf")
            .unwrap()
            .terms()
            .iter()
            .map(|t| t.local.clone())
            .chain(["Étiquette".to_string(), "ÉTIQUETTE2".to_string()])
            .collect();
        let table = NameTable::new(names.iter().map(String::as_str));
        let mut buf = EditBuf::default();
        for probe in [
            "nmae",
            "Name",
            "etiquette",
            "mbox_sha1sumx",
            "x",
            "",
            "KNOWS",
        ] {
            for max in 1..=3 {
                let mut got = Vec::new();
                table.search(probe, max, &mut buf, |i, d| got.push((i, d)));
                got.sort_unstable();
                let want: Vec<(usize, usize)> = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (i, edit_distance(&probe.to_lowercase(), &n.to_lowercase())))
                    .filter(|(_, d)| *d <= max)
                    .collect();
                assert_eq!(got, want, "{probe} {max}");
            }
        }
    }

    #[test]
    fn every_vocabulary_has_terms() {
        for v in &VOCABS.all {
            assert!(!v.terms().is_empty(), "{} has no terms", v.prefix);
        }
        // GeoSPARQL's function and rule namespaces carry none: not registered.
        assert!(VOCABS.by_prefix("geof").is_none());
        assert!(
            VOCABS
                .lookup("http://www.opengis.net/def/function/geosparql/sfWithin")
                .is_none()
        );
    }

    #[test]
    fn a_local_name_may_hold_a_colon() {
        let og = "http://ogp.me/ns#";
        assert_eq!(
            split_namespace("http://ogp.me/ns#audio:album"),
            (og, "audio:album")
        );
        let (v, local) = VOCABS.lookup("http://ogp.me/ns#audio:album").unwrap();
        assert_eq!((v.prefix, local), ("og", "audio:album"));
        assert!(v.term(local).is_some());
        assert!(v.term("audio:albm").is_none());
        assert_eq!(v.suggest("audio:albm").unwrap().local, "audio:album");
        // Unregistered namespaces still split at the last delimiter.
        assert_eq!(
            split_namespace("http://example.org/ns#a:b"),
            ("http://example.org/ns#a:", "b")
        );
        assert_eq!(split_namespace("urn:x"), ("urn:", "x"));
        // A trailing colon does not make a local name, nor does a path.
        assert!(
            VOCABS
                .lookup("http://purl.org/dc/terms/language:")
                .is_none()
        );
        assert!(VOCABS.lookup("http://ogp.me/ns#audio:").is_none());
        assert!(
            VOCABS
                .lookup("https://schema.org/docs/collab/MBZ")
                .is_none()
        );
        assert!(VOCABS.lookup("http://xmlns.com/foaf/0.1/a/b").is_none());
        assert_eq!(split_namespace("nodelimiter"), ("", "nodelimiter"));
        assert_eq!(
            split_namespace("http://xmlns.com/foaf/0.1/"),
            ("http://xmlns.com/foaf/0.1/", "")
        );
    }

    #[test]
    fn https_schema_org_is_schema() {
        let (v, local) = VOCABS.lookup("https://schema.org/Person").unwrap();
        assert_eq!((v.prefix, local), ("schema", "Person"));
        assert!(v.term(local).is_some());
        assert_eq!(
            VOCABS.by_namespace("https://schema.org/").unwrap().prefix,
            "schema"
        );
        assert_eq!(
            VOCABS.lookup("http://schema.org/Person").unwrap().0.prefix,
            "schema"
        );
    }

    #[test]
    fn foaf_by_namespace() {
        let (v, local) = VOCABS.lookup("http://xmlns.com/foaf/0.1/Person").unwrap();
        assert_eq!(v.prefix, "foaf");
        assert_eq!(v.term(local).unwrap().kind, TermKind::Class);
        assert_eq!(v.suggest("Persn").unwrap().local, "Person");
        assert_eq!(v.suggest("Preson").unwrap().local, "Person");
    }

    #[test]
    fn terms_carry_domain_range_and_supers() {
        let (v, local) = VOCABS.lookup("http://xmlns.com/foaf/0.1/knows").unwrap();
        let t = v.term(local).unwrap();
        assert_eq!(&*t.range, ["http://xmlns.com/foaf/0.1/Person"]);
        assert_eq!(&*t.domain, ["http://xmlns.com/foaf/0.1/Person"]);
        let (v, local) = VOCABS.lookup("http://xmlns.com/foaf/0.1/Person").unwrap();
        let supers = &v.term(local).unwrap().supers;
        assert!(
            supers
                .iter()
                .any(|s| s == "http://xmlns.com/foaf/0.1/Agent"),
            "{supers:?}"
        );
    }

    #[test]
    fn suggest_matches_the_unpruned_search() {
        let full = |v: &'static Vocabulary, local: &str| {
            let lower = local.to_lowercase();
            let max = (local.chars().count() / 3).clamp(1, 3);
            v.terms()
                .iter()
                .map(|t| (edit_distance(&lower, &t.local.to_lowercase()), t))
                .filter(|(d, _)| *d <= max)
                .min_by_key(|(d, t)| (*d, t.local.len()))
                .map(|(_, t)| t.local.as_str())
        };
        for v in &VOCABS.all {
            for t in v.terms().iter().step_by(7).take(40) {
                let l = &t.local;
                for probe in [
                    format!("{l}x"),
                    l.chars().skip(1).collect::<String>(),
                    l.to_uppercase(),
                    format!("{l}Zzz"),
                    l.chars().rev().collect::<String>(),
                ] {
                    assert_eq!(
                        v.suggest(&probe).map(|t| t.local.as_str()),
                        full(v, &probe),
                        "{}:{probe}",
                        v.prefix
                    );
                }
            }
        }
    }

    #[test]
    fn custom_namespace_is_unknown() {
        assert!(
            VOCABS
                .lookup("https://ontology.example.com/core/fillsSlot")
                .is_none()
        );
    }
}
