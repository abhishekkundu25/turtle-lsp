//! A proof as hover markdown: a nested list under the inferred type, each
//! rule step naming its W3C rule (`cax-sco`, `eq-rep-s`), each base triple a
//! link to where it is written. A step the list has shown already is
//! written again as "shown above", as TurbOWL's own tree writes it, and a
//! long proof is cut after [`MAX_ITEMS`] items.

use super::{Origin, Proof};
use crate::syntax::RDF_TYPE;
use spargebra::term::Term;
use turbowl::{Reason, Statement};

/// The most items a proof's list has; the rest are left out, and said so.
const MAX_ITEMS: usize = 30;

/// How the hover writes what a proof names.
pub(crate) struct Names<'a> {
    /// An IRI, as `prefix:local` or `<iri>`.
    pub(crate) iri: &'a dyn Fn(&str) -> String,
    /// Where a base triple is written: a link to its file and line.
    pub(crate) place: &'a dyn Fn(&Origin) -> String,
}

impl Proof {
    /// The proof as a markdown list item: `title`, the goal's rule, and
    /// below it, indented, the steps it rests on.
    pub(crate) fn markdown(&self, title: &str, names: &Names<'_>) -> String {
        let steps = self.proof.steps();
        let mut w = Writer {
            proof: self,
            names,
            out: String::new(),
            shown: vec![false; steps.len()],
            items: 0,
        };
        let goal = steps.len() - 1;
        w.out.push_str(&format!("- {title}"));
        w.reason(goal, 0);
        if w.items > MAX_ITEMS {
            w.out.push_str("\n  - _and more steps_");
        }
        w.out
    }
}

struct Writer<'p, 'n> {
    proof: &'p Proof,
    names: &'p Names<'n>,
    out: String,
    shown: Vec<bool>,
    /// Items below the goal; past [`MAX_ITEMS`] they are counted, not
    /// written.
    items: usize,
}

impl Writer<'_, '_> {
    /// After step `i`'s statement: why it holds, and its premises at
    /// `depth + 1`.
    fn reason(&mut self, i: usize, depth: usize) {
        let step = &self.proof.proof.steps()[i];
        match &step.reason {
            Reason::Base => match &self.proof.origins[i] {
                Some(o) => self.out.push_str(&format!(" {}", (self.names.place)(o))),
                None => self.out.push_str(" · _base_"),
            },
            Reason::Rule { rule, .. } if self.shown[i] => {
                self.out.push_str(&format!(" · _{rule}, shown above_"));
            }
            Reason::Rule { rule, premises } => {
                self.out.push_str(&format!(" · _{rule}_"));
                self.shown[i] = true;
                for &p in premises {
                    self.step(p, depth + 1);
                }
            }
        }
    }

    /// Step `i` as an item at `depth`.
    fn step(&mut self, i: usize, depth: usize) {
        self.items += 1;
        if self.items > MAX_ITEMS {
            return;
        }
        let statement = statement(&self.proof.proof.steps()[i].statement, self.names);
        self.out.push_str(&format!(
            "\n{:indent$}- {statement}",
            "",
            indent = 2 * depth
        ));
        self.reason(i, depth);
    }
}

/// A statement as code: `ex:rex a ex:Dog`.
fn statement(s: &Statement, names: &Names<'_>) -> String {
    let predicate = match &s.predicate {
        Term::NamedNode(n) if n.as_str() == RDF_TYPE => "a".to_string(),
        p => term(p, names),
    };
    code(&format!(
        "{} {predicate} {}",
        term(&s.subject, names),
        term(&s.object, names)
    ))
}

fn term(t: &Term, names: &Names<'_>) -> String {
    match t {
        Term::NamedNode(n) => (names.iri)(n.as_str()),
        // The base's names for blank nodes are its own, not the user's.
        Term::BlankNode(_) => "[]".into(),
        Term::Literal(l) => {
            let lexical = spargebra::term::Literal::new_simple_literal(l.value()).to_string();
            if let Some(lang) = l.language() {
                format!("{lexical}@{lang}")
            } else if l.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                lexical
            } else {
                format!("{lexical}^^{}", (names.iri)(l.datatype().as_str()))
            }
        }
        Term::Triple(t) => format!(
            "<<( {} {} {} )>>",
            term(&t.subject.clone().into(), names),
            (names.iri)(t.predicate.as_str()),
            term(&t.object, names)
        ),
    }
}

