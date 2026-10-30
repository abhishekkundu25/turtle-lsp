use super::*;

mod oracle;

const HEAD: &str = "@prefix ex: <http://example.org/> .\n\
    @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
    @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
    @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
    @prefix schema: <http://schema.org/> .\n\
    @prefix schemas: <https://schema.org/> .\n";

type Sources = Vec<(String, Source, Arc<Analysis>)>;

fn doc(name: &str, body: &str, source: Source) -> (String, Source, Arc<Analysis>) {
    let uri = format!("file:///w/{name}.ttl");
    let a = Analysis::new(uri.clone(), &format!("{HEAD}{body}"), Arc::default(), false);
    (uri, source, Arc::new(a))
}

fn workspace(docs: &[(&str, &str)]) -> Sources {
    docs.iter().map(|(n, b)| doc(n, b, Source::Disk)).collect()
}

fn ask(sources: &Sources, params: serde_json::Value, cache: &FactsCache) -> SchemaResult {
    let p: SchemaParams = serde_json::from_value(params).expect("params");
    let focus = p.focus.clone();
    schema(sources, Encoding::Utf16, p, focus, cache).expect("valid params")
}

fn run(sources: &Sources, params: serde_json::Value) -> SchemaResult {
    ask(sources, params, &FactsCache::default())
}

fn class<'r>(r: &'r SchemaResult, name: &str) -> &'r ClassBox {
    r.classes
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| {
            let names: Vec<&str> = r.classes.iter().map(|c| c.name.as_str()).collect();
            panic!("no class {name} in {names:?}")
        })
}

fn index(r: &SchemaResult, name: &str) -> u32 {
    r.classes.iter().position(|c| c.name == name).expect(name) as u32
}

fn row<'c>(c: &'c ClassBox, name: &str) -> &'c Row {
    c.rows.iter().find(|r| r.name == name).unwrap_or_else(|| {
        let names: Vec<&str> = c.rows.iter().map(|r| r.name.as_str()).collect();
        panic!("no row {name} on {} in {names:?}", c.name)
    })
}

fn row_names(c: &ClassBox) -> Vec<&str> {
    c.rows.iter().map(|r| r.name.as_str()).collect()
}

fn class_names(r: &SchemaResult) -> Vec<&str> {
    r.classes.iter().map(|c| c.name.as_str()).collect()
}

const PUMPS: &str = "ex:Pump a owl:Class .\n\
    ex:pressure rdfs:domain ex:Pump ; rdfs:range xsd:decimal .\n\
    ex:maxRpm rdfs:domain ex:Pump .\n\
    ex:flow rdfs:domain ex:Pump ; rdfs:range xsd:decimal .\n\
    ex:PumpShape a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
      sh:property [ sh:path ex:flow ; sh:datatype xsd:integer ] .\n\
    ex:p1 a ex:Pump ; ex:pressure 1.5 ; ex:flow 2 ; ex:vendor \"A\" ; rdfs:label \"P1\" .\n\
    ex:p2 a ex:Pump ; ex:pressure \"high\" ; ex:flow 3 .\n";

