// Canned `turtle/graph` answers for the graph view's tests (graph-ui.test.mjs
// in Node, graph-ui-page.mjs in the browser): a small graph with known
// positions, and a big one of any size; and a `turtle/schema` answer of any
// number of classes (the small one is schema-fixture.json).

export const EX = "http://example.org/"
const RDFS = "http://www.w3.org/2000/01/rdf-schema#"
export const SUBCLASS = `${RDFS}subClassOf`
export const A = "file:///work/a.ttl"
export const B = "file:///work/b.ttl"
const range = (line) => ({ start: { line, character: 0 }, end: { line, character: 6 } })

/**
 * Pump in the middle at (0, 0) of a box from (-200, -150) to (200, 150), so a
 * fit puts it at the canvas centre; 50 sensors on a circle of radius 120 feed
 * it (more than ring 1's cap of 40).
 */
export function smallGraph({ sensors = 50 } = {}) {
  const nodes = [
    { id: `${EX}Pump`, label: "Pump", name: "ex:Pump", kind: "class", uri: A, range: range(3), literal: false },
    { id: `${EX}Device`, name: "ex:Device", kind: "class", uri: A, range: range(1), literal: false },
    { id: `${EX}Valve`, label: "Valve", name: "ex:Valve", kind: "class", uri: B, range: range(2), literal: false },
    { id: `${EX}hasPart`, name: "ex:hasPart", kind: "property", uri: A, range: range(8), literal: false },
    { id: `${EX}PumpShape`, name: "ex:PumpShape", kind: "shape", uri: B, range: range(5), literal: false },
  ]
  const positions = [0, 0, -200, -150, 200, 150, -200, 150, 200, -150]
  const edges = [
    { source: 0, target: 1, predicate: SUBCLASS, name: "rdfs:subClassOf", uri: A },
    { source: 2, target: 1, predicate: SUBCLASS, name: "rdfs:subClassOf", uri: B },
    { source: 0, target: 2, predicate: `${EX}hasPart`, name: "ex:hasPart", uri: A },
    { source: 4, target: 0, predicate: "http://www.w3.org/ns/shacl#targetClass", name: "sh:targetClass", uri: B },
  ]
  for (let k = 0; k < sensors; k++) {
    nodes.push({ id: `${EX}sensor${k}`, name: `ex:sensor${k}`, kind: "individual", uri: B, range: range(20 + k), literal: false })
    const a = (k / sensors) * Math.PI * 2 + 0.3
    positions.push(Math.round(Math.cos(a) * 120 * 100) / 100, Math.round(Math.sin(a) * 120 * 100) / 100)
    edges.push({ source: nodes.length - 1, target: 0, predicate: `${EX}feeds`, name: "ex:feeds", uri: B })
  }
  return {
    nodes,
    edges,
    positions,
    total: { nodes: nodes.length, edges: edges.length },
    facets: {
      files: [{ uri: A, nodes: 4 }, { uri: B, nodes: nodes.length - 4 }],
      namespaces: [{ iri: EX, prefix: "ex", nodes: nodes.length }],
      predicates: [
        { iri: `${EX}feeds`, name: "ex:feeds", edges: sensors },
        { iri: SUBCLASS, name: "rdfs:subClassOf", edges: 2 },
        { iri: `${EX}hasPart`, name: "ex:hasPart", edges: 1 },
      ],
    },
  }
}

/** `n` nodes scattered over a square (a fixed pseudo-random sequence), about 1.5 edges each; `total` nodes before the cap. */
export function bigGraph(n, total = n) {
  let seed = 7
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648)
  const kinds = ["class", "property", "individual", "shape", "concept", "other"]
  const nodes = []
  const positions = []
  const side = Math.sqrt(n) * 40
  for (let i = 0; i < n; i++) {
    nodes.push({ id: `${EX}n${i}`, label: i % 3 ? undefined : `Node ${i}`, name: `ex:n${i}`, kind: kinds[i % kinds.length], uri: i % 2 ? A : B, range: range(i), literal: false })
    positions.push((rnd() - 0.5) * side, (rnd() - 0.5) * side)
  }
  const edges = []
  for (let i = 1; i < n; i++) {
    edges.push({ source: i, target: Math.floor(rnd() * i), predicate: SUBCLASS, name: "rdfs:subClassOf", uri: A })
    if (i % 2) edges.push({ source: i, target: Math.floor(rnd() * n), predicate: `${EX}rel`, name: "ex:rel", uri: B })
  }
  return {
    nodes,
    edges,
    positions,
    total: { nodes: total, edges: edges.length },
    facets: {
      files: [{ uri: A, nodes: n / 2 }, { uri: B, nodes: n / 2 }],
      namespaces: [{ iri: EX, prefix: "ex", nodes: n }],
      predicates: [{ iri: SUBCLASS, name: "rdfs:subClassOf", edges: n - 1 }, { iri: `${EX}rel`, name: "ex:rel", edges: Math.floor(n / 2) }],
    },
  }
}

const XSD = "http://www.w3.org/2001/XMLSchema#"
const STATES = ["ok", "ok", "ok", "unused", "undeclared", "badValues", "conflict"]

/**
 * A `turtle/schema` answer of `n` classes: a subclass tree three wide (every
 * tenth class a root, every seventh with a second parent), four to eight rows
 * each in every state, a link row to another class in most.
 */
export function bigSchema(n) {
  let seed = 11
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648)
  const classes = []
  for (let i = 0; i < n; i++) {
    const parents = i % 10 === 0 || i === 0 ? [] : [Math.floor((i - 1) / 3)]
    if (i % 7 === 6 && i > 2) parents.push(i - 2)
    const of = 10 + Math.floor(rnd() * 500)
    const rows = []
    const nrows = 4 + Math.floor(rnd() * 5)
    for (let k = 0; k < nrows; k++) {
      const state = STATES[Math.floor(rnd() * STATES.length)]
      const link = k === 0 && n > 1
      const target = link ? Math.floor(rnd() * n) : undefined
      const row = {
        property: `${EX}c${i}p${k}`,
        name: `ex:c${i}p${k}`,
        link,
        ...(link ? { target, range: `${EX}C${target}`, rangeName: `ex:C${target}` } : { range: `${XSD}decimal`, rangeName: "xsd:decimal" }),
        declared: state === "undeclared" ? [] : [{ how: "domain", uri: A, range: range(i * 10 + k) }],
        used: state === "unused" ? 0 : Math.floor(of * rnd()),
        of,
        state,
      }
      if (state === "badValues") row.badValues = 1 + Math.floor(rnd() * 20)
      if (state === "conflict") row.conflict = { declared: "xsd:decimal", shape: "xsd:integer" }
      if (k === 1) (row.required = { min: 1, max: 1 }), (row.missing = i % 4 === 0 ? 2 : 0)
      rows.push(row)
    }
    const problems = rows.filter((r) => r.state !== "ok").length + rows.filter((r) => r.missing > 0).length
    classes.push({ id: `${EX}C${i}`, name: `ex:C${i}`, label: `Class ${i}`, uri: i % 2 ? A : B, range: range(i), kind: "class", parents, instances: of, problems, rows, moreRows: 0 })
  }
  return { classes, focus: 0, total: { classes: n }, imported: 0 }
}
