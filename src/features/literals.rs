//! Ill-typed literals (code `invalid-literal`): a literal whose datatype is
//! one RDF 1.2 Concepts recognizes (§5.1 the XSD datatypes, §5.3 `rdf:JSON`)
//! but whose lexical form is not in that datatype's lexical space, as
//! [`crate::shacl::literal::well_formed`] checks it.
//!
//! Leading and trailing whitespace is allowed: the `whiteSpace` facet of
//! every datatype checked collapses it before the lexical space applies
//! (and JSON text allows it). Datatypes whose lexical space is (almost) any
//! string are not checked: `xsd:string` and its other subtypes,
//! `xsd:anyURI`, `rdf:HTML`; nor is `rdf:XMLLiteral`.

use crate::analysis::Analysis;
use crate::shacl::literal::well_formed;
use crate::syntax::{Node, RDF, Span, XSD};

pub(crate) const CODE: &str = "invalid-literal";

/// The longest value a message quotes in full.
const MAX_SHOWN: usize = 40;

/// `value` on one line, cut to [`MAX_SHOWN`] characters: a long or multi-line
/// literal would otherwise fill the hover and the problems list.
fn shown(value: &str) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_SHOWN {
        return one_line;
    }
    let mut s: String = one_line.chars().take(MAX_SHOWN - 1).collect();
    s.push('…');
    s
}

/// Whether `lexical` is not in the lexical space of `datatype`.
pub(crate) fn ill_typed(datatype: &str, lexical: &str) -> bool {
    !well_formed(datatype, lexical.trim_matches([' ', '\t', '\n', '\r']))
}