/// Catches a state computed from the wrong facts: each of the five states
/// on one class, with the counts behind them.
#[test]
fn each_row_state_with_its_counts() {
    let r = run(&workspace(&[("pumps", PUMPS)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    assert_eq!(pump.kind, ClassKind::Class);
    assert_eq!(pump.instances, 2);
    let pressure = row(pump, "ex:pressure");
    assert_eq!(
        (
            pressure.state,
            pressure.used,
            pressure.of,
            pressure.bad_values
        ),
        (RowState::BadValues, 2, 2, Some(1))
    );
    assert_eq!(pressure.range.as_deref(), Some(&*format!("{XSD}decimal")));
    assert_eq!(pressure.range_name.as_deref(), Some("xsd:decimal"));
    assert!(!pressure.link);
    let rpm = row(pump, "ex:maxRpm");
    assert_eq!((rpm.state, rpm.used), (RowState::Unused, 0));
    let flow = row(pump, "ex:flow");
    assert_eq!(flow.state, RowState::Conflict);
    assert_eq!(
        flow.conflict,
        Some(Conflict {
            declared: "xsd:decimal".into(),
            shape: "xsd:integer".into()
        })
    );
    let how: Vec<DeclaredBy> = flow.declared.iter().map(|d| d.how).collect();
    assert_eq!(how, [DeclaredBy::Domain, DeclaredBy::Shape]);
    assert_eq!(flow.declared[0].uri, "file:///w/pumps.ttl");
    let vendor = row(pump, "ex:vendor");
    assert_eq!(
        (vendor.state, vendor.used, vendor.declared.len()),
        (RowState::Undeclared, 1, 0)
    );
    // rdfs:label is a standard vocabulary's: never "not declared".
    assert!(!row_names(pump).contains(&"rdfs:label"));
    assert_eq!(pump.problems, 4);
    // Undeclared rows come last; the others by IRI.
    assert_eq!(
        row_names(pump),
        ["ex:flow", "ex:maxRpm", "ex:pressure", "ex:vendor"]
    );
}

/// Catches a declared row called "never used" on a class with no instances.
#[test]
fn a_class_without_instances_has_no_unused_rows() {
    let body = "ex:Pump a owl:Class . ex:p rdfs:domain ex:Pump .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let p = row(class(&r, "ex:Pump"), "ex:p");
    assert_eq!((p.state, p.used, p.of), (RowState::Ok, 0, 0));
    assert_eq!(class(&r, "ex:Pump").problems, 0);
}

/// Catches a declaration read from `rdfs:domain` only.
#[test]
fn domain_includes_declares_in_both_schema_namespaces() {
    let body = "ex:Thing a rdfs:Class .\n\
        ex:a schema:domainIncludes ex:Thing .\n\
        ex:b schemas:domainIncludes ex:Thing .\n\
        ex:t a ex:Thing ; ex:a 1 .\n";
    let r = run(&workspace(&[("s", body)]), serde_json::json!({}));
    let t = class(&r, "ex:Thing");
    let a = row(t, "ex:a");
    assert_eq!(a.declared[0].how, DeclaredBy::DomainIncludes);
    assert_eq!((a.state, a.used), (RowState::Ok, 1));
    let b = row(t, "ex:b");
    assert_eq!(
        (b.state, b.declared[0].how),
        (RowState::Unused, DeclaredBy::DomainIncludes)
    );
}

/// Catches instances counted for their direct type only, an ancestor walk
/// that loops on a cycle, and a property declared on an ancestor reported
/// as not declared on the subclass.
#[test]
fn subclass_instances_count_for_ancestors_and_cycles_end() {
    let body = "ex:Machine a owl:Class .\n\
        ex:Pump rdfs:subClassOf ex:Machine .\n\
        ex:Centrifugal rdfs:subClassOf ex:Pump .\n\
        ex:serial rdfs:domain ex:Machine .\n\
        ex:c1 a ex:Centrifugal ; ex:serial \"1\" .\n\
        ex:p1 a ex:Pump .\n\
        ex:m1 a ex:Machine , ex:Pump .\n\
        ex:A rdfs:subClassOf ex:B . ex:B rdfs:subClassOf ex:C . ex:C rdfs:subClassOf ex:A .\n\
        ex:a1 a ex:B .\n";
    let r = run(&workspace(&[("m", body)]), serde_json::json!({}));
    assert_eq!(class(&r, "ex:Machine").instances, 3);
    assert_eq!(class(&r, "ex:Pump").instances, 3);
    assert_eq!(class(&r, "ex:Centrifugal").instances, 1);
    let serial = row(class(&r, "ex:Machine"), "ex:serial");
    assert_eq!((serial.used, serial.of), (1, 3));
    assert!(class(&r, "ex:Centrifugal").rows.is_empty());
    for c in ["ex:A", "ex:B", "ex:C"] {
        assert_eq!(class(&r, c).instances, 1, "{c}");
    }
    assert_eq!(class(&r, "ex:Pump").parents, [index(&r, "ex:Machine")]);
    assert_eq!(class(&r, "ex:A").parents, [index(&r, "ex:B")]);
}

/// Catches subjects joined per document: the type in one file, the values
/// in another.
#[test]
fn an_instance_typed_in_one_file_and_described_in_another_counts() {
    let onto = "ex:Pump a owl:Class . ex:pressure rdfs:domain ex:Pump .\n";
    let types = "ex:p1 a ex:Pump .\n";
    let values = "ex:p1 ex:pressure 3 .\n_:b ex:pressure 4 .\n";
    let r = run(
        &workspace(&[("onto", onto), ("types", types), ("values", values)]),
        serde_json::json!({}),
    );
    let p = row(class(&r, "ex:Pump"), "ex:pressure");
    assert_eq!((p.used, p.of, p.state), (1, 1, RowState::Ok));
}

/// Catches blank-node subjects left out, or joined across documents.
#[test]
fn blank_node_subjects_count_per_document() {
    let onto = "ex:Pump a owl:Class . ex:pressure rdfs:domain ex:Pump .\n";
    let a = "[ a ex:Pump ; ex:pressure 1 ] .\n_:x a ex:Pump .\n";
    let b = "_:x ex:pressure 2 .\n";
    let r = run(
        &workspace(&[("onto", onto), ("a", a), ("b", b)]),
        serde_json::json!({}),
    );
    let pump = class(&r, "ex:Pump");
    assert_eq!(pump.instances, 2);
    // `_:x` of b is not `_:x` of a.
    assert_eq!(row(pump, "ex:pressure").used, 1);
}

/// Catches a datatype check that is not exact (a numeric literal of another
/// type passing, a tagged string passing as `xsd:string`), one made against
/// `rdfs:Literal` or a class, or one made when two datatypes are declared.
#[test]
fn bad_values_per_datatype_pair() {
    let cases = [
        ("xsd:decimal", "1.5", false),
        ("xsd:decimal", "1", true),
        ("xsd:integer", "1", false),
        ("xsd:integer", "1.0e0", true),
        ("xsd:double", "1.0e0", false),
        ("xsd:string", "\"a\"", false),
        ("xsd:string", "\"a\"@en", true),
        ("rdf:langString", "\"a\"@en", false),
        ("rdf:langString", "\"a\"", true),
        ("xsd:date", "\"2020-01-01\"^^xsd:date", false),
        ("xsd:date", "\"2020-01-01\"", true),
        ("xsd:boolean", "true", false),
        ("xsd:boolean", "\"true\"", true),
        ("rdfs:Literal", "\"a\"", false),
        ("ex:Pipe", "\"a\"", false),
    ];
    for (range, value, bad) in cases {
        let body = format!(
            "ex:Pump a owl:Class . ex:v rdfs:domain ex:Pump ; rdfs:range {range} .\n\
             ex:p a ex:Pump ; ex:v {value} , ex:other .\n"
        );
        let r = run(&workspace(&[("d", &body)]), serde_json::json!({}));
        let v = row(class(&r, "ex:Pump"), "ex:v");
        assert_eq!(v.bad_values, bad.then_some(1), "{range} {value}");
        let state = if bad {
            RowState::BadValues
        } else {
            RowState::Ok
        };
        assert_eq!(v.state, state, "{range} {value}");
    }
    let two = "ex:Pump a owl:Class .\n\
        ex:v rdfs:domain ex:Pump ; rdfs:range xsd:string , xsd:integer .\n\
        ex:p a ex:Pump ; ex:v 1.5 .\n";
    let r = run(&workspace(&[("d", two)]), serde_json::json!({}));
    let v = row(class(&r, "ex:Pump"), "ex:v");
    assert_eq!((v.state, v.bad_values), (RowState::Ok, None));
    // A shape's datatype alone is checked too.
    let shaped = "ex:Pump a owl:Class .\n\
        ex:S sh:targetClass ex:Pump ; sh:property [ sh:path ex:v ; sh:datatype xsd:integer ] .\n\
        ex:p a ex:Pump ; ex:v \"7\" .\n";
    let r = run(&workspace(&[("d", shaped)]), serde_json::json!({}));
    let v = row(class(&r, "ex:Pump"), "ex:v");
    assert_eq!((v.state, v.bad_values), (RowState::BadValues, Some(1)));
}

/// Catches a conflict missed between a range and `sh:class`, or reported
/// when they agree; and a link row without its target.
#[test]
fn conflict_between_range_and_shape() {
    let body = "ex:Pump a owl:Class . ex:Pipe a owl:Class . ex:Hose a owl:Class .\n\
        ex:feeds rdfs:domain ex:Pump ; rdfs:range ex:Pipe .\n\
        ex:drains rdfs:domain ex:Pump ; rdfs:range ex:Pipe .\n\
        ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
          sh:property [ sh:path ex:feeds ; sh:class ex:Hose ] ,\n\
                      [ sh:path ex:drains ; sh:class ex:Pipe ] .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    let feeds = row(pump, "ex:feeds");
    assert_eq!(feeds.state, RowState::Conflict);
    assert_eq!(
        feeds.conflict,
        Some(Conflict {
            declared: "ex:Pipe".into(),
            shape: "ex:Hose".into()
        })
    );
    let drains = row(pump, "ex:drains");
    assert_eq!(
        (drains.state, drains.conflict.clone()),
        (RowState::Ok, None)
    );
    assert!(drains.link);
    assert_eq!(drains.target, Some(index(&r, "ex:Pipe")));
}

/// Catches link rows read from the range only: an object property, a
/// shape's `sh:class`, or IRI values make one too.
#[test]
fn link_rows_from_types_shapes_and_values() {
    let body = "ex:Pump a owl:Class . ex:Pipe a owl:Class .\n\
        ex:a a owl:ObjectProperty ; rdfs:domain ex:Pump .\n\
        ex:b a owl:DatatypeProperty ; rdfs:domain ex:Pump .\n\
        ex:S sh:targetClass ex:Pump ; sh:property [ sh:path ex:c ; sh:class ex:Pipe ] .\n\
        ex:d rdfs:domain ex:Pump .\n\
        ex:e rdfs:domain ex:Pump .\n\
        ex:p a ex:Pump ; ex:d ex:x ; ex:e \"x\" .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    let links: Vec<(&str, bool)> = pump
        .rows
        .iter()
        .map(|r| (r.name.as_str(), r.link))
        .collect();
    assert_eq!(
        links,
        [
            ("ex:a", true),
            ("ex:b", false),
            ("ex:c", true),
            ("ex:d", true),
            ("ex:e", false)
        ]
    );
    assert_eq!(row(pump, "ex:c").target, Some(index(&r, "ex:Pipe")));
    assert_eq!(row(pump, "ex:a").target, None);
}

/// Catches `missing` counted as instances without the property whatever
/// `sh:minCount` says, or a maximum lost.
#[test]
fn required_rows_count_the_instances_missing_them() {
    let body = "ex:Pump a owl:Class .\n\
        ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
          sh:property [ sh:path ex:serial ; sh:minCount 1 ; sh:maxCount 1 ] ,\n\
                      [ sh:path ex:port ; sh:minCount 2 ] ,\n\
                      [ sh:path ex:note ; sh:maxCount 3 ] .\n\
        ex:p1 a ex:Pump ; ex:serial \"1\" ; ex:port 1 , 2 .\n\
        ex:p2 a ex:Pump ; ex:port 1 .\n\
        ex:p3 a ex:Pump ; ex:serial \"3\" , \"4\" .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    let serial = row(pump, "ex:serial");
    assert_eq!(
        serial.required,
        Some(Required {
            min: 1,
            max: Some(1)
        })
    );
    assert_eq!(
        (serial.state, serial.used, serial.missing),
        (RowState::Ok, 2, Some(1))
    );
    let port = row(pump, "ex:port");
    assert_eq!(port.required, Some(Required { min: 2, max: None }));
    assert_eq!((port.used, port.missing), (2, Some(2)));
    let note = row(pump, "ex:note");
    assert_eq!((note.required, note.missing), (None, None));
    // serial and port miss values; note is never used.
    assert_eq!(pump.problems, 3);
}

/// The examples of a row as (kind, file name, line, text at the range, name).
fn examples<'a>(ws: &'a Sources, r: &'a Row) -> Vec<(ExampleKind, &'a str, u32, &'a str, &'a str)> {
    r.examples
        .iter()
        .map(|e| {
            let a = &ws
                .iter()
                .find(|(.., a)| a.uri == e.uri)
                .expect("its document")
                .2;
            let from = a.offset(Encoding::Utf16, e.range.start).expect("start") as usize;
            let to = a.offset(Encoding::Utf16, e.range.end).expect("end") as usize;
            let file = e.uri.rsplit('/').next().unwrap_or("");
            (
                e.kind,
                file,
                e.range.start.line,
                &a.text[from..to],
                e.name.as_str(),
            )
        })
        .collect()
}

/// The line of the body's `n`th line (from 0), after [`HEAD`].
fn line(n: u32) -> u32 {
    HEAD.lines().count() as u32 + n
}

/// Catches missing instances listed in hash or subject order instead of
/// document and text order, or located at a use instead of where the
/// instance is described.
#[test]
fn missing_examples_are_the_instances_in_document_order() {
    let shape = "ex:Pump a owl:Class .\n\
        ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
          sh:property [ sh:path ex:serial ; sh:minCount 1 ] .\n\
        ex:p9 a ex:Pump .\n";
    let other = "ex:p1 a ex:Pump ; ex:serial \"1\" .\n\
        ex:p2 ex:note \"b\" .\n\
        ex:p2 a ex:Pump .\n\
        [ a ex:Pump ; ex:note \"anon\" ] .\n";
    // b.ttl sorts before c.ttl: p2 and the blank node come first, though p9
    // was typed first.
    let ws = workspace(&[("c", shape), ("b", other)]);
    let r = run(
        &ws,
        serde_json::json!({ "examples": "http://example.org/Pump" }),
    );
    let serial = row(class(&r, "ex:Pump"), "ex:serial");
    assert_eq!(serial.missing, Some(3));
    assert_eq!(
        examples(&ws, serial),
        vec![
            (ExampleKind::Missing, "b.ttl", line(1), "ex:p2", "ex:p2"),
            (ExampleKind::Missing, "b.ttl", line(3), "a ex:Pump", "_:b1"),
            (ExampleKind::Missing, "c.ttl", line(3), "ex:p9", "ex:p9"),
        ]
    );
    let json = serde_json::to_value(serial).expect("json");
    assert_eq!(json["examples"][0]["kind"], "missing");
    assert_eq!(json["examples"][1]["name"], "_:b1");
}

/// Catches examples past the cap, or a cap that keeps the last ones.
#[test]
fn examples_stop_at_ten_and_keep_the_first() {
    let mut body = String::from(
        "ex:Pump a owl:Class .\n\
         ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
           sh:property [ sh:path ex:serial ; sh:minCount 1 ; sh:datatype xsd:string ] .\n",
    );
    for i in 0..25 {
        body.push_str(&format!("ex:p{i} a ex:Pump ; ex:serial {i} .\n"));
    }
    for i in 25..40 {
        body.push_str(&format!("ex:p{i} a ex:Pump .\n"));
    }
    let ws = workspace(&[("d", &body)]);
    let r = run(
        &ws,
        serde_json::json!({ "examples": "http://example.org/Pump" }),
    );
    let serial = row(class(&r, "ex:Pump"), "ex:serial");
    assert_eq!((serial.missing, serial.bad_values), (Some(15), Some(25)));
    let got = examples(&ws, serial);
    let missing: Vec<&str> = got
        .iter()
        .filter(|e| e.0 == ExampleKind::Missing)
        .map(|e| e.4)
        .collect();
    let want: Vec<String> = (25..35).map(|i| format!("ex:p{i}")).collect();
    assert_eq!(missing, want);
    let bad: Vec<(u32, &str, &str)> = got
        .iter()
        .filter(|e| e.0 == ExampleKind::BadValue)
        .map(|e| (e.2, e.3, e.4))
        .collect();
    let want: Vec<(u32, String, String)> = (0..10)
        .map(|i| (line(3 + i), i.to_string(), format!("ex:p{i}")))
        .collect();
    let want: Vec<(u32, &str, &str)> = want
        .iter()
        .map(|(l, v, n)| (*l, v.as_str(), n.as_str()))
        .collect();
    assert_eq!(bad, want);
}

/// Catches undeclared rows without the uses, or one example per use instead
/// of per instance; and examples on rows with nothing to point at.
#[test]
fn undeclared_rows_point_at_each_instances_first_use() {
    let ws = workspace(&[("pumps", PUMPS)]);
    let r = run(
        &ws,
        serde_json::json!({ "examples": "http://example.org/Pump" }),
    );
    let pump = class(&r, "ex:Pump");
    let vendor = row(pump, "ex:vendor");
    assert_eq!(vendor.state, RowState::Undeclared);
    assert_eq!(
        examples(&ws, vendor),
        vec![(
            ExampleKind::Undeclared,
            "pumps.ttl",
            line(6),
            "ex:vendor \"A\"",
            "ex:p1"
        )]
    );
    let pressure = row(pump, "ex:pressure");
    assert_eq!(
        examples(&ws, pressure),
        vec![(
            ExampleKind::BadValue,
            "pumps.ttl",
            line(7),
            "\"high\"",
            "ex:p2"
        )]
    );
    for name in ["ex:maxRpm", "ex:flow"] {
        let r = row(pump, name);
        assert!(r.examples.is_empty(), "{name}: {:?}", r.examples);
        let json = serde_json::to_value(r).expect("json");
        assert!(json.get("examples").is_none(), "{name}: {json}");
    }
}

const TWO_CLASSES: &str = "ex:S a sh:NodeShape ; sh:targetClass ex:Pump , ex:Valve ;\n\
      sh:property [ sh:path ex:serial ; sh:minCount 1 ] .\n\
    ex:p1 a ex:Pump ; ex:vendor \"A\" .\n\
    ex:v1 a ex:Valve ; ex:vendor \"B\" .\n\
    ex:v2 a ex:Valve ; ex:serial \"2\" .\n";

/// The classes of `r` whose rows list examples.
fn with_examples(r: &SchemaResult) -> Vec<&str> {
    r.classes
        .iter()
        .filter(|c| c.rows.iter().any(|row| !row.examples.is_empty()))
        .map(|c| c.name.as_str())
        .collect()
}

/// Catches examples on every class (the answer grows with the classes
/// listed), or `examples` ignored: only one class lists them, `examples`
/// when it is listed, else the focus class, else none; the counts stay.
#[test]
fn examples_are_listed_on_one_class_only() {
    let ws = workspace(&[("two", TWO_CLASSES)]);
    let ex = |s: &str| format!("http://example.org/{s}");
    let none = run(&ws, serde_json::json!({}));
    assert_eq!(with_examples(&none), Vec::<&str>::new());
    let pump = run(&ws, serde_json::json!({ "examples": ex("Pump") }));
    assert_eq!(with_examples(&pump), ["ex:Pump"]);
    let valve = run(&ws, serde_json::json!({ "focus": ex("Valve") }));
    assert_eq!(with_examples(&valve), ["ex:Valve"]);
    let both = run(
        &ws,
        serde_json::json!({ "focus": ex("Valve"), "examples": ex("Pump") }),
    );
    assert_eq!(with_examples(&both), ["ex:Pump"]);
    // A property focus: its class.
    let prop = run(&ws, serde_json::json!({ "focus": ex("serial") }));
    assert_eq!(with_examples(&prop).len(), 1);
    // Not a listed class: the focus, else none.
    for absent in [ex("serial"), ex("Nothing"), "not an iri".into()] {
        let r = run(
            &ws,
            serde_json::json!({ "focus": ex("Valve"), "examples": absent }),
        );
        assert_eq!(with_examples(&r), ["ex:Valve"]);
        let r = run(&ws, serde_json::json!({ "examples": absent }));
        assert_eq!(with_examples(&r), Vec::<&str>::new());
    }
    // Cut by the cap: the focus.
    let r = run(
        &ws,
        serde_json::json!({ "focus": ex("Valve"), "examples": ex("Pump"), "maxClasses": 1 }),
    );
    assert_eq!(with_examples(&r), ["ex:Valve"]);
    // The same counts, with or without examples.
    let strip = |mut r: SchemaResult| {
        for c in &mut r.classes {
            for row in &mut c.rows {
                row.examples.clear();
            }
        }
        r
    };
    assert_eq!(strip(none.clone()), strip(pump.clone()));
    let valve_first = run(
        &ws,
        serde_json::json!({ "focus": ex("Valve"), "examples": ex("Valve") }),
    );
    assert_eq!(valve_first, valve);
    // The places are those the class lists alone.
    let vendor = row(class(&pump, "ex:Pump"), "ex:vendor");
    assert_eq!(examples(&ws, vendor).len(), 1);
    let serial = row(class(&valve, "ex:Valve"), "ex:serial");
    assert_eq!(
        examples(&ws, serial)
            .iter()
            .map(|e| (e.0, e.4))
            .collect::<Vec<_>>(),
        [(ExampleKind::Missing, "ex:v1")]
    );
}

/// Catches places collected on every row and dropped at the answer: only
/// the rows of the focus class and of the `examples` class collect any.
#[test]
fn places_are_collected_for_the_chosen_classes_only() {
    let ws = workspace(&[("two", TWO_CLASSES)]);
    let cache = FactsCache::default();
    let wanted: Vec<usize> = (0..ws.len()).collect();
    let docs: Vec<Doc> = cache
        .facts(&ws, &wanted)
        .into_iter()
        .zip(&ws)
        .map(|(facts, (_, _, a))| Doc {
            a,
            external: false,
            facts,
        })
        .collect();
    let collected = |focus: Option<&str>, examples: Option<&str>| {
        let q = Query {
            focus: focus.map(|f| format!("http://example.org/{f}")),
            max_classes: 300,
            max_rows: 64,
            imports: false,
            examples: examples.map(|e| format!("http://example.org/{e}")),
            search: None,
        };
        let (m, _) = Merge::new(&docs, &q);
        let mut out: Vec<String> = m
            .classes
            .iter()
            .filter(|c| {
                c.rows.iter().any(|&r| {
                    let row = &m.rows[r as usize];
                    !(row.missing_at.0.is_empty()
                        && row.bad_at.0.is_empty()
                        && row.undeclared_at.0.is_empty()
                        && row.shape_at.0.is_empty())
                })
            })
            .map(|c| c.iri.to_string())
            .collect();
        out.sort();
        out
    };
    let ex = |s: &str| format!("http://example.org/{s}");
    assert_eq!(collected(None, None), Vec::<String>::new());
    assert_eq!(collected(None, Some("Pump")), [ex("Pump")]);
    assert_eq!(collected(Some("Valve"), None), [ex("Valve")]);
    assert_eq!(
        collected(Some("Valve"), Some("Pump")),
        [ex("Pump"), ex("Valve")]
    );
}

/// Catches a cap that keeps classes by name, or drops the focus's family.
#[test]
fn the_cap_keeps_the_focus_family_first() {
    let mut body = String::from(
        "ex:Root a owl:Class . ex:Mid rdfs:subClassOf ex:Root . ex:Leaf rdfs:subClassOf ex:Mid .\n\
         ex:Other a owl:Class . ex:Linked a owl:Class .\n\
         ex:to rdfs:domain ex:Mid ; rdfs:range ex:Linked .\n",
    );
    for i in 0..5 {
        body.push_str(&format!("ex:o{i} a ex:Other .\n"));
    }
    body.push_str("ex:l a ex:Leaf .\n");
    let ws = workspace(&[("d", &body)]);

    // No focus: the class with most instances leads.
    let r = run(&ws, serde_json::json!({}));
    assert_eq!(class_names(&r)[0], "ex:Other");
    assert_eq!((r.focus, r.total.classes), (None, 5));

    let focus = serde_json::json!({ "focus": "http://example.org/Mid", "maxClasses": 4 });
    let r = run(&ws, focus);
    assert_eq!(
        class_names(&r),
        ["ex:Mid", "ex:Leaf", "ex:Root", "ex:Linked"]
    );
    assert_eq!((r.focus, r.total.classes), (Some(0), 5));
    assert_eq!(class(&r, "ex:Leaf").parents, [0]);
    assert_eq!(row(class(&r, "ex:Mid"), "ex:to").target, Some(3));

    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/Mid", "maxClasses": 2 }),
    );
    assert_eq!(class_names(&r), ["ex:Mid", "ex:Leaf"]);
    // A target or parent outside the cap is not pointed to.
    let to = row(class(&r, "ex:Mid"), "ex:to");
    assert_eq!((to.link, to.target), (true, None));
    assert!(class(&r, "ex:Mid").parents.is_empty());
}

/// Catches a property focus that misses its class or its row, or a row cut
/// that drops problems before fine rows.
#[test]
fn a_property_focus_picks_its_class_and_row_and_cuts_keep_the_worst() {
    let mut body = String::from("ex:Pump a owl:Class . ex:Other a owl:Class .\n");
    for i in 0..6 {
        body.push_str(&format!("ex:r{i} rdfs:domain ex:Pump .\n"));
    }
    body.push_str("ex:p a ex:Pump ; ex:r0 1 ; ex:r1 1 ; ex:r2 1 ; ex:r3 1 ; ex:zz 1 .\n");
    body.push_str("ex:o a ex:Other . ex:o2 a ex:Other .\n");
    let ws = workspace(&[("d", &body)]);
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/r1", "maxRows": 3 }),
    );
    assert_eq!(r.focus, Some(0));
    let pump = &r.classes[0];
    assert_eq!(pump.name, "ex:Pump");
    // r1 (the focus), then the worst: zz not declared, r4 and r5 never used
    // (r5 cut); shown in row order.
    assert_eq!(row_names(pump), ["ex:r1", "ex:r4", "ex:zz"]);
    assert_eq!(pump.more_rows, 4);
    // Counted over every row, the cut ones too.
    assert_eq!(pump.problems, 3);
    assert_eq!(r.focus_row, Some(0));

    let r = run(&ws, serde_json::json!({ "focus": "http://example.org/zz" }));
    assert_eq!(r.classes[0].name, "ex:Pump");
    assert_eq!(r.focus_row, Some(6));
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/nothing" }),
    );
    assert_eq!((r.focus, r.focus_row), (None, None));
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/Pump" }),
    );
    assert_eq!((r.focus, r.focus_row), (Some(0), None));
}

