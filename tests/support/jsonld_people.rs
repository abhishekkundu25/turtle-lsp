// The people document of the JSON-LD performance budgets, from
// `bench/jsonld-people.tpl`: one `@graph` of people under an inline
// `@context` (no remote contexts, so it parses offline), using what real
// files lean on: compact IRIs, `@type`, `@id` references, typed and
// language-tagged values, nested blank nodes and `@list`. The template is
// the one description of the document: `bench/jsonld-gen.mjs` reads it too.
// Included with `include!`; not a test crate of its own.

const PEOPLE_TEMPLATE: &str = include_str!("../../bench/jsonld-people.tpl");

/// The head, one person and the tail of [`PEOPLE_TEMPLATE`], split at its
/// `@@` lines.
fn people_parts() -> (&'static str, &'static str, &'static str) {
    let mut parts = PEOPLE_TEMPLATE.split("@@\n");
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(head), Some(person), Some(tail), None) => (head, person, tail),
        _ => panic!("bench/jsonld-people.tpl needs a head, a person and a tail between @@ lines"),
    }
}

/// How many people make a document of about `lines` lines.
fn people_count(lines: usize) -> usize {
    let (head, person, tail) = people_parts();
    let count = |s: &str| s.lines().count();
    ((lines.saturating_sub(count(head) + count(tail)) as f64 / count(person) as f64).round()
        as usize)
        .max(1)
}

/// What person `i` of `n` has that varies: the birth year's last two
/// digits, the two people they know, the town number and the last skill.
fn person_values(i: usize, n: usize) -> (String, usize, usize, usize, usize) {
    (
        format!("{:02}", 50 + i % 50),
        (i + 1) % n,
        (i + 7) % n,
        i % 100,
        i % 13,
    )
}

/// A pretty-printed document of about `lines` lines; `tag` makes the IRIs
/// unique per document.
fn people_jsonld(lines: usize, tag: &str) -> String {
    let (head, person, tail) = people_parts();
    let n = people_count(lines);
    let mut out = String::from(head);
    for i in 0..n {
        let (born, k1, k2, town, skill) = person_values(i, n);
        out += person
            .replace("%TAG%", tag)
            .replace("%I%", &i.to_string())
            .replace("%BORN%", &born)
            .replace("%K1%", &k1.to_string())
            .replace("%K2%", &k2.to_string())
            .replace("%TOWN%", &town.to_string())
            .replace("%SKILL%", &skill.to_string())
            .trim_end_matches('\n');
        out += if i + 1 < n { ",\n" } else { "\n" };
    }
    out + tail
}
