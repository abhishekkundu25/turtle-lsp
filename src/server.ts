#!/usr/bin/env node

import {
	createConnection,
	ProposedFeatures,
	TextDocuments,
	TextDocumentSyncKind,
	FoldingRangeParams,
	TextDocumentChangeEvent,
} from "vscode-languageserver/node"
import { TextDocument } from "vscode-languageserver-textdocument"
import { AbstractLanguageServer, errorMessageProvider } from "stardog-language-utils"
import { TurtleParser, ModeString } from "millan"
import { CompletionEngine } from "./completion"
import { Indexer } from "./indexer"
import { NavigationEngine } from "./navigation"
import { detectPrefixAtPosition } from "./util"

const defaultPrefixes: Record<string, string> = {
	rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
	rdfs: "http://www.w3.org/2000/01/rdf-schema#",
	owl: "http://www.w3.org/2002/07/owl#",
	xsd: "http://www.w3.org/2001/XMLSchema#",
	sh: "http://www.w3.org/ns/shacl#",
	skos: "http://www.w3.org/2004/02/skos/core#",
	dcterms: "http://purl.org/dc/terms/",
	foaf: "http://xmlns.com/foaf/0.1/",
}

class TurtleLanguageServer extends AbstractLanguageServer<TurtleParser> {
	private mode: ModeString = "standard"
	private indexer: Indexer
	private completionEngine: CompletionEngine
	private navigation: NavigationEngine

	constructor(private conn: ReturnType<typeof createConnection>, workspaceRoot?: string) {
		super(conn as any, new TurtleParser({ errorMessageProvider }))
		this.indexer = new Indexer(workspaceRoot)
		this.completionEngine = new CompletionEngine(this.indexer)
		this.navigation = new NavigationEngine(this.indexer, this.documents)
	}

	onInitialization() {
		this.conn.onFoldingRanges((params: FoldingRangeParams) => this.handleFoldingRanges(params, true, true))
		this.conn.onCompletion((params) => {
			const doc = this.documents.get(params.textDocument.uri)
			if (!doc) return []
			const text = doc.getText()
			const namespaces = this.collectNamespaceEntries(text)
			const namespaceMap = this.buildPrefixMap(text, namespaces)
			return this.completionEngine.build(params, doc, namespaces, namespaceMap)
		})
		this.conn.onDocumentSymbol((params) => {
			const doc = this.documents.get(params.textDocument.uri)
			if (!doc) return []
			const text = doc.getText()
			const namespaces = this.collectNamespaceEntries(text)
			const subjects = this.indexerSubjects(text)
			return [
				...namespaces.map((ns: any) => ({
					name: `${ns.prefix}: ${ns.iri}`,
					kind: 3 as any,
					location: { uri: doc.uri, range: ns.range },
				})) as any,
				...subjects.map((subj: any) => ({
					name: subj.label,
					kind: 19 as any,
					location: { uri: doc.uri, range: subj.range },
				})) as any,
			]
		})
		this.conn.onDefinition((p) => this.navigation.definition(p))
		this.conn.onTypeDefinition((p) => this.navigation.definition(p))
		this.conn.onImplementation((p) => this.navigation.definition(p))
		this.conn.onReferences((p) => this.navigation.references(p))
		this.conn.onRenameRequest((p) => this.navigation.rename(p))
		this.conn.onDidCloseTextDocument((params) => {
			this.indexer.removeFromIndexes(params.textDocument.uri)
		})

		this.indexer.indexWorkspace()

		return {
			capabilities: {
				textDocumentSync: TextDocumentSyncKind.Full,
				foldingRangeProvider: true,
				hoverProvider: true,
				completionProvider: { triggerCharacters: [":", "@"] },
				documentSymbolProvider: true,
				definitionProvider: true,
				referencesProvider: true,
				renameProvider: true,
				typeDefinitionProvider: true,
				implementationProvider: true,
			},
		}
	}

	onContentChange(
		{ document }: TextDocumentChangeEvent<TextDocument>,
		_parseResults: ReturnType<AbstractLanguageServer<TurtleParser>["parseDocument"]>
	) {
		const uri = document.uri
		const content = document.getText()

		let diagnostics: any[] = []
		try {
			const lintDiagnostics = this.basicDiagnostics(document, content)

			let lexDiagnostics: any[] = []
			let parseDiagnostics: any[] = []
			try {
				const parser = new TurtleParser({ errorMessageProvider })
				const parsed = parser.parse(content, this.mode)
				const tokens = parser.input || []
				lexDiagnostics = this.getLexDiagnostics(document, tokens)
				parseDiagnostics = this.getParseDiagnostics(document, parsed.errors || [])
			} catch (err) {
				parseDiagnostics = [
					{
						severity: 1,
						range: {
							start: { line: 0, character: 0 },
							end: { line: 0, character: 1 },
						},
						message: `Parse error: ${
							err instanceof Error ? err.message : typeof err === "string" ? err : "unknown"
						}`,
						source: "turtle-node-lsp",
					},
				]
			}

			diagnostics = [...lexDiagnostics, ...parseDiagnostics, ...lintDiagnostics]
		} catch (err) {
			diagnostics = [
				{
					severity: 1,
					range: {
						start: { line: 0, character: 0 },
						end: { line: 0, character: 1 },
					},
					message: `LSP internal error: ${
						err instanceof Error ? err.message : typeof err === "string" ? err : "unknown"
					}`,
					source: "turtle-node-lsp",
				},
			]
		}

		this.conn.sendDiagnostics({ uri, diagnostics })
		this.indexer.reindexDocument(uri, content)
	}

