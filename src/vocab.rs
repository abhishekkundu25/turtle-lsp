//! Well-known RDF vocabularies, embedded at build time (data/vocab.tsv, generated
//! by tools/build-vocab.mjs from @zazuko/rdf-vocabularies). Vocabularies are
//! looked up by namespace IRI, never by prefix name, so `sd:` bound to your own
//! namespace is never checked against SPARQL Service Description.

use rustc_hash::FxHashMap;
use std::sync::{LazyLock, OnceLock};

static DATA: &str = include_str!("../data/vocab.tsv");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKind {
    Class,
    Property,
    Individual,
}

#[derive(Debug)]
pub struct Term {
    pub local: String,
    pub kind: TermKind,
    pub deprecated: bool,
    pub label: String,
    pub comment: String,
    /// Direct super terms (rdfs:subClassOf / rdfs:subPropertyOf), as full IRIs.
    pub supers: Box<[String]>,
    /// rdfs:domain (schema:domainIncludes for schema.org), as full IRIs.
    pub domain: Box<[String]>,
    /// rdfs:range (schema:rangeIncludes for schema.org), as full IRIs.
    pub range: Box<[String]>,
}

pub struct Vocabulary {
    pub prefix: &'static str,
    pub namespace: &'static str,
    body: &'static str,
    terms: OnceLock<Vec<Term>>,
    index: OnceLock<FxHashMap<String, usize>>,
    /// Lowercase local name and its char count, parallel to `terms`.
    lower: OnceLock<Vec<(String, usize)>>,
}

impl Vocabulary {
    pub fn terms(&self) -> &[Term] {
        self.terms.get_or_init(|| {
            self.body
                .lines()
                .filter_map(|l| parse_term(l, self.namespace))
                .collect()
        })
    }

    pub fn term(&self, local: &str) -> Option<&Term> {
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
    pub fn suggest(&self, local: &str) -> Option<&Term> {
        let lower = local.to_lowercase();
        let n = lower.chars().count();
        let max = (local.chars().count() / 3).clamp(1, 3);
        let names = self.lower.get_or_init(|| {
            self.terms()
                .iter()
                .map(|t| {
                    let l = t.local.to_lowercase();
                    let c = l.chars().count();
                    (l, c)
                })
                .collect()
        });
        self.terms()
            .iter()
            .zip(names)
            // Edit distance is at least the difference in length.
            .filter(|(_, (_, c))| c.abs_diff(n) <= max)
            .map(|(t, (l, _))| (edit_distance(&lower, l), t))
            .filter(|(d, _)| *d <= max)
            .min_by_key(|(d, t)| (*d, t.local.len()))
            .map(|(_, t)| t)
    }
}

pub struct Vocabularies {
    pub all: Vec<Vocabulary>,
    by_prefix: FxHashMap<&'static str, usize>,
    by_namespace: FxHashMap<&'static str, usize>,
}

pub static VOCABS: LazyLock<Vocabularies> = LazyLock::new(load);

impl Vocabularies {
    pub fn by_prefix(&self, prefix: &str) -> Option<&Vocabulary> {
        self.by_prefix.get(prefix).map(|i| &self.all[*i])
    }

    pub fn by_namespace(&self, ns: &str) -> Option<&Vocabulary> {
        self.by_namespace.get(ns).map(|i| &self.all[*i])
    }

    /// Split an IRI into (vocabulary, local name) if it falls in a known namespace.
    pub fn lookup<'a>(&self, iri: &'a str) -> Option<(&Vocabulary, &'a str)> {
        let cut = iri.rfind(['#', '/', ':'])? + 1;
        let (ns, local) = iri.split_at(cut);
        self.by_namespace(ns).map(|v| (v, local))
    }
}

fn load() -> Vocabularies {
    let mut all = Vec::new();
    let mut sections: Vec<(usize, usize, &str, &str)> = Vec::new();
    let mut offset = 0;
    for line in DATA.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("@ns\t") {
            let mut f = rest.trim_end().split('\t');
            let (p, ns) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            if let Some(last) = sections.last_mut() {
                last.1 = offset;
            }
            sections.push((offset + line.len(), DATA.len(), p, ns));
        }
        offset += line.len();
    }
    for (start, end, prefix, namespace) in sections {
        all.push(Vocabulary {
            prefix,
            namespace,
            body: &DATA[start..end],
            terms: OnceLock::new(),
            index: OnceLock::new(),
            lower: OnceLock::new(),
        });
    }
    let by_prefix = all.iter().enumerate().map(|(i, v)| (v.prefix, i)).collect();
    let mut by_namespace = FxHashMap::default();
    for (i, v) in all.iter().enumerate() {
        by_namespace.entry(v.namespace).or_insert(i);
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

/// Edit distance with adjacent transpositions (optimal string alignment):
/// insertions, deletions, substitutions and swapping two neighbouring
/// characters each cost 1, so `lable` is 1 from `label`.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 0..a.len() {
        cur[0] = i + 1;
        for j in 0..b.len() {
            let mut d = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + usize::from(a[i] != b[j]));
            if i > 0 && j > 0 && a[i] == b[j - 1] && a[i - 1] == b[j] {
                d = d.min(before[j - 1] + 1);
            }
            cur[j + 1] = d;
        }
        std::mem::swap(&mut before, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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