/// Catches imports listed when they should be counted, or lost when asked.
#[test]
fn imported_classes_are_counted_unless_asked_for() {
    let ws = vec![
        doc(
            "mine",
            "ex:Pump rdfs:subClassOf ex:Up .\nex:p a ex:Pump .\n",
            Source::Disk,
        ),
        doc(
            "upper",
            "ex:Up a owl:Class . ex:Down a owl:Class .\n",
            Source::External,
        ),
    ];
    let r = run(&ws, serde_json::json!({}));
    assert_eq!((r.imported, r.total.classes), (2, 1));
    assert!(class(&r, "ex:Pump").parents.is_empty());
    let r = run(&ws, serde_json::json!({ "imports": true }));
    assert_eq!((r.imported, r.total.classes), (0, 3));
    let up = class(&r, "ex:Up");
    assert_eq!(
        (up.kind, up.instances, up.uri.clone()),
        (ClassKind::Imported, 1, None)
    );
    assert_eq!(class(&r, "ex:Pump").parents, [index(&r, "ex:Up")]);
}

/// Catches typos left out, or standard classes drawn as ghosts.
#[test]
fn classes_declared_nowhere_are_undefined() {
    let body = "ex:Pump a owl:Class .\n\
        ex:p a ex:Pmup .\n\
        ex:feeds rdfs:domain ex:Valve .\n\
        ex:S sh:targetClass ex:Hose .\n\
        ex:Pump rdfs:subClassOf ex:Machine .\n\
        ex:c a skos:Concept , owl:Thing .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    for name in ["ex:Pmup", "ex:Valve", "ex:Hose", "ex:Machine"] {
        let c = class(&r, name);
        assert_eq!(
            (c.kind, c.uri.clone()),
            (ClassKind::Undefined, None),
            "{name}"
        );
    }
    let pump = class(&r, "ex:Pump");
    assert_eq!(pump.kind, ClassKind::Class);
    assert_eq!(pump.uri.as_deref(), Some("file:///w/d.ttl"));
    let range = pump.range.unwrap();
    assert_eq!((range.start.line, range.start.character), (9, 0));
    assert_eq!(range.end.character, 7);
    assert!(r.classes.iter().all(|c| c.name.starts_with("ex:")));
}

