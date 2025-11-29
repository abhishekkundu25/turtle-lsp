import {
	CompletionItemKind,
	CompletionItem,
	TextDocument,
	TextDocumentPositionParams,
} from "vscode-languageserver/node"
import { getCommonCompletionItemsGivenNamespaces } from "stardog-language-utils"
import { Indexer } from "./indexer"
import { abbreviateWithPrefixes, currentWord, currentWordRange, detectPrefixAtPosition, extractPrefix } from "./util"

const vocabTerms: Record<string, { classes?: string[]; properties?: string[] }> = {
	rdf: { properties: ["type", "value", "subject", "predicate", "object"] },
	rdfs: {
		classes: ["Class", "Datatype", "Resource", "Literal"],
		properties: [
			"label",
			"comment",
			"subClassOf",
			"subPropertyOf",
			"range",
			"domain",
			"seeAlso",
			"isDefinedBy",
			"member",
			"first",
			"rest",
		],
	},
	owl: {
		classes: ["Class", "ObjectProperty", "DatatypeProperty", "AnnotationProperty", "Restriction", "Ontology"],
		properties: [
			"sameAs",
			"equivalentClass",
			"equivalentProperty",
			"disjointWith",
			"intersectionOf",
			"unionOf",
			"onProperty",
			"someValuesFrom",
			"allValuesFrom",
			"hasValue",
			"minCardinality",
			"maxCardinality",
			"cardinality",
			"inverseOf",
			"onClass",
			"onDatatype",
			"withRestrictions",
		],
	},
	xsd: {
		classes: ["string", "boolean", "integer", "decimal", "date", "dateTime", "time", "anyURI", "float", "double", "duration"],
	},
	skos: {
		classes: ["Concept", "ConceptScheme", "Collection"],
		properties: ["prefLabel", "altLabel", "hiddenLabel", "broader", "narrower", "related", "inScheme"],
	},
	sh: {
		classes: ["Shape", "NodeShape", "PropertyShape"],
		properties: ["path", "datatype", "class", "nodeKind", "minCount", "maxCount", "pattern", "minLength", "maxLength"],
	},
	dcterms: {
		properties: ["title", "creator", "subject", "description", "publisher", "contributor", "date", "type", "format", "identifier"],
	},
	foaf: {
		classes: ["Person", "Agent", "Group", "Document"],
		properties: ["name", "nick", "mbox", "homepage", "knows"],
	},
}

export class CompletionEngine {
	constructor(private indexer: Indexer) {}