/// `s` as a markdown code span, fenced past any backticks it holds.
fn code(s: &str) -> String {
    let longest = s
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest + 1);
    if longest == 0 {
        format!("{fence}{s}{fence}")
    } else {
        format!("{fence} {s} {fence}")
    }
}

#[cfg(test)]
mod tests {
    use super::super::base::tests::doc;
    use super::super::{Inferred, Job, Shared, Types, run_inline};
    use super::*;
    use crate::analysis::{Analysis, Encoding};
    use crate::knowledge::Source;
    use crate::settings::Reasoning;
    use std::sync::Arc;
    use std::time::Duration;

    const EX: &str = "http://example.org/";

    /// The inferred types of `ex:<local>` after a run over `docs`.
    fn inferred(docs: Vec<(Source, Arc<Analysis>)>, local: &str) -> Vec<Inferred> {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        run_inline(
            &shared,
            &Job {
                generation: 1,
                docs,
                rules: Reasoning::Default,
                max_triples: 1000,
                enc: Encoding::Utf16,
            },
        );
        match shared.inferred_types(&format!("{EX}{local}"), Duration::from_secs(10)) {
            Types::Proved(v) => v,
            _ => panic!("not proved"),
        }
    }

    fn written(t: &Inferred) -> String {
        let iri = |i: &str| match i.strip_prefix(EX) {
            Some(local) => format!("ex:{local}"),
            None => crate::features::hover::short(i, &Default::default()),
        };
        let place = |o: &Origin| format!("[{}:{}]", o.uri, o.line);
        let names = Names {
            iri: &iri,
            place: &place,
        };
        let title = format!("**{}**", iri(&t.class));
        t.proof.as_ref().expect("a proof").markdown(&title, &names)
    }

    #[test]
    fn a_proof_is_a_nested_list_with_its_rules_and_where_each_base_triple_is_written() {
        let d = doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:Dog rdfs:subClassOf ex:Mammal .\n\
             ex:Mammal rdfs:subClassOf ex:Animal .\n\
             ex:rex a ex:Dog .\n",
        );
        let types = inferred(vec![d], "rex");
        let animal = types.iter().find(|t| t.class == format!("{EX}Animal"));
        let md = written(animal.unwrap());
        assert_eq!(
            md,
            "- **ex:Animal** · _cax-sco_\n  \
             - `ex:Dog rdfs:subClassOf ex:Animal` · _scm-sco_\n    \
             - `ex:Dog rdfs:subClassOf ex:Mammal` [file:///w/a.ttl:3]\n    \
             - `ex:Mammal rdfs:subClassOf ex:Animal` [file:///w/a.ttl:4]\n  \
             - `ex:rex a ex:Dog` [file:///w/a.ttl:5]",
        );
    }

    #[test]
    fn a_type_through_same_as_shows_the_chain_of_links() {
        let d = doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             ex:a owl:sameAs ex:b .\n\
             ex:b owl:sameAs ex:c .\n\
             ex:c a ex:C .\n",
        );
        let types = inferred(vec![d], "a");
        let c = types.iter().find(|t| t.class == format!("{EX}C"));
        let md = written(c.expect("ex:a a ex:C"));
        assert!(md.contains("_eq-rep-s_"), "{md}");
        assert!(
            md.contains("`ex:a owl:sameAs ex:b` [file:///w/a.ttl:3]"),
            "{md}"
        );
        assert!(
            md.contains("`ex:b owl:sameAs ex:c` [file:///w/a.ttl:4]"),
            "{md}"
        );
        assert!(md.contains("`ex:c a ex:C` [file:///w/a.ttl:5]"), "{md}");
    }

    #[test]
    fn terms_are_written_as_turtle_and_code_spans_hold_backticks() {
        let iri = |i: &str| format!("<{i}>");
        let place = |_: &Origin| String::new();
        let names = Names {
            iri: &iri,
            place: &place,
        };
        let lit = |l: spargebra::term::Literal| term(&l.into(), &names);
        use spargebra::term::{Literal, NamedNode};
        assert_eq!(lit(Literal::new_simple_literal("a\"b")), "\"a\\\"b\"");
        assert_eq!(
            lit(Literal::new_language_tagged_literal_unchecked("chat", "fr")),
            "\"chat\"@fr"
        );
        assert_eq!(
            lit(Literal::new_typed_literal(
                "1",
                NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer")
            )),
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(code("x"), "`x`");
        assert_eq!(code("a `b` c"), "`` a `b` c ``");
    }
}