/// Catches a standard class listed although nothing declares rows for it.
#[test]
fn standard_classes_are_listed_only_with_declared_rows() {
    let body = "ex:age schema:domainIncludes schema:Person .\n\
        ex:x a schema:Person ; ex:age 3 .\n\
        ex:y a owl:Thing ; ex:age 4 .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    assert_eq!(class_names(&r), ["schema:Person"]);
    assert_eq!(row(&r.classes[0], "ex:age").used, 1);
}

/// Catches a path other than a predicate dropped or counted.
#[test]
fn complex_paths_are_listed_as_written() {
    let body = "ex:Pump a owl:Class .\n\
        ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
          sh:property [ sh:path [ sh:inversePath ex:feeds ] ; sh:minCount 1 ] .\n\
        ex:p a ex:Pump .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    assert_eq!(row_names(pump), ["[ sh:inversePath ex:feeds ]"]);
    let w = &pump.rows[0];
    assert_eq!((w.state, w.used, w.missing), (RowState::Ok, 0, None));
    assert_eq!(w.required, Some(Required { min: 1, max: None }));
}

/// Catches an implicit class target (a shape that is also a class) missed.
#[test]
fn a_shape_that_is_a_class_targets_itself() {
    let body = "ex:Pump a owl:Class , sh:NodeShape ;\n\
          sh:property [ sh:path ex:serial ; sh:minCount 1 ] .\n\
        ex:p a ex:Pump .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let serial = row(class(&r, "ex:Pump"), "ex:serial");
    assert_eq!(serial.missing, Some(1));
}

