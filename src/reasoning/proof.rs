//! A proof as hover markdown: a nested list under the inferred type, each
//! rule step naming its W3C rule (`cax-sco`, `eq-rep-s`), each base triple a
//! link to where it is written. Statements are written as `turtle.rs`
//! writes them, as the diagnostics are: an RDF list is one item. A step the
//! list has shown already is written again as "shown above", as TurbOWL's
//! own tree writes it, and a long proof is cut after [`MAX_ITEMS`] items.

use super::{Origin, Proof};
use turbowl::Reason;

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
            Reason::Base => match self.proof.lead(i).as_ref().and_then(|l| l.origin()) {
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

    /// Step `i` as an item at `depth`; nothing for a base triple written
    /// within another one's item.
    fn step(&mut self, i: usize, depth: usize) {
        let step = &self.proof.proof.steps()[i];
        let statement = match step.reason {
            Reason::Base => match self.proof.lead(i) {
                Some(leaf) => leaf.text(self.names.iri),
                None => return,
            },
            Reason::Rule { .. } => self.proof.statement(&step.statement, self.names.iri),
        };
        self.items += 1;
        if self.items > MAX_ITEMS {
            return;
        }
        let statement = code(&statement);
        self.out.push_str(&format!(
            "\n{:indent$}- {statement}",
            "",
            indent = 2 * depth
        ));
        self.reason(i, depth);
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
                related: true,
                findings: true,
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
    fn an_rdf_list_in_a_proof_is_one_item() {
        let d = doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             ex:Pet owl:unionOf ( ex:Cat ex:Dog ex:Fish ) .\n\
             ex:tom a ex:Dog .\n",
        );
        let types = inferred(vec![d], "tom");
        let pet = types.iter().find(|t| t.class == format!("{EX}Pet"));
        let md = written(pet.expect("ex:tom a ex:Pet"));
        assert_eq!(
            md,
            "- **ex:Pet** · _cls-uni_\n  \
             - `ex:Pet owl:unionOf ( ex:Cat ex:Dog ex:Fish )` [file:///w/a.ttl:3]\n  \
             - `ex:tom a ex:Dog` [file:///w/a.ttl:4]",
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
    fn code_spans_hold_backticks() {
        assert_eq!(code("x"), "`x`");
        assert_eq!(code("a `b` c"), "`` a `b` c ``");
    }
}
