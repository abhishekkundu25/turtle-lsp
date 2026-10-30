//! The schema recounted the slow, obvious way, from the parses' triples
//! with strings and ordered collections, for the random workspaces of
//! [`super::generated`]. Shapes are read straight from the triples, in the
//! forms the generator writes.

use super::*;
use std::collections::{BTreeMap as Map, BTreeSet as Set};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const SH: &str = "http://www.w3.org/ns/shacl#";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum O {
    Iri(String),
    Blank(String),
    Literal(String, String),
    Triple,
}

struct T {
    ws: bool,
    s: String,
    p: String,
    o: O,
}

#[derive(Default)]
struct ShapeRow {
    min: Option<u32>,
    max: Option<u32>,
    datatype: Option<String>,
    class: Option<String>,
}

fn datatype(i: &str) -> bool {
    i.starts_with(XSD)
        || i == format!("{RDFS}Literal")
        || ["langString", "dirLangString", "HTML", "XMLLiteral", "JSON"]
            .iter()
            .any(|l| i == format!("http://www.w3.org/1999/02/22-rdf-syntax-ns#{l}"))
}

fn checkable(i: &str) -> bool {
    datatype(i) && i != format!("{RDFS}Literal")
}

pub(super) fn check(ws: &Sources, r: &SchemaResult, imports: bool) -> Result<(), String> {
    // Every triple, blank nodes named per document.
    let mut ts: Vec<T> = Vec::new();
    for (d, (_, source, a)) in ws.iter().enumerate() {
        for t in &a.parsed.triples {
            if t.unresolved {
                continue;
            }
            let node = |n: &crate::syntax::Node| match n {
                crate::syntax::Node::Iri(i) => O::Iri(i.to_string()),
                crate::syntax::Node::Blank(b) => O::Blank(format!("_:{d}:{}", b.0)),
                crate::syntax::Node::Literal { datatype, .. } => {
                    O::Literal(String::new(), datatype.to_string())
                }
                crate::syntax::Node::Triple(_) => O::Triple,
            };
            let s = match node(&t.s) {
                O::Iri(s) | O::Blank(s) => s,
                _ => continue,
            };
            ts.push(T {
                ws: *source != Source::External,
                s,
                p: t.p.to_string(),
                o: node(&t.o),
            });
        }
    }
    let iri_s = |t: &T| t.s.starts_with("http") || !t.s.starts_with("_:");
    let obj = |t: &T| match &t.o {
        O::Iri(i) => Some(i.clone()),
        _ => None,
    };

    // Classes: declared where, used where.
    let mut declared_ws: Set<String> = Set::new();
    let mut declared_ext: Set<String> = Set::new();
    let mut used_ws: Set<String> = Set::new();
    let mut supers: Map<String, Set<String>> = Map::new();
    for t in &ts {
        let Some(o) = obj(t) else { continue };
        let is_class = t.p == RDF_TYPE && (o == format!("{RDFS}Class") || o == OWL_CLASS);
        let sub = t.p == format!("{RDFS}subClassOf");
        if (is_class || sub) && iri_s(t) {
            if t.ws {
                declared_ws.insert(t.s.clone());
            } else {
                declared_ext.insert(t.s.clone());
            }
        }
        if sub && iri_s(t) {
            if t.s != o {
                supers.entry(t.s.clone()).or_default().insert(o.clone());
            }
            if t.ws {
                used_ws.insert(t.s.clone());
                used_ws.insert(o.clone());
            }
        }
        let domain = t.p == format!("{RDFS}domain") || t.p.ends_with("schema.org/domainIncludes");
        if t.ws && (t.p == RDF_TYPE || t.p == format!("{SH}targetClass") || (domain && iri_s(t))) {
            used_ws.insert(o.clone());
        }
    }
    let ancestors = |c: &str| -> Set<String> {
        let mut seen = Set::from([c.to_string()]);
        let mut stack = vec![c.to_string()];
        while let Some(x) = stack.pop() {
            for s in supers.get(&x).into_iter().flatten() {
                if seen.insert(s.clone()) {
                    stack.push(s.clone());
                }
            }
        }
        seen
    };

    // Properties: ranges, OWL types.
    let mut ranges: Map<String, Set<String>> = Map::new();
    let mut object_props: Set<String> = Set::new();
    let mut datatype_props: Set<String> = Set::new();
    for t in &ts {
        if let Some(o) = obj(t)
            && iri_s(t)
        {
            if t.p == format!("{RDFS}range") {
                ranges.entry(t.s.clone()).or_default().insert(o.clone());
            }
            if t.p == RDF_TYPE && o == OWL_OBJECT_PROPERTY {
                object_props.insert(t.s.clone());
            }
            if t.p == RDF_TYPE && o == OWL_DATATYPE_PROPERTY {
                datatype_props.insert(t.s.clone());
            }
        }
    }

    // Declared rows: (class, property) -> how, and the shapes' parameters.
    let mut declared: Map<(String, String), Vec<DeclaredBy>> = Map::new();
    let mut shapes: Map<(String, String), Vec<ShapeRow>> = Map::new();
    let mut written: Map<String, usize> = Map::new();
    for t in &ts {
        let Some(o) = obj(t) else { continue };
        if !iri_s(t) {
            continue;
        }
        let how = if t.p == format!("{RDFS}domain") {
            DeclaredBy::Domain
        } else if t.p.ends_with("schema.org/domainIncludes") {
            DeclaredBy::DomainIncludes
        } else {
            continue;
        };
        declared.entry((o, t.s.clone())).or_default().push(how);
    }
    let out = |s: &str, p: &str| -> Vec<&O> {
        ts.iter()
            .filter(|t| t.s == s && t.p == p)
            .map(|t| &t.o)
            .collect()
    };
    let is_shape_class = |s: &str| {
        !s.starts_with("_:")
            && out(s, RDF_TYPE).contains(&&O::Iri(format!("{SH}NodeShape")))
            && out(s, RDF_TYPE)
                .iter()
                .any(|o| **o == O::Iri(OWL_CLASS.into()) || **o == O::Iri(format!("{RDFS}Class")))
    };
    let mut targeting: Map<String, Set<String>> = Map::new();
    for t in &ts {
        if t.p == format!("{SH}targetClass")
            && let Some(o) = obj(t)
        {
            targeting.entry(t.s.clone()).or_default().insert(o);
        }
        if is_shape_class(&t.s) {
            targeting
                .entry(t.s.clone())
                .or_default()
                .insert(t.s.clone());
        }
    }
    for (shape, classes) in &targeting {
        let mut property_shapes: Vec<String> = vec![shape.clone()];
        for o in out(shape, &format!("{SH}property")) {
            if let O::Blank(b) | O::Iri(b) = o {
                property_shapes.push(b.clone());
            }
        }
        for ps in property_shapes {
            let Some(path) = out(&ps, &format!("{SH}path")).first().copied().cloned() else {
                continue;
            };
            let first_iri = |p: &str| {
                out(&ps, &format!("{SH}{p}"))
                    .into_iter()
                    .find_map(|o| match o {
                        O::Iri(i) => Some(i.clone()),
                        _ => None,
                    })
            };
            // The literal's text is not kept in `O`: read it again.
            let count = |p: &str| ts_count(ws, &ps, &format!("{SH}{p}"));
            let row = ShapeRow {
                min: count("minCount"),
                max: count("maxCount"),
                datatype: first_iri("datatype"),
                class: first_iri("class"),
            };
            for c in classes {
                match &path {
                    O::Iri(p) if p != RDF_NIL => {
                        declared
                            .entry((c.clone(), p.clone()))
                            .or_default()
                            .push(DeclaredBy::Shape);
                        shapes
                            .entry((c.clone(), p.clone()))
                            .or_default()
                            .push(ShapeRow {
                                min: row.min,
                                max: row.max,
                                datatype: row.datatype.clone(),
                                class: row.class.clone(),
                            });
                    }
                    _ => *written.entry(c.clone()).or_default() += 1,
                }
            }
        }
    }

    // Subjects: their direct types and their values.
    let mut types: Map<String, Set<String>> = Map::new();
    for t in &ts {
        if t.p == RDF_TYPE
            && let Some(o) = obj(t)
        {
            types.entry(t.s.clone()).or_default().insert(o);
        }
    }
    let mut values: Map<(String, String), Vec<O>> = Map::new();
    for t in &ts {
        if t.p != RDF_TYPE && types.contains_key(&t.s) {
            values
                .entry((t.s.clone(), t.p.clone()))
                .or_default()
                .push(t.o.clone());
        }
    }
    let mut members: Map<String, Set<String>> = Map::new();
    for (s, ty) in &types {
        let mut m = Set::new();
        for t in ty {
            m.extend(ancestors(t));
        }
        members.insert(s.clone(), m);
    }
    let instances = |c: &str| -> Vec<&String> {
        members
            .iter()
            .filter(|(_, m)| m.contains(c))
            .map(|(s, _)| s)
            .collect()
    };

    // Undeclared rows, on the direct types.
    let mut rows: Map<String, Set<String>> = Map::new();
    for (c, p) in declared.keys() {
        rows.entry(c.clone()).or_default().insert(p.clone());
    }
    let mut undeclared: Set<(String, String)> = Set::new();
    for (s, p) in values.keys() {
        if super::super::is_standard(p) {
            continue;
        }
        for t in &types[s] {
            let covered = ancestors(t)
                .iter()
                .any(|a| declared.contains_key(&(a.clone(), p.clone())));
            if !covered {
                undeclared.insert((t.clone(), p.clone()));
                rows.entry(t.clone()).or_default().insert(p.clone());
            }
        }
    }

    // Which classes are listed, and how.
    let all: Set<String> = declared_ws
        .iter()
        .chain(&declared_ext)
        .chain(&used_ws)
        .cloned()
        .collect();
    let mut kinds: Map<String, ClassKind> = Map::new();
    let mut imported = 0;
    for c in &all {
        let kind = if declared_ws.contains(c) {
            Some(ClassKind::Class)
        } else if declared_ext.contains(c) {
            Some(ClassKind::Imported)
        } else if !used_ws.contains(c) {
            None
        } else if super::super::is_standard(c) {
            declared
                .keys()
                .any(|(k, _)| k == c)
                .then_some(ClassKind::Class)
        } else {
            Some(ClassKind::Undefined)
        };
        match kind {
            Some(ClassKind::Imported) if !imports => imported += 1,
            Some(k) => {
                kinds.insert(c.clone(), k);
            }
            None => {}
        }
    }

    // Compare.
    let listed: Map<String, ClassKind> = r.classes.iter().map(|c| (c.id.clone(), c.kind)).collect();
    if listed != kinds {
        return Err(format!("classes: got {listed:?}, recounted {kinds:?}"));
    }
    if r.total.classes != kinds.len() || r.imported != imported {
        return Err(format!(
            "total {} imported {}, recounted {} {imported}",
            r.total.classes,
            r.imported,
            kinds.len()
        ));
    }
    for b in &r.classes {
        let c = &b.id;
        let inst = instances(c);
        let of = inst.len();
        if b.instances != of {
            return Err(format!("{c}: instances {} recounted {of}", b.instances));
        }
        let parents: Set<&str> = b
            .parents
            .iter()
            .map(|&i| r.classes[i as usize].id.as_str())
            .collect();
        let want: Set<&str> = supers
            .get(c)
            .into_iter()
            .flatten()
            .filter(|s| kinds.contains_key(*s))
            .map(String::as_str)
            .collect();
        if parents != want {
            return Err(format!("{c}: parents {parents:?} recounted {want:?}"));
        }
        let got: Set<&str> = b
            .rows
            .iter()
            .filter(|r| r.property.starts_with("http"))
            .map(|r| r.property.as_str())
            .collect();
        let want: Set<&str> = rows
            .get(c)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if got != want {
            return Err(format!("{c}: rows {got:?} recounted {want:?}"));
        }
        let n_written = b.rows.len() - got.len();
        if n_written > written.get(c).copied().unwrap_or(0) {
            return Err(format!("{c}: {n_written} written rows"));
        }
        let mut problems = 0;
        for row in &b.rows {
            let p = &row.property;
            if !p.starts_with("http") {
                continue;
            }
            let key = (c.clone(), p.clone());
            let vals = |s: &String| {
                values
                    .get(&(s.clone(), p.clone()))
                    .map_or(&[][..], Vec::as_slice)
            };
            let used = if of == 0 {
                0
            } else {
                inst.iter().filter(|s| !vals(s).is_empty()).count()
            };
            let rng = ranges.get(p).cloned().unwrap_or_default();
            let sh = shapes.get(&key).map_or(&[][..], Vec::as_slice);
            let sh_dt: Set<String> = sh.iter().filter_map(|s| s.datatype.clone()).collect();
            let sh_class: Set<String> = sh.iter().filter_map(|s| s.class.clone()).collect();
            let dts: Set<&String> = rng.iter().chain(&sh_dt).filter(|d| checkable(d)).collect();
            let check = (dts.len() == 1)
                .then(|| dts.iter().next().copied().cloned())
                .flatten();
            let bad = match &check {
                Some(dt) if of > 0 => inst
                    .iter()
                    .flat_map(|s| vals(s))
                    .filter(|v| matches!(v, O::Literal(_, d) if d != dt))
                    .count(),
                _ => 0,
            };
            let conflict = !rng.is_empty()
                && !sh.is_empty()
                && sh_dt.iter().chain(&sh_class).any(|v| !rng.contains(v));
            let hows = declared.get(&key).cloned().unwrap_or_default();
            let state = if hows.is_empty() {
                RowState::Undeclared
            } else if conflict {
                RowState::Conflict
            } else if bad > 0 {
                RowState::BadValues
            } else if of > 0 && used == 0 {
                RowState::Unused
            } else {
                RowState::Ok
            };
            let min = sh.iter().filter_map(|s| s.min).max().filter(|&m| m >= 1);
            let max = sh.iter().filter_map(|s| s.max).min();
            let missing = min
                .map(|m| inst.iter().filter(|s| vals(s).len() < m as usize).count())
                .filter(|&n| n > 0);
            let range = rng
                .iter()
                .next()
                .or(sh_class.iter().next())
                .or(sh_dt.iter().next());
            let link = if let Some(first) = rng.iter().next() {
                !datatype(first)
            } else if object_props.contains(p) {
                true
            } else if datatype_props.contains(p) {
                false
            } else if !sh_class.is_empty() {
                true
            } else if !sh_dt.is_empty() {
                false
            } else {
                inst.iter()
                    .flat_map(|s| vals(s))
                    .any(|v| matches!(v, O::Iri(_) | O::Blank(_)))
            };
            let mut got_hows: Vec<DeclaredBy> = row.declared.iter().map(|d| d.how).collect();
            let mut want_hows = hows.clone();
            got_hows.sort_by_key(|h| *h as u8);
            want_hows.sort_by_key(|h| *h as u8);
            let got = (
                row.used,
                row.of,
                row.state,
                row.bad_values.unwrap_or(0),
                row.missing,
                row.required.map(|r| (r.min, r.max)),
                row.range.as_ref(),
                row.link,
                got_hows,
            );
            let want = (
                used,
                of,
                state,
                bad,
                missing,
                min.map(|m| (m, max)),
                range,
                link,
                want_hows,
            );
            if got != want {
                return Err(format!(
                    "{c} {p}: (used, of, state, bad, missing, required, range, link, how) got {got:?}, recounted {want:?}"
                ));
            }
            if state != RowState::Ok || missing.is_some() {
                problems += 1;
            }
            if undeclared.contains(&key) != (state == RowState::Undeclared) {
                return Err(format!("{c} {p}: undeclared mismatch"));
            }
        }
        if b.problems != problems {
            return Err(format!("{c}: problems {} recounted {problems}", b.problems));
        }
    }
    Ok(())
}

/// The first `p` value of `s` whose text is a whole number, across the
/// documents (blank nodes as [`check`] names them).
fn ts_count(ws: &Sources, s: &str, p: &str) -> Option<u32> {
    for (d, (.., a)) in ws.iter().enumerate() {
        for t in &a.parsed.triples {
            if t.unresolved || t.p.as_str() != p {
                continue;
            }
            let subject = match &t.s {
                crate::syntax::Node::Iri(i) => i.to_string(),
                crate::syntax::Node::Blank(b) => format!("_:{d}:{}", b.0),
                _ => continue,
            };
            if subject != s {
                continue;
            }
            if let crate::syntax::Node::Literal { value, .. } = &t.o
                && let Ok(n) = value.trim().parse::<u32>()
            {
                return Some(n);
            }
        }
    }
    None
}