/// Catches a parse error that stops the answer, or one that drops what
/// parsed.
#[test]
fn documents_with_syntax_errors_answer_what_parsed() {
    let body = "ex:Pump a owl:Class .\nex:p a ex:Pump ; ex:v 1 .\nex:q a ex:Pump ; ex:v \"oops .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    assert!(class(&r, "ex:Pump").instances >= 1);
    let r = run(
        &workspace(&[("d", "@@@ [[[ ( . ; ,")]),
        serde_json::json!({}),
    );
    assert!(r.classes.is_empty());
}

#[test]
fn an_empty_workspace_has_no_classes() {
    let r = run(&Vec::new(), serde_json::json!({}));
    assert_eq!(r, SchemaResult::default());
    let r = run(
        &workspace(&[("e", "")]),
        serde_json::json!({ "focus": "http://example.org/x" }),
    );
    assert_eq!(r, SchemaResult::default());
}

#[test]
fn out_of_range_caps_are_invalid() {
    let ws = workspace(&[("d", PUMPS)]);
    for p in [
        serde_json::json!({ "maxClasses": 0 }),
        serde_json::json!({ "maxClasses": MAX_CLASSES + 1 }),
        serde_json::json!({ "maxRows": 0 }),
        serde_json::json!({ "maxRows": MAX_ROWS + 1 }),
        serde_json::json!({ "search": "x".repeat(MAX_SEARCH + 1) }),
    ] {
        let params: SchemaParams = serde_json::from_value(p.clone()).unwrap();
        let cache = FactsCache::default();
        assert!(
            schema(&ws, Encoding::Utf16, params, None, &cache).is_err(),
            "{p}"
        );
    }
}

/// Catches `files` ignored, or applied to the imports.
#[test]
fn files_limits_the_workspace_documents() {
    let ws = vec![
        doc("a", "ex:A a owl:Class .\n", Source::Disk),
        doc("b", "ex:B a owl:Class .\n", Source::Open),
        doc("i", "ex:I a owl:Class .\n", Source::External),
    ];
    let r = run(
        &ws,
        serde_json::json!({ "files": ["file:///w/b.ttl"], "imports": true }),
    );
    assert_eq!(class_names(&r), ["ex:B", "ex:I"]);
}

/// Catches a cache that recomputes unchanged documents, keeps a changed
/// one's old facts, or keeps documents that left.
#[test]
fn the_cache_reuses_unchanged_documents_only() {
    let cache = FactsCache::default();
    let mut ws = workspace(&[
        ("onto", "ex:Pump a owl:Class .\n"),
        ("data", "ex:p a ex:Pump .\n"),
    ]);
    let first = ask(&ws, serde_json::json!({}), &cache);
    assert_eq!(cache.len(), 2);
    let kept = cache.facts(&ws, &[0])[0].clone();
    ws[1] = doc("data", "ex:p a ex:Pump . ex:q a ex:Pump .\n", Source::Disk);
    let second = ask(&ws, serde_json::json!({}), &cache);
    assert!(Arc::ptr_eq(&kept, &cache.facts(&ws, &[0])[0]));
    assert_eq!(class(&first, "ex:Pump").instances, 1);
    assert_eq!(class(&second, "ex:Pump").instances, 2);
    ws.pop();
    ask(&ws, serde_json::json!({}), &cache);
    assert_eq!(cache.len(), 1);
}

/// Catches a cache that cannot be shared between the jobs' threads.
#[test]
fn the_cache_is_shared_between_threads() {
    fn shared<T: Send + Sync>(_: &T) {}
    let cache = FactsCache::default();
    shared(&cache);
    let ws = workspace(&[("d", PUMPS)]);
    let answers: Vec<SchemaResult> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..4)
            .map(|_| s.spawn(|| ask(&ws, serde_json::json!({}), &cache)))
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    assert!(answers.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(cache.len(), 1);
}

// ---- generated workspaces ----

/// xorshift64*, as tests/chaos.rs.
struct Rng(u64);