/// Each ill-typed literal of `a` that is a triple's object (literals inside
/// triple terms are not checked), with the message.
pub(crate) fn invalid_literals(a: &Analysis) -> Vec<(Span, String)> {
    let mut out = Vec::new();
    for t in &a.parsed.triples {
        let Node::Literal {
            value,
            datatype,
            lang: None,
            ..
        } = &t.o
        else {
            continue;
        };
        let (prefix, local) = match (datatype.strip_prefix(XSD), datatype.strip_prefix(RDF)) {
            (Some(local), _) if local != "string" => ("xsd", local),
            (_, Some("JSON")) => ("rdf", "JSON"),
            _ => continue,
        };
        if ill_typed(datatype, value) {
            let message = format!("'{}' is not a valid {prefix}:{local}", shown(value));
            out.push((t.o_span, message));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Syntax;
    use std::sync::Arc;

    /// Asserts each of `valid` is in the lexical space of `xsd:local` and
    /// each of `invalid` is not.
    fn space(local: &str, valid: &[&str], invalid: &[&str]) {
        let dt = format!("{XSD}{local}");
        for v in valid {
            assert!(!ill_typed(&dt, v), "{v:?} should be a valid xsd:{local}");
        }
        for v in invalid {
            assert!(ill_typed(&dt, v), "{v:?} should not be a valid xsd:{local}");
        }
    }

    #[test]
    fn integers_and_their_ranges() {
        space(
            "integer",
            &[
                "12",
                "-0",
                "+7",
                "0012",
                " 12 ",
                "123456789012345678901234567890123456789012",
            ],
            &["twelve", "1.0", "", "1 2", "+-1", "1e3", "0x1F"],
        );
        space("byte", &["127", "-128"], &["128", "300", "-129"]);
        space("short", &["32767", "-32768"], &["32768"]);
        space("int", &["2147483647", "-2147483648"], &["2147483648"]);
        space(
            "long",
            &["9223372036854775807", "-9223372036854775808"],
            &[
                "9223372036854775808",
                "123456789012345678901234567890123456789012",
            ],
        );
        space("unsignedByte", &["255", "0", "-0"], &["256", "-1"]);
        space("unsignedShort", &["65535"], &["65536"]);
        space("unsignedInt", &["4294967295"], &["4294967296"]);
        space(
            "unsignedLong",
            &["18446744073709551615"],
            &["18446744073709551616", "-1"],
        );
        space("positiveInteger", &["1", "+0001"], &["0", "-1"]);
        space(
            "nonNegativeInteger",
            &["0", "-0", "123456789012345678901234567890123456789012"],
            &["-1"],
        );
        space("negativeInteger", &["-1"], &["0", "1"]);
        space("nonPositiveInteger", &["0", "+0", "-5"], &["1"]);
    }

    #[test]
    fn decimals_doubles_and_booleans() {
        space(
            "decimal",
            &["1.5", ".5", "5.", "-0.0", "+3"],
            &["1.5.2", "1e3", ".", "INF", "", "1,5"],
        );
        for dt in ["double", "float"] {
            space(
                dt,
                &[
                    "1e3", "1.5E-3", "INF", "-INF", "+INF", "NaN", ".5e1", "5.", "-12",
                ],
                &["Infinity", "nan", "1e", "e3", "-NaN", "1.0f", "", "1e3.5"],
            );
        }
        space(
            "boolean",
            &["true", "false", "1", "0"],
            &["yes", "True", "TRUE", "2", ""],
        );
    }

    #[test]
    fn dates_and_times() {
        space(
            "date",
            &[
                "2020-02-29",
                "2000-02-29",
                "2020-01-01Z",
                "2020-01-01+14:00",
                "2020-01-01-05:30",
                "-0044-03-15",
                "0000-01-01",
                "12020-01-01",
            ],
            &[
                "2020-13-45",
                "2021-02-29",
                "1900-02-29",
                "2020-04-31",
                "2020-00-10",
                "2020-01-00",
                "2020-01-01+14:01",
                "2020-01-01+15:00",
                "20-01-01",
                "02020-01-01",
                "2020-1-1",
                "2020-01-01T00:00:00",
            ],
        );
        space(
            "dateTime",
            &[
                "2020-01-01T10:00:00",
                "2020-01-01T24:00:00",
                "2020-01-01T24:00:00.000",
                "2020-01-01T10:00:00.123Z",
                "2020-01-01T23:59:59-13:59",
            ],
            &[
                "2020-01-01T24:00:01",
                "2020-01-01 10:00:00",
                "2020-01-01T10:00",
                "2020-01-01T10:60:00",
                "2020-01-01T10:00:60",
                "2020-01-01T10:00:00.",
                "2020-01-01",
            ],
        );
        space(
            "dateTimeStamp",
            &["2020-01-01T10:00:00Z", "2020-01-01T10:00:00+01:00"],
            &["2020-01-01T10:00:00"],
        );
        space(
            "time",
            &["10:00:00", "24:00:00", "23:59:59.5-05:00", "00:00:00Z"],
            &["25:00:00", "10:00", "24:30:00", "1:00:00"],
        );
        space("gYear", &["2020", "-0001", "2020Z"], &["20", "2020-01"]);
        space(
            "gYearMonth",
            &["2020-12", "2020-01+02:00"],
            &["2020-13", "2020"],
        );
        space(
            "gMonthDay",
            &["--02-29", "--12-31"],
            &["--02-30", "--04-31", "02-01", "--13-01"],
        );
        space("gDay", &["---31", "---01Z"], &["---32", "---00", "--31"]);
        space("gMonth", &["--12", "--01Z"], &["--13", "--12--", "12"]);
    }

    #[test]
    fn durations() {
        space(
            "duration",
            &[
                "P1Y2M3DT4H5M6.5S",
                "PT1H",
                "-P1D",
                "P0Y",
                "PT0.5S",
                "P1M",
                "PT1M",
            ],
            &[
                "P", "PT", "P1Y2X", "P1DT", "P1H", "PT1D", "1Y", "P-1Y", "P1M2Y", "P1.5Y", "",
            ],
        );
        space(
            "yearMonthDuration",
            &["P1Y2M", "-P3M", "P0Y"],
            &["P1D", "PT1H", "P"],
        );
        space(
            "dayTimeDuration",
            &["P1DT2H", "PT0S", "-P2D"],
            &["P1Y", "P1M", "P"],
        );
    }

    #[test]
    fn binary_and_language() {
        space("hexBinary", &["0FB7", "", "ab"], &["abc", "0G"]);
        space(
            "base64Binary",
            &["", "AAAA", "AA==", "AAA=", "AQ==", "AA AA", "SGVsbG8="],
            &["A", "AB=", "AB==", "AAAA=", "AA=A", "A A", "****"],
        );
        space(
            "language",
            &["en", "en-US", "de-CH-1996", "x-private"],
            &["a b", "en_US", "toolongtag", "", "en-"],
        );
    }

    #[test]
    fn json_and_unchecked_datatypes() {
        let json = "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON";
        for v in [r#"{"a":1}"#, r#""x""#, "1", " [true, null] "] {
            assert!(!ill_typed(json, v), "{v:?} is JSON");
        }
        for v in ["{", "{'a':1}", "", "[1,]", "1 2"] {
            assert!(ill_typed(json, v), "{v:?} is not JSON");
        }
        // Any string is a valid xsd:string, xsd:anyURI (XSD 1.1 does not
        // constrain its lexical space), rdf:HTML or other datatype.
        for dt in [
            format!("{XSD}string"),
            format!("{XSD}anyURI"),
            format!("{XSD}normalizedString"),
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#HTML".to_string(),
            "http://example.org/myType".to_string(),
        ] {
            assert!(!ill_typed(&dt, "not a number\n {"), "{dt}");
        }
    }

    /// (the text each finding covers, its message) in `text` as `syntax`.
    fn findings(text: &str, syntax: Syntax, ext: &str) -> Vec<(String, String)> {
        let a = Analysis::with_syntax(
            format!("file:///x/a.{ext}"),
            text,
            Arc::default(),
            false,
            syntax,
        );
        invalid_literals(&a)
            .into_iter()
            .map(|(s, m)| (text[s.start as usize..s.end as usize].to_string(), m))
            .collect()
    }

    fn f(at: &str, message: &str) -> (String, String) {
        (at.to_string(), message.to_string())
    }

    #[test]
    fn found_at_the_literal_in_every_rdf_syntax() {
        let ttl = r#"@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:a ex:age "twelve"^^xsd:integer ;
    ex:born "2020-13-45"^^<http://www.w3.org/2001/XMLSchema#date> ;
    ex:ok "12"^^xsd:integer, 12, 1.5, 1e3, true ;
    ex:list ( "300"^^xsd:byte ) ;
    ex:s "x"^^xsd:string .
"#;
        assert_eq!(
            findings(ttl, Syntax::Turtle, "ttl"),
            vec![
                f(
                    r#""twelve"^^xsd:integer"#,
                    "'twelve' is not a valid xsd:integer"
                ),
                f(
                    r#""2020-13-45"^^<http://www.w3.org/2001/XMLSchema#date>"#,
                    "'2020-13-45' is not a valid xsd:date"
                ),
                f(r#""300"^^xsd:byte"#, "'300' is not a valid xsd:byte"),
            ]
        );
        let trig = r#"@prefix ex: <http://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:g { ex:a ex:ok "yes"^^xsd:boolean }
"#;
        assert_eq!(
            findings(trig, Syntax::TriG, "trig"),
            vec![f(
                r#""yes"^^xsd:boolean"#,
                "'yes' is not a valid xsd:boolean"
            )]
        );
        let nt = "<http://example.org/a> <http://example.org/p> \"twelve\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n";
        let lit = "\"twelve\"^^<http://www.w3.org/2001/XMLSchema#integer>";
        let msg = "'twelve' is not a valid xsd:integer";
        assert_eq!(findings(nt, Syntax::NTriples, "nt"), vec![f(lit, msg)]);
        let nq = "<http://example.org/a> <http://example.org/p> \"twelve\"^^<http://www.w3.org/2001/XMLSchema#integer> <http://example.org/g> .\n";
        assert_eq!(findings(nq, Syntax::NQuads, "nq"), vec![f(lit, msg)]);
    }

    #[test]
    fn a_long_value_is_cut_in_the_message() {
        let long = "9".repeat(30) + &"x".repeat(5000);
        let ttl = format!(
            "<http://example.org/a> <http://example.org/p> \"{long}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n"
        );
        let found = findings(&ttl, Syntax::Turtle, "ttl");
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].1,
            format!(
                "'{}{}…' is not a valid xsd:integer",
                "9".repeat(30),
                "x".repeat(9)
            )
        );
    }

    #[test]
    fn found_at_the_value_in_json_ld() {
        let text = r#"{
  "@context": {
    "xsd": "http://www.w3.org/2001/XMLSchema#",
    "ex": "http://example.org/",
    "age": { "@id": "ex:age", "@type": "xsd:integer" }
  },
  "@id": "ex:a",
  "age": "twelve",
  "ex:born": { "@value": "2020-13-45", "@type": "xsd:date" },
  "ex:ok": [{ "@value": "12", "@type": "xsd:integer" }, 12, true]
}"#;
        assert_eq!(
            findings(text, Syntax::JsonLd, "jsonld"),
            vec![
                f(r#""twelve""#, "'twelve' is not a valid xsd:integer"),
                f(
                    r#"{ "@value": "2020-13-45", "@type": "xsd:date" }"#,
                    "'2020-13-45' is not a valid xsd:date"
                ),
            ]
        );
    }
}