	build(params: TextDocumentPositionParams, doc: TextDocument, namespaces: any[], namespaceMap: Record<string, string>) {
		const text = doc.getText()
		const subjectSet = this.collectSubjectSet(text)
		const current = currentWord(doc, params.position) || ""
		const currentPrefix = detectPrefixAtPosition(doc, params.position, current)
		const replaceRange = currentWordRange(doc, params.position)
		const globalSubjects = this.indexer.collectGlobalSubjects(currentPrefix || "")

		const prefixItems = namespaces.map(({ prefix, iri }) => {
			const label = `${prefix}:`
			return {
				label,
				kind: CompletionItemKind.Keyword,
				detail: iri,
				textEdit: replaceRange ? { newText: label, range: replaceRange } : undefined,
				insertText: replaceRange ? undefined : label,
				sortText: `400_${label}`,
			}
		})

		const keywordItems = [
			{ label: "@prefix", kind: CompletionItemKind.Keyword, detail: "Declare namespace prefix", insertText: "@prefix ", sortText: "500_prefix" },
			{ label: "@base", kind: CompletionItemKind.Keyword, detail: "Set base IRI", insertText: "@base ", sortText: "500_base" },
			{ label: "a", kind: CompletionItemKind.Keyword, detail: "rdf:type shortcut", insertText: "a ", sortText: "500_a" },
			{ label: "BASE", kind: CompletionItemKind.Keyword, sortText: "500_BASE" },
			{ label: "PREFIX", kind: CompletionItemKind.Keyword, sortText: "500_PREFIX" },
		]

		const subjectItems = Array.from(new Set([...subjectSet, ...globalSubjects])).map((subject) => {
			const display = abbreviateWithPrefixes(subject, namespaceMap)
			return {
				label: display,
				kind: CompletionItemKind.Reference,
				textEdit: replaceRange ? { newText: display, range: replaceRange } : undefined,
				insertText: replaceRange ? undefined : display,
			}
		})

		const vocabItemsRaw = [
			...(getCommonCompletionItemsGivenNamespaces(namespaceMap).classes || []),
			...(getCommonCompletionItemsGivenNamespaces(namespaceMap).properties || []),
		]

		const normalizeVocab = (item: any) => {
			const label = item.label || item.insertText || ""
			const prefix = extractPrefix(label)
			const textEdit = replaceRange ? { newText: label, range: replaceRange } : undefined
			const insertText = replaceRange ? undefined : item.insertText ?? label
			const sortBucket = prefix && prefix === currentPrefix ? "020" : "350"
			return {
				...item,
				label,
				textEdit,
				insertText,
				sortText: `${sortBucket}_${label}`,
			}
		}

		const vocabItems = vocabItemsRaw.map(normalizeVocab).concat(this.vocabFromPrefix(currentPrefix, replaceRange))

		const allItems: CompletionItem[] = [...vocabItems, ...subjectItems, ...prefixItems, ...keywordItems]
		const withSort = allItems.map((item) => {
			const label = item.label || ""
			const prefix = extractPrefix(label)
			let bucket = "900"
			if (item.kind === CompletionItemKind.Reference && (currentPrefix === "" || currentPrefix === ":")) {
				bucket = "010"
			} else if (item.kind === CompletionItemKind.Reference) {
				bucket = "120"
			} else if (prefix && prefix === currentPrefix) {
				bucket = "020"
			} else if (item.kind === CompletionItemKind.Keyword && label.startsWith("@")) {
				bucket = "500"
			} else if (item.kind === CompletionItemKind.Keyword) {
				bucket = "450"
			} else if (!prefix && label.startsWith(":")) {
				bucket = "030"
			}
			return { ...item, sortText: item.sortText || `${bucket}_${label}` }
		})

		const deduped: CompletionItem[] = []
		const seen = new Set<string>()
		for (const item of withSort) {
			const key = item.label
			if (seen.has(key)) continue
			seen.add(key)
			deduped.push(item)
		}

		return deduped.sort((a, b) => (a.sortText || "").localeCompare(b.sortText || ""))
	}

	private collectSubjectSet(text: string): Set<string> {
		const lines = text.split(/\r?\n/)
		const subjects = new Set<string>()

		lines.forEach((line) => {
			const trimmed = line.trim()
			if (!trimmed || trimmed.startsWith("@") || trimmed.startsWith("#")) return

			const subjectMatch = trimmed.match(/^([<A-Za-z][^\s>]*)/)
			if (subjectMatch) {
				subjects.add(subjectMatch[1])
			}
		})

		return subjects
	}

	private vocabFromPrefix(currentPrefix: string, replaceRange: any) {
		if (!currentPrefix || !vocabTerms[currentPrefix]) return []
		const terms = vocabTerms[currentPrefix]
		const items: any[] = []

		for (const cls of terms.classes || []) {
			const label = `${currentPrefix}:${cls}`
			items.push({
				label,
				kind: CompletionItemKind.Class,
				textEdit: replaceRange ? { newText: label, range: replaceRange } : undefined,
				insertText: replaceRange ? undefined : label,
				detail: `${currentPrefix} class`,
			})
		}

		for (const prop of terms.properties || []) {
			const label = `${currentPrefix}:${prop}`
			items.push({
				label,
				kind: CompletionItemKind.Property,
				textEdit: replaceRange ? { newText: label, range: replaceRange } : undefined,
				insertText: replaceRange ? undefined : label,
				detail: `${currentPrefix} property`,
			})
		}

		return items
	}
}
