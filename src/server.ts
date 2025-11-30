#!/usr/bin/env node

import {
  createConnection,
  ProposedFeatures,
  TextDocuments,
  TextDocumentSyncKind,
  FoldingRangeParams,
  TextDocumentChangeEvent,
  Diagnostic,
  DiagnosticSeverity,
  DocumentOnTypeFormattingParams,
  Hover,
  TextEdit,
} from "vscode-languageserver/node"
import { TextDocument } from "vscode-languageserver-textdocument"
import { AbstractLanguageServer, errorMessageProvider } from "stardog-language-utils"
import { TurtleParser, ModeString } from "millan"
import { CompletionEngine } from "./completion"
import { Indexer } from "./indexer"
import { NavigationEngine } from "./navigation"
import { currentWord } from "./util"
// UPDATED: Import Zazuko for vocabulary validation
import { vocabularies, prefixes as zazukoPrefixes } from '@zazuko/rdf-vocabularies'

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

  // CACHE: Stores Set of valid terms for O(1) lookup (e.g. 'foaf' -> Set('Person', 'knows'...))
  private validTermsCache: Map<string, Set<string>> = new Map();
  private loadingVocabs: Set<string> = new Set();

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
    this.conn.onHover(async (params) => {
      const doc = this.documents.get(params.textDocument.uri)
      if (!doc) return null
      const word = currentWord(doc, params.position)
      if (!word) return null

      // Check for prefix:term pattern
      const match = word.match(/^([A-Za-z0-9_\-]+):([A-Za-z0-9_\-]+)$/)
      if (match) {
        const prefix = match[1]
        const suffix = match[2]
        const documentation = await this.completionEngine.getTermDocumentation(prefix, suffix)
        if (documentation) {
          return { contents: documentation }
        }
      }
      return null
    })
    this.conn.onDocumentOnTypeFormatting((params: DocumentOnTypeFormattingParams) => {
      const doc = this.documents.get(params.textDocument.uri)
      if (!doc) return []
      
      const { position } = params
      if (position.line === 0) return []

      // Get previous line content
      const prevLineText = doc.getText({
        start: { line: position.line - 1, character: 0 },
        end: { line: position.line - 1, character: Number.MAX_VALUE }
      })

      // Logic: 
      // 1. Remove comments from prev line to check terminator
      // 2. If ends with '.', new indent is 0.
      // 3. Else, copy indent from prev line.
      const cleanPrevLine = prevLineText.replace(/#.*$/, "").trimEnd()
      
      let newIndent = ""
      if (!cleanPrevLine.endsWith(".")) {
        const match = prevLineText.match(/^(\s*)/)
        if (match) newIndent = match[1]
      }

      return [
        TextEdit.replace(
          {
            start: { line: position.line, character: 0 },
            end: { line: position.line, character: params.options.insertSpaces ? params.options.tabSize : 999 } 
            // Note: Simple replacement of leading space. To be safe, we replace current indentation.
            // But on a fresh newline, indentation is usually empty or auto-inserted by editor. 
            // We replace a small range to enforce our indent.
          },
          newIndent
        )
      ]
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
        documentOnTypeFormattingProvider: { firstTriggerCharacter: "\n" },
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

    let diagnostics: Diagnostic[] = []
    try {
      const lintDiagnostics = this.basicDiagnostics(document, content)

      let lexDiagnostics: Diagnostic[] = []
      let parseDiagnostics: Diagnostic[] = []
      try {
        const parser = new TurtleParser({ errorMessageProvider })
        const parsed = parser.parse(content, this.mode)
        const tokens = parser.input || []
        lexDiagnostics = this.getLexDiagnostics(document, tokens)
        parseDiagnostics = this.getParseDiagnostics(document, parsed.errors || [])
      } catch (err) {
        parseDiagnostics = [
          {
            severity: DiagnosticSeverity.Error,
            range: {
              start: { line: 0, character: 0 },
              end: { line: 0, character: 1 },
            },
            message: `Parse error: ${err instanceof Error ? err.message : typeof err === "string" ? err : "unknown"
              }`,
            source: "turtle-node-lsp",
          },
        ]
      }

      diagnostics = [...lexDiagnostics, ...parseDiagnostics, ...lintDiagnostics]
    } catch (err) {
      diagnostics = [
        {
          severity: DiagnosticSeverity.Error,
          range: {
            start: { line: 0, character: 0 },
            end: { line: 0, character: 1 },
          },
          message: `LSP internal error: ${err instanceof Error ? err.message : typeof err === "string" ? err : "unknown"
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
    // @ts-ignore - Indexer private method access workaround
    return this.indexer["collectNamespaceEntries"](text)
  }

  private buildPrefixMap(text: string, prefixes: any[]) {
    // @ts-ignore - Indexer private method access workaround
    const declared = this.indexer["collectNamespaceMap"](text, prefixes)
    const map: Record<string, string> = { ...defaultPrefixes }
    for (const [pref, iri] of Object.entries(declared)) {
      map[normalizePrefix(pref)] = iri as string
    }
    return map
  }

  private indexerSubjects(text: string) {
    const namespaces = this.collectNamespaceEntries(text)
    // @ts-ignore - Indexer private method access workaround
    return this.indexer["collectSubjectEntries"](text, this.buildPrefixMap(text, namespaces))
  }

  /**
   * Helper to lazily load and cache vocabulary terms
   */
  private async ensureVocabLoaded(prefix: string) {
    if (this.validTermsCache.has(prefix) || this.loadingVocabs.has(prefix)) return;

    // Only load if it's a known Zazuko prefix
    if (!((zazukoPrefixes as any)[prefix])) return;

    this.loadingVocabs.add(prefix);
    try {
      const datasets = await vocabularies({ only: [prefix] });
      const dataset = datasets[prefix];
      const namespaceUri = (zazukoPrefixes as any)[prefix];

      if (dataset && namespaceUri) {
        const terms = new Set<string>();
        for (const quad of dataset) {
          if (quad.subject.value.startsWith(namespaceUri)) {
            const localName = quad.subject.value.substring(namespaceUri.length);
            if (localName) terms.add(localName);
          }
        }
        this.validTermsCache.set(prefix, terms);
      }
    } catch (e) {
      console.error(`Failed to load validation vocab for ${prefix}`, e);
    } finally {
      this.loadingVocabs.delete(prefix);
    }
  }

  private basicDiagnostics(document: TextDocument, text: string) {
    const diagnostics: Diagnostic[] = []
    const namespaceEntries = this.collectNamespaceEntries(text)
    const prefixMap = this.buildPrefixMap(text, namespaceEntries)
    
    // Build a set of explicitly declared prefixes for strict validation
    const declaredPrefixes = new Set<string>()
    for (const entry of namespaceEntries) {
      declaredPrefixes.add(normalizePrefix(entry.prefix))
    }

    const lines = text.split(/\r?\n/)
    const prefixUsage: Record<string, number> = {}
    const seenPrefixDecl: Record<string, number> = {}
    let bracketDepth = 0
    let parenDepth = 0

    lines.forEach((line, idx) => {
      const trimmed = line.trim()
      if (!trimmed || trimmed.startsWith("@") || trimmed.startsWith("#")) return

      // Sanitize strings to avoid checking terms inside quotes and comments
      // Supports "...", '...', """...""", '''...'''
      // Also remove IRIs <...> and comments #... to avoid false positives
      const sanitized = trimmed
        .replace(/("""(?:[^"\\]|\\.)*?"""|'''(?:[^'\\]|\\.)*?'''|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, "") // Strings
        .replace(/<[^>]*>/g, "") // IRIs
        .replace(/#.*$/, "") // Comments

      const depthBefore = bracketDepth + parenDepth

      // Terminator check
      // Use sanitized string to ignore brackets/parens inside strings and comments
      const openBrackets = countChars(sanitized, "[")
      const closeBrackets = countChars(sanitized, "]")
      const openParens = countChars(sanitized, "(")
      const closeParens = countChars(sanitized, ")")
      const depthAfter = Math.max(0, depthBefore + openBrackets - closeBrackets + openParens - closeParens)

      // Warn only if we are at top level, line has content (predicate/object implied by whitespace), and no terminator
      // We check /\s/ to skip single-token lines (likely just a Subject, e.g. "core:Metadata")
      if (depthBefore === 0 && depthAfter === 0 && !/[.;,\]\)]\s*$/.test(sanitized) && /\s/.test(sanitized)) {
        diagnostics.push({
          severity: DiagnosticSeverity.Warning,
          range: {
            start: { line: idx, character: 0 },
            end: { line: idx, character: line.length },
          },
          message: "Turtle statements typically end with '.', ';', or ','",
          source: "turtle-node-lsp",
        })
      }

      // Regex to find 'prefix:suffix' patterns
      // Captures: prefix (group 1), suffix (group 2)
      // Suffix can be empty (e.g. ":" or "ex:")
      const uses = [...sanitized.matchAll(/([A-Za-z0-9_\-]*):([A-Za-z0-9_\-]*)/g)]

      for (const match of uses) {
        const pref = match[1]
        const suffix = match[2]
        const normalized = normalizePrefix(pref)
        const start = line.indexOf(match[0])
        const range = {
          start: { line: idx, character: start },
          end: { line: idx, character: start + match[0].length },
        }

        prefixUsage[normalized] = (prefixUsage[normalized] || 0) + 1

        // 1. Check if Prefix is Declared (Strict check)
        if (!declaredPrefixes.has(normalized)) {
          diagnostics.push({
            severity: DiagnosticSeverity.Warning,
            range,
            message: `Prefix '${displayPrefix(pref)}' is not declared`,
            source: "turtle-node-lsp",
          })
        }

        // 2. Check if Suffix is Valid (if the prefix resolves to a namespace we know)
        if (prefixMap[normalized]) {
          // We verify if this prefix maps to a standard URI that Zazuko knows
          const iri = prefixMap[normalized];
          // Simple check: does the prefix key exist in Zazuko? 
          // (Ideally we check IRI, but prefix keys are safer for the cache key)
          if ((zazukoPrefixes as any)[pref]) {
            if (this.validTermsCache.has(pref)) {
              const validTerms = this.validTermsCache.get(pref)!;
              if (!validTerms.has(suffix)) {
                // Edge case: Sometimes ontologies use terms not explicitly defined in the standard bundle.
                // We use 'Warning' instead of 'Error' to be safe.
                diagnostics.push({
                  severity: DiagnosticSeverity.Warning,
                  range,
                  message: `Term '${suffix}' is not defined in the '${pref}' vocabulary.`,
                  source: "turtle-node-lsp",
                })
              }
            } else {
              // Fire and forget load for next time
              this.ensureVocabLoaded(pref);
            }
          }
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
          severity: DiagnosticSeverity.Warning,
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
          severity: DiagnosticSeverity.Information,
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