	parseDocument(document: TextDocument) {
		const content = document.getText()
		const { cst, errors, ...otherParseData } = this.parser.parse(content, this.mode)
		const tokens = this.parser.input
		return {
			cst,
			tokens,
			errors,
			otherParseData,
		}
	}

	private collectNamespaceEntries(text: string) {
		// Reuse indexer utility to parse prefixes quickly
		return this.indexer["collectNamespaceEntries"](text)
	}

	private buildPrefixMap(text: string, prefixes: any[]) {
		const declared = this.indexer["collectNamespaceMap"](text, prefixes)
		const map: Record<string, string> = { ...defaultPrefixes }
		for (const [pref, iri] of Object.entries(declared)) {
			map[normalizePrefix(pref)] = iri
		}
		return map
	}

	private indexerSubjects(text: string) {
		const namespaces = this.collectNamespaceEntries(text)
		return this.indexer["collectSubjectEntries"](text, this.buildPrefixMap(text, namespaces))
	}

	private basicDiagnostics(document: TextDocument, text: string) {
		const diagnostics: any[] = []
		const namespaceEntries = this.collectNamespaceEntries(text)
		const prefixMap = this.buildPrefixMap(text, namespaceEntries)
		const lines = text.split(/\r?\n/)
		const prefixUsage: Record<string, number> = {}
		const seenPrefixDecl: Record<string, number> = {}
		let bracketDepth = 0
		let parenDepth = 0

		lines.forEach((line, idx) => {
			const trimmed = line.trim()
			if (!trimmed || trimmed.startsWith("@") || trimmed.startsWith("#")) return

			const depthBefore = bracketDepth + parenDepth

			// Terminator check
			const openBrackets = countChars(trimmed, "[")
			const closeBrackets = countChars(trimmed, "]")
			const openParens = countChars(trimmed, "(")
			const closeParens = countChars(trimmed, ")")
			const depthAfter = Math.max(0, depthBefore + openBrackets - closeBrackets + openParens - closeParens)

			if (depthBefore === 0 && depthAfter === 0 && !/[.;,\]\)]\s*$/.test(trimmed)) {
				diagnostics.push({
					severity: 2,
					range: {
						start: { line: idx, character: 0 },
						end: { line: idx, character: line.length },
					},
					message: "Turtle statements typically end with '.', ';', or ','",
					source: "turtle-node-lsp",
				})
			}

			// Undefined prefixes (ignore inside quoted strings)
			const sanitized = trimmed.replace(/"([^"\\]|\\.)*"/g, "")
			const uses = [...sanitized.matchAll(/([A-Za-z]*):[\w-]+/g)]
			for (const match of uses) {
				const pref = match[1]
				const normalized = normalizePrefix(pref)
				prefixUsage[normalized] = (prefixUsage[normalized] || 0) + 1
				if (!prefixMap[normalized]) {
					const start = line.indexOf(match[0])
					diagnostics.push({
						severity: 2,
						range: {
							start: { line: idx, character: start },
							end: { line: idx, character: start + match[0].length },
						},
						message: `Prefix '${displayPrefix(pref)}' is not declared`,
						source: "turtle-node-lsp",
					})
				}
			}

			// Update nesting depth for [] and ()
			bracketDepth = Math.max(0, bracketDepth + openBrackets - closeBrackets)
			parenDepth = Math.max(0, parenDepth + openParens - closeParens)
		})

		// Duplicate prefix declarations
		for (const entry of namespaceEntries) {
			const normalized = normalizePrefix(entry.prefix)
			seenPrefixDecl[normalized] = (seenPrefixDecl[normalized] || 0) + 1
			if (seenPrefixDecl[normalized] > 1) {
				diagnostics.push({
					severity: 2,
					range: entry.range,
					message: `Prefix '${displayPrefix(entry.prefix)}' is declared multiple times`,
					source: "turtle-node-lsp",
				})
			}
		}

		// Unused prefixes (informational)
		for (const entry of namespaceEntries) {
			const normalized = normalizePrefix(entry.prefix)
			if ((prefixUsage[normalized] || 0) === 0) {
				diagnostics.push({
					severity: 3,
					range: entry.range,
					message: `Prefix '${displayPrefix(entry.prefix)}' is never used`,
					source: "turtle-node-lsp",
				})
			}
		}

		return diagnostics
	}
}

function countChars(text: string, char: string) {
	return (text.match(new RegExp(`\\${char}`, "g")) || []).length
}

function normalizePrefix(pref?: string) {
	return pref && pref.length > 0 ? pref : ":"
}

function displayPrefix(pref?: string) {
	return pref && pref.length > 0 ? pref : ":"
}

const connection = createConnection(ProposedFeatures.all, process.stdin, process.stdout)
const workspaceRoot = process.cwd()

const server = new TurtleLanguageServer(connection, workspaceRoot)

server.start()