impl Rng {
    /// Seeds pass through splitmix64, so neighbouring seeds differ.
    fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const DATATYPES: [&str; 7] = [
    "xsd:string",
    "xsd:integer",
    "xsd:decimal",
    "xsd:date",
    "rdf:langString",
    "xsd:boolean",
    "rdfs:Literal",
];

const LITERALS: [&str; 9] = [
    "\"a\"",
    "\"a\"@en",
    "1",
    "1.5",
    "1.0e3",
    "true",
    "\"2020-01-01\"^^xsd:date",
    "\"x\"^^xsd:integer",
    "\"\"\"long\"\"\"",
];

/// The statements of a random workspace, as Turtle, before they are split
/// among documents: classes (some declared), subclass links (cycles
/// included), properties with domains and ranges, shapes, and instances
/// (IRIs and blank nodes) with values of mixed datatypes.
fn statements(rng: &mut Rng) -> Vec<String> {
    let classes = 1 + rng.below(7);
    let props = 1 + rng.below(6);
    let subjects = rng.below(14);
    // One more class than declared: a class used and declared nowhere.
    let c = |rng: &mut Rng| format!("ex:C{}", rng.below(classes + 1));
    let p = |rng: &mut Rng| format!("ex:p{}", rng.below(props));
    let mut out = Vec::new();
    for i in 0..classes {
        match rng.below(4) {
            0 => out.push(format!("ex:C{i} a owl:Class .")),
            1 => out.push(format!("ex:C{i} a rdfs:Class ; rdfs:label \"C{i}\" .")),
            _ => {}
        }
    }
    for _ in 0..rng.below(classes + 2) {
        out.push(format!("{} rdfs:subClassOf {} .", c(rng), c(rng)));
    }
    for i in 0..props {
        let mut s = format!("ex:p{i}");
        match rng.below(5) {
            0 => s.push_str(&format!(" rdfs:domain {}", c(rng))),
            1 => s.push_str(&format!(" schema:domainIncludes {}", c(rng))),
            2 => s.push_str(&format!(" schemas:domainIncludes {} , {}", c(rng), c(rng))),
            _ => s.push_str(" rdfs:comment \"p\""),
        }
        for _ in 0..rng.below(3) {
            if rng.chance(60) {
                s.push_str(&format!(" ; rdfs:range {}", rng.pick(&DATATYPES)));
            } else {
                s.push_str(&format!(" ; rdfs:range {}", c(rng)));
            }
        }
        if rng.chance(25) {
            s.push_str(" ; a owl:ObjectProperty");
        }
        s.push_str(" .");
        out.push(s);
    }
    for i in 0..rng.below(3) {
        let mut s = format!("ex:S{i} a sh:NodeShape ; sh:targetClass {}", c(rng));
        for _ in 0..rng.below(3) {
            let mut ps = format!("sh:path {}", p(rng));
            if rng.chance(40) {
                ps.push_str(&format!(" ; sh:datatype {}", rng.pick(&DATATYPES)));
            } else if rng.chance(30) {
                ps.push_str(&format!(" ; sh:class {}", c(rng)));
            }
            if rng.chance(50) {
                ps.push_str(&format!(" ; sh:minCount {}", rng.below(3)));
            }
            if rng.chance(30) {
                ps.push_str(&format!(" ; sh:maxCount {}", rng.below(3)));
            }
            s.push_str(&format!(" ; sh:property [ {ps} ]"));
        }
        s.push_str(" .");
        out.push(s);
    }
    for _ in 0..subjects {
        let blank = rng.chance(25);
        let name = if blank {
            format!("_:b{}", rng.below(4))
        } else {
            format!("ex:i{}", rng.below(subjects.max(1)))
        };
        let mut parts = Vec::new();
        for _ in 0..rng.below(3) {
            parts.push(format!("a {}", c(rng)));
        }
        for _ in 0..rng.below(5) {
            let value = match rng.below(4) {
                0 => format!("ex:i{}", rng.below(4)),
                1 => "[ ex:p0 1 ]".to_string(),
                _ => rng.pick(&LITERALS).to_string(),
            };
            let pred = if rng.chance(15) {
                "rdfs:label".to_string()
            } else {
                p(rng)
            };
            parts.push(format!("{pred} {value}"));
        }
        if parts.is_empty() {
            continue;
        }
        if blank && rng.chance(40) {
            out.push(format!("[ {} ] .", parts.join(" ; ")));
        } else {
            out.push(format!("{name} {} .", parts.join(" ; ")));
        }
    }
    out
}

/// Breaks the text: a stray token, or a cut.
fn break_text(rng: &mut Rng, text: &str) -> String {
    let at = rng.below(text.len() + 1);
    let at = (0..=at)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let junk = ["\"", ";", ".", "[", "]", "(", "@", "ex:", "<", "^^", "a"];
    if rng.chance(30) {
        text[..at].to_string()
    } else {
        format!("{}{}{}", &text[..at], rng.pick(&junk), &text[at..])
    }
}

/// The documents of a random workspace: up to four, some maybe imports,
/// some maybe broken.
fn generated(seed: u64) -> Sources {
    let mut rng = Rng::new(seed);
    let stmts = statements(&mut rng);
    let n = 1 + rng.below(4);
    let mut bodies = vec![String::new(); n];
    for s in stmts {
        let d = rng.below(n);
        bodies[d].push_str(&s);
        bodies[d].push('\n');
    }
    bodies
        .into_iter()
        .enumerate()
        .map(|(i, mut body)| {
            if rng.chance(15) {
                body = break_text(&mut rng, &body);
            }
            let source = if i > 0 && rng.chance(25) {
                Source::External
            } else {
                Source::Disk
            };
            doc(&format!("g{i}"), &body, source)
        })
        .collect()
}

fn dump(ws: &Sources) -> String {
    ws.iter()
        .map(|(k, s, a)| format!("# {k} {s:?}\n{}", a.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Catches any count or state that disagrees with a plain recount of the
/// triples ([`oracle`]), on thousands of random workspaces.
#[test]
fn every_count_matches_a_naive_recount() {
    for seed in 0..20_000u64 {
        let ws = generated(seed);
        let imports = seed % 2 == 0;
        let mut params = serde_json::json!({ "imports": imports, "maxRows": MAX_ROWS, "maxClasses": MAX_CLASSES });
        // Examples on one class: by `examples`, by the focus, or on none.
        let class = format!("http://example.org/C{}", seed / 3 % 4);
        // Some focuses are instances, which focus their class.
        let instance = format!("http://example.org/i{}", seed / 3 % 4);
        let on_instance = seed % 3 == 1 && seed % 4 >= 2;
        match seed % 3 {
            0 => params["examples"] = class.as_str().into(),
            1 if on_instance => params["focus"] = instance.as_str().into(),
            1 => params["focus"] = class.as_str().into(),
            _ => {}
        }
        let r = run(&ws, params);
        if on_instance && let Err(e) = oracle::instance_focus(&ws, &r, &instance) {
            panic!("seed {seed}: {e}\n{}", dump(&ws));
        }
        let chosen = match seed % 3 {
            0 => r
                .classes
                .iter()
                .find(|c| c.id == class)
                .map(|c| c.id.as_str()),
            1 => r.focus.map(|f| r.classes[f as usize].id.as_str()),
            _ => None,
        };
        if let Err(e) = oracle::check(&ws, &r, imports, chosen) {
            panic!("seed {seed}: {e}\n{}", dump(&ws));
        }
    }
}

/// Catches a cache that drifts: after random edits, an answer from reused
/// facts equals one computed fresh.
#[test]
fn reused_facts_answer_as_fresh_ones() {
    for seed in 0..2_000u64 {
        let mut rng = Rng::new(seed ^ 0xABCD);
        let cache = FactsCache::default();
        let mut ws = generated(seed);
        for step in 0..8u64 {
            match rng.below(4) {
                0 if !ws.is_empty() => {
                    ws.remove(rng.below(ws.len()));
                }
                1 => {
                    let mut extra = generated(seed * 31 + step + 7).swap_remove(0);
                    extra.0 = format!("file:///w/extra{step}.ttl");
                    ws.push(extra);
                }
                _ if !ws.is_empty() => {
                    let i = rng.below(ws.len());
                    let other = generated(seed * 17 + step + 3).swap_remove(0).2;
                    let text = if rng.chance(50) {
                        break_text(&mut rng, &other.text)
                    } else {
                        other.text.to_string()
                    };
                    let (key, source, _) = ws[i].clone();
                    let fresh = Analysis::new(key.clone(), &text, Arc::default(), false);
                    ws[i] = (key, source, Arc::new(fresh));
                }
                _ => {}
            }
            let params = serde_json::json!({
                "focus": format!("http://example.org/{}{}", rng.pick(&["C", "p"]), rng.below(4)),
                "maxClasses": 1 + rng.below(6),
                "maxRows": 1 + rng.below(4),
                "imports": rng.chance(50),
                "search": rng.pick(&["", "c", "C1", "example"]),
            });
            let warm = ask(&ws, params.clone(), &cache);
            let cold = run(&ws, params);
            assert_eq!(warm, cold, "seed {seed} step {step}\n{}", dump(&ws));
        }
    }
}

/// Catches an answer that depends on the order of the sources or on hash
/// order, and indexes out of range under tight caps.
#[test]
fn answers_are_deterministic_and_indexes_in_range() {
    for seed in 0..1_000u64 {
        let ws = generated(seed);
        let a = run(&ws, serde_json::json!({}));
        let mut reversed = ws.clone();
        reversed.reverse();
        assert_eq!(a, run(&reversed, serde_json::json!({})), "seed {seed}");
        let params = serde_json::json!({ "maxClasses": 2, "maxRows": 1, "search": "c" });
        let r = run(&ws, params.clone());
        assert_eq!(r, run(&reversed, params), "seed {seed}");
        assert!(r.classes.len() <= 2, "seed {seed}");
        assert!(r.matches.len() <= MAX_MATCHES);
        for m in &r.matches {
            assert!(m.index.is_none_or(|i| r.classes[i as usize].id == m.id));
        }
        for c in &r.classes {
            assert!(c.rows.len() <= 1);
            assert!(c.parents.iter().all(|&p| (p as usize) < r.classes.len()));
            for row in &c.rows {
                assert!(row.used <= row.of);
                assert!(row.target.is_none_or(|t| (t as usize) < r.classes.len()));
            }
        }
    }
}

/// A workspace of about `triples` triples in two files: an ontology of 40
/// classes in chains, 120 properties with domains and ranges, 10 shapes;
/// instances with a type and five values each.
fn big_workspace(triples: usize) -> Sources {
    let mut onto = String::new();
    for c in 0..40 {
        onto.push_str(&format!("ex:C{c} a owl:Class"));
        if c % 8 != 0 {
            onto.push_str(&format!(" ; rdfs:subClassOf ex:C{}", c - 1));
        }
        onto.push_str(" .\n");
    }
    for p in 0..120 {
        let range = if p % 3 == 0 { "ex:C0" } else { "xsd:integer" };
        onto.push_str(&format!(
            "ex:p{p} rdfs:domain ex:C{} ; rdfs:range {range} .\n",
            p % 40
        ));
    }
    for s in 0..10 {
        onto.push_str(&format!(
            "ex:S{s} a sh:NodeShape ; sh:targetClass ex:C{s} ; \
             sh:property [ sh:path ex:p{s} ; sh:minCount 1 ] .\n"
        ));
    }
    let mut data = String::new();
    let instances = triples / 6 + 1;
    for i in 0..instances {
        data.push_str(&format!("ex:i{i} a ex:C{}", i % 40));
        for k in 0..5 {
            let p = (i + k * 7) % 120;
            if p % 3 == 0 {
                data.push_str(&format!(" ; ex:p{p} ex:i{}", (i + 1) % instances));
            } else {
                data.push_str(&format!(" ; ex:p{p} {}", i % 1000));
            }
        }
        data.push_str(" .\n");
    }
    workspace(&[("onto", &onto), ("data", &data)])
}

/// The answer on a 100,000-triple workspace with every document's facts
/// cached (nothing changed since the last request) is fast: what is left
/// is the merge.
#[test]
fn timing_schema_warm_answer_on_100k_triples() {
    let ws = big_workspace(100_000);
    let triples: usize = ws.iter().map(|(.., a)| a.parsed.triples.len()).sum();
    assert!(triples >= 100_000, "{triples} triples");
    let cache = FactsCache::default();
    let t = std::time::Instant::now();
    let first = ask(&ws, serde_json::json!({}), &cache);
    let cold = t.elapsed();
    let mut times = Vec::new();
    for _ in 0..11 {
        let t = std::time::Instant::now();
        let again = ask(&ws, serde_json::json!({}), &cache);
        times.push(t.elapsed());
        assert_eq!(again, first);
    }
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "schema, {triples} triples: first {cold:?}; warm median {median:?} of {} (min {:?}, max {:?})",
        times.len(),
        times[0],
        times[times.len() - 1]
    );
    assert!(median.as_millis() < 30, "warm answer took {median:?}");
}

/// Catches IRIs written in full when two documents bind one prefix name to
/// different namespaces: each keeps the name its own document gives it.
#[test]
fn a_prefix_name_bound_twice_shortens_both_namespaces() {
    let ws = workspace(&[
        ("a", "ex:Person a owl:Class .\n"),
        (
            "b",
            "@prefix ex: <http://example.org/plant#> .\nex:Pump a owl:Class .\n",
        ),
    ]);
    let r = run(&ws, serde_json::json!({}));
    let names = class_names(&r);
    assert!(names.contains(&"ex:Person"), "{names:?}");
    assert!(names.contains(&"ex:Pump"), "{names:?}");
}

// ---- instance focus, search, conflict values, declared-for ----

/// Catches an instance focus that answers with no focus, picks a type that
/// is not listed, or picks by hash or class order instead of where the
/// types are stated.
#[test]
fn an_instance_focus_is_its_first_listed_type() {
    let ws = workspace(&[
        (
            "a",
            "ex:Pump a owl:Class . ex:Valve a owl:Class .\n\
             ex:p1 a owl:Thing , ex:Valve , ex:Pump .\n\
             ex:v1 a ex:Valve .\n",
        ),
        ("b", "ex:p2 a ex:Pump .\nex:p2 a ex:Valve .\n"),
        ("c", "ex:p2 a ex:Valve .\n"),
    ]);
    let focus = |iri: &str| {
        let r = run(&ws, serde_json::json!({ "focus": iri }));
        let f = r.focus.map(|f| r.classes[f as usize].name.clone());
        (f, r.focus_row)
    };
    // owl:Thing is not listed: the next type, as stated.
    assert_eq!(
        focus("http://example.org/p1"),
        (Some("ex:Valve".into()), None)
    );
    // Typed in b and c: b's first statement.
    assert_eq!(
        focus("http://example.org/p2"),
        (Some("ex:Pump".into()), None)
    );
    assert_eq!(focus("http://example.org/nothing"), (None, None));
    // The focus class lists the examples.
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/v1", "maxClasses": 1 }),
    );
    assert_eq!(class_names(&r), ["ex:Valve"]);
    assert_eq!(r.focus, Some(0));
}

fn found(r: &SchemaResult) -> Vec<(&str, Option<u32>)> {
    r.matches
        .iter()
        .map(|m| (m.name.as_str(), m.index))
        .collect()
}

/// Catches a search over the listed classes only, a ranking that puts a
/// substring before an exact name, ties out of rank order, or a label that
/// is not searched.
#[test]
fn search_ranks_exact_then_prefix_then_substring_over_every_class() {
    let body = "ex:Pump a owl:Class . ex:PumpHouse a owl:Class . ex:SubPump a owl:Class .\n\
        ex:Valve a owl:Class ; rdfs:label \"Pump valve\" .\n\
        ex:Gasket a owl:Class .\n\
        ex:Odd a owl:Class ; rdfs:label \"Not a pump\" .\n\
        ex:v a ex:Valve . ex:g1 a ex:Gasket . ex:g2 a ex:Gasket .\n";
    let ws = workspace(&[("d", body)]);
    let r = run(
        &ws,
        serde_json::json!({ "search": " PUMP ", "maxClasses": 1 }),
    );
    assert_eq!(class_names(&r), ["ex:Gasket"]);
    // Valve (1 instance) before PumpHouse (none) among the prefixes; none
    // is listed, so none has an index.
    assert_eq!(
        found(&r),
        [
            ("ex:Pump", None),
            ("ex:Valve", None),
            ("ex:PumpHouse", None),
            ("ex:Odd", None),
            ("ex:SubPump", None),
        ]
    );
    assert_eq!(r.matches[1].label.as_deref(), Some("Pump valve"));
    assert_eq!(r.matches[1].id, "http://example.org/Valve");
    // A listed class has its index.
    let r = run(&ws, serde_json::json!({ "search": "gask" }));
    assert_eq!(found(&r), [("ex:Gasket", Some(index(&r, "ex:Gasket")))]);
    // The IRI is searched too.
    let r = run(&ws, serde_json::json!({ "search": "example.org/sub" }));
    assert_eq!(found(&r), [("ex:SubPump", Some(index(&r, "ex:SubPump")))]);
    // Blank or absent: no matches, and nothing else changes.
    let plain = run(&ws, serde_json::json!({}));
    for search in ["", "   "] {
        let r = run(&ws, serde_json::json!({ "search": search }));
        assert_eq!(r, plain);
    }
    let r = run(&ws, serde_json::json!({ "search": "nothing like it" }));
    assert!(r.matches.is_empty());
}

/// Catches more than [`MAX_MATCHES`] matches, or imports found when they
/// are not asked for.
#[test]
fn search_stops_at_twenty_and_finds_imports_only_when_asked() {
    let mut body = String::new();
    for i in 0..30 {
        body.push_str(&format!("ex:K{i:02} a owl:Class .\n"));
    }
    let ws = vec![
        doc("d", &body, Source::Disk),
        doc("up", "ex:Kimport a owl:Class .\n", Source::External),
    ];
    let r = run(&ws, serde_json::json!({ "search": "k" }));
    assert_eq!(r.matches.len(), MAX_MATCHES);
    assert_eq!(r.matches[0].name, "ex:K00");
    assert!(r.matches.iter().all(|m| m.name != "ex:Kimport"));
    let r = run(&ws, serde_json::json!({ "search": "kimp" }));
    assert!(r.matches.is_empty());
    let r = run(
        &ws,
        serde_json::json!({ "search": "kimp", "imports": true }),
    );
    assert_eq!(found(&r), [("ex:Kimport", Some(index(&r, "ex:Kimport")))]);
}

const CONFLICTS: &str = "ex:Pump a owl:Class . ex:Valve a owl:Class .\n\
    ex:Sub rdfs:subClassOf ex:Valve .\n\
    ex:flow rdfs:domain ex:Pump ; rdfs:range xsd:decimal .\n\
    ex:feeds rdfs:domain ex:Pump ; rdfs:range ex:Pump .\n\
    ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
      sh:property [ sh:path ex:flow ; sh:datatype xsd:integer ] ;\n\
      sh:property [ sh:path ex:feeds ; sh:class ex:Valve ] .\n\
    ex:v1 a ex:Valve . ex:v2 a ex:Sub .\n\
    ex:p1 a ex:Pump ;\n\
      ex:flow 2 , 1.5 , ex:v1 ;\n\
      ex:feeds ex:v1 , ex:v2 , ex:p1 , \"x\" , [ a ex:Valve ] , [ ex:q 1 ] , ex:nowhere .\n";

/// Catches a conflict row that does not count its values against the
/// shape: literals of another datatype, resources where a datatype is
/// required, values not typed with the class (or a subclass) or literals
/// where a class is required; and their places listed out of order or on
/// the wrong class.
#[test]
fn conflict_rows_count_the_values_outside_the_shape() {
    let ws = workspace(&[("d", CONFLICTS)]);
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/Pump" }),
    );
    let pump = class(&r, "ex:Pump");
    let flow = row(pump, "ex:flow");
    assert_eq!(flow.state, RowState::Conflict);
    // 1.5 and ex:v1; the range and the shape disagree, so no bad values.
    assert_eq!((flow.shape_bad_values, flow.bad_values), (Some(2), None));
    let ex = examples(&ws, flow);
    let body = |n| line(n);
    assert_eq!(
        ex,
        [
            (ExampleKind::ShapeValue, "d.ttl", body(9), "1.5", "ex:p1"),
            (ExampleKind::ShapeValue, "d.ttl", body(9), "ex:v1", "ex:p1"),
        ]
    );
    let feeds = row(pump, "ex:feeds");
    assert_eq!(feeds.state, RowState::Conflict);
    // ex:p1 (a Pump), "x", the untyped blank node, ex:nowhere.
    assert_eq!(feeds.shape_bad_values, Some(4));
    let texts: Vec<&str> = examples(&ws, feeds).iter().map(|e| e.3).collect();
    assert_eq!(texts, ["ex:p1", "\"x\"", "[ ex:q 1 ]", "ex:nowhere"]);
    // Other classes list no places; the count is there all the same.
    let r = run(
        &ws,
        serde_json::json!({ "focus": "http://example.org/Valve" }),
    );
    let flow = row(class(&r, "ex:Pump"), "ex:flow");
    assert_eq!((flow.shape_bad_values, flow.examples.len()), (Some(2), 0));
}

/// Catches `shapeBadValues` on rows that are no conflict, or on a conflict
/// whose values all fit.
#[test]
fn shape_bad_values_only_on_conflict_rows_with_misfits() {
    let body = "ex:Pump a owl:Class .\n\
        ex:flow rdfs:domain ex:Pump ; rdfs:range xsd:decimal .\n\
        ex:size rdfs:domain ex:Pump ; rdfs:range xsd:integer .\n\
        ex:S a sh:NodeShape ; sh:targetClass ex:Pump ;\n\
          sh:property [ sh:path ex:flow ; sh:datatype xsd:integer ] ;\n\
          sh:property [ sh:path ex:size ; sh:datatype xsd:integer ] .\n\
        ex:p a ex:Pump ; ex:flow 2 ; ex:size 1.5 .\n";
    let r = run(&workspace(&[("d", body)]), serde_json::json!({}));
    let pump = class(&r, "ex:Pump");
    let flow = row(pump, "ex:flow");
    assert_eq!(
        (flow.state, flow.shape_bad_values),
        (RowState::Conflict, None)
    );
    let size = row(pump, "ex:size");
    assert_eq!(
        (size.state, size.bad_values, size.shape_bad_values),
        (RowState::BadValues, Some(1), None)
    );
}

/// Catches a not-declared row that does not say which classes do declare
/// its property: by domain, domainIncludes or shape, sorted, at most five.
#[test]
fn undeclared_rows_name_the_classes_that_declare_them() {
    let mut body = String::from(
        "ex:Pipe a owl:Class . ex:Gasket a owl:Class . ex:Valve a owl:Class .\n\
         ex:diameter rdfs:domain ex:Pipe ; schema:domainIncludes ex:Valve .\n\
         ex:S a sh:NodeShape ; sh:targetClass ex:Hose ; sh:property [ sh:path ex:diameter ] .\n\
         ex:g1 a ex:Gasket ; ex:diameter 3 ; ex:color \"red\" ; ex:mass 1 .\n",
    );
    for c in ["F", "E", "D", "C", "B", "A"] {
        body.push_str(&format!("ex:mass rdfs:domain ex:{c} .\n"));
    }
    let r = run(&workspace(&[("d", &body)]), serde_json::json!({}));
    let gasket = class(&r, "ex:Gasket");
    let diameter = row(gasket, "ex:diameter");
    assert_eq!(diameter.state, RowState::Undeclared);
    assert_eq!(diameter.declared_for, ["ex:Hose", "ex:Pipe", "ex:Valve"]);
    assert!(row(gasket, "ex:color").declared_for.is_empty());
    assert_eq!(
        row(gasket, "ex:mass").declared_for,
        ["ex:A", "ex:B", "ex:C", "ex:D", "ex:E"]
    );
    // Declared rows say nothing.
    assert!(
        row(class(&r, "ex:Pipe"), "ex:diameter")
            .declared_for
            .is_empty()
    );
}

/// `search` over 2,000 classes, each labelled, adds under 5 ms to a warm
/// answer.
#[test]
fn timing_schema_search_on_2000_classes() {
    let mut body = String::new();
    for c in 0..2_000 {
        body.push_str(&format!(
            "ex:Class{c} a owl:Class ; rdfs:label \"Class number {c}\" .\nex:i{c} a ex:Class{c} .\n"
        ));
    }
    let ws = workspace(&[("d", &body)]);
    let cache = FactsCache::default();
    let median = |params: serde_json::Value| {
        let mut times: Vec<_> = (0..11)
            .map(|_| {
                let t = std::time::Instant::now();
                ask(&ws, params.clone(), &cache);
                t.elapsed()
            })
            .collect();
        times.sort();
        times[times.len() / 2]
    };
    let plain = median(serde_json::json!({}));
    let searched = median(serde_json::json!({ "search": "number 1" }));
    eprintln!("schema, 2,000 classes: warm median {plain:?}, with search {searched:?}");
    assert!(
        searched.saturating_sub(plain).as_millis() < 5,
        "search added {:?}",
        searched.saturating_sub(plain)
    );
}

/// Catches role classes flagged: an instance typed with two classes uses a
/// property one of them declares; the other type gets no "not declared" row.
#[test]
fn a_property_one_of_an_instances_types_declares_is_declared_for_it() {
    let ws = workspace(&[(
        "a",
        "ex:Device a owl:Class .\nex:Asset a owl:Class .\n\
         ex:locatedIn rdfs:domain ex:Device .\n\
         ex:d1 a ex:Device , ex:Asset ; ex:locatedIn ex:here .\n\
         ex:a1 a ex:Asset ; ex:locatedIn ex:there .\n",
    )]);
    let r = run(&ws, serde_json::json!({}));
    assert_eq!(
        row(class(&r, "ex:Device"), "ex:locatedIn").state,
        RowState::Ok
    );
    // ex:a1 is an Asset only: for it, nothing declares the property.
    let asset = row(class(&r, "ex:Asset"), "ex:locatedIn");
    assert_eq!(asset.state, RowState::Undeclared);
    assert_eq!(asset.declared_for, ["ex:Device"]);
    assert_eq!(
        (asset.used, asset.of),
        (1, 2),
        "only ex:a1 uses it undeclared"
    );
}
