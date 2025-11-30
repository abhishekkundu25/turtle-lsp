import {
  CompletionItemKind,
  CompletionItem,
  TextDocument,
  TextDocumentPositionParams,
  MarkupKind
} from "vscode-languageserver/node"
import { getCommonCompletionItemsGivenNamespaces } from "stardog-language-utils"
import { Indexer } from "./indexer"
import { abbreviateWithPrefixes, currentWord, currentWordRange, detectPrefixAtPosition, extractPrefix } from "./util"

// UPDATED: Use named exports from the root package to avoid ENOENT errors
import { vocabularies, prefixes } from '@zazuko/rdf-vocabularies'

// Common vocabularies we want to preload for instant access
const COMMON_VOCABS = ['rdf', 'rdfs', 'owl', 'xsd', 'skos', 'sh', 'dcterms', 'foaf', 'schema'];

export class CompletionEngine {
  // PRO TIP: Cache the processed completion items. 
  // Querying the dataset on every keystroke is O(n) and slow. 
  // A Map lookup is O(1).
  private cachedVocabs: Map<string, CompletionItem[]> = new Map();

  constructor(private indexer: Indexer) {
    this.preloadVocabularies();
  }

  /**
   * Pre-computes completion items for common vocabularies.
   * This ensures the first autocomplete trigger is snappy.
   */
  private preloadVocabularies() {
    console.log('Pre-loading common RDF vocabularies for autocomplete...');
    for (const prefix of COMMON_VOCABS) {
      this.loadVocabularyIntoCache(prefix);
    }
  }

  /**
   * Loads a vocabulary from Zazuko, processes the triples, and caches the result.
   */
  private loadVocabularyIntoCache(prefix: string) {
    if (this.cachedVocabs.has(prefix)) return;

    // Check if Zazuko has this vocabulary
    if (!(prefix in vocabularies)) {
      return;
    }

    // Typescript casting for the dynamic access
    const dataset = (vocabularies as any)[prefix];

    // UPDATED: Access the prefixes object directly
    const namespaceUri = (prefixes as any)[prefix];

    if (!dataset || !namespaceUri) return;

    const items: CompletionItem[] = [];

    // We use a temporary map to merge properties (labels, comments, types) 
    // for the same term before creating the final CompletionItem
    const termData = new Map<string, { kind: CompletionItemKind, docs: string[], label: string }>();

    for (const quad of dataset) {
      const subject = quad.subject.value;

      // Only process terms strictly within this namespace
      if (!subject.startsWith(namespaceUri)) continue;

      // Extract local name (e.g., "Person" from "http://xmlns.com/foaf/0.1/Person")
      const localName = subject.substring(namespaceUri.length);
      if (!localName) continue; // Skip the ontology definition itself if it matches the base URI

      if (!termData.has(localName)) {
        termData.set(localName, {
          kind: CompletionItemKind.Property, // Default to Property
          docs: [],
          label: localName
        });
      }

      const data = termData.get(localName)!;
      const pred = quad.predicate.value;
      const obj = quad.object.value;

      // Heuristics to determine Icon/Kind
      if (pred === 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type') {
        if (obj.includes('Class') || obj.includes('Shape')) {
          data.kind = CompletionItemKind.Class;
        } else if (obj.includes('Property')) {
          data.kind = CompletionItemKind.Property;
        }
      }

      // Capture documentation
      if (pred === 'http://www.w3.org/2000/01/rdf-schema#comment' ||
        pred === 'http://www.w3.org/2004/02/skos/core#definition' ||
        pred === 'http://purl.org/dc/terms/description') {
        if (quad.object.value) {
          data.docs.push(quad.object.value);
        }
      }
    }

    // Convert processed data to LSP CompletionItems
    for (const [localName, data] of termData) {
      items.push({
        label: `${prefix}:${localName}`,
        kind: data.kind,
        detail: `${prefix} ${CompletionItemKind[data.kind]}`,
        documentation: {
          kind: MarkupKind.Markdown,
          value: data.docs.join('\n\n') || `Term from ${prefix} vocabulary.`
        },
        // We set insertText in the build method based on range, 
        // but we store the clean name here for matching
        data: { localName, prefix }
      });
    }

    this.cachedVocabs.set(prefix, items);
  }

  build(params: TextDocumentPositionParams, doc: TextDocument, namespaces: any[], namespaceMap: Record<string, string>) {
    const text = doc.getText()
    const subjectSet = this.collectSubjectSet(text)
    const current = currentWord(doc, params.position) || ""
    const currentPrefix = detectPrefixAtPosition(doc, params.position, current)
    const replaceRange = currentWordRange(doc, params.position)
    const globalSubjects = this.indexer.collectGlobalSubjects(currentPrefix || "")

    // 1. Prefix Keywords (e.g., "rdf:", "foaf:")
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

    // 2. Syntax Keywords
    const keywordItems = [
      { label: "@prefix", kind: CompletionItemKind.Keyword, detail: "Declare namespace prefix", insertText: "@prefix ", sortText: "500_prefix" },
      { label: "@base", kind: CompletionItemKind.Keyword, detail: "Set base IRI", insertText: "@base ", sortText: "500_base" },
      { label: "a", kind: CompletionItemKind.Keyword, detail: "rdf:type shortcut", insertText: "a ", sortText: "500_a" },
      { label: "BASE", kind: CompletionItemKind.Keyword, sortText: "500_BASE" },
      { label: "PREFIX", kind: CompletionItemKind.Keyword, sortText: "500_PREFIX" },
    ]

    // 3. Local Subjects
    const subjectItems = Array.from(new Set([...subjectSet, ...globalSubjects])).map((subject) => {
      const display = abbreviateWithPrefixes(subject, namespaceMap)
      return {
        label: display,
        kind: CompletionItemKind.Reference,
        textEdit: replaceRange ? { newText: display, range: replaceRange } : undefined,
        insertText: replaceRange ? undefined : display,
      }
    })

    // 4. Stardog/Common Utils items
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

    // 5. Merge all items
    // Note: We use vocabFromPrefix (the cached Zazuko data) here
    const vocabItems = vocabItemsRaw.map(normalizeVocab)
      .concat(this.vocabFromPrefix(currentPrefix, replaceRange))

    const allItems: CompletionItem[] = [...vocabItems, ...subjectItems, ...prefixItems, ...keywordItems]

    // 6. Sorting Logic
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

  /**
   * Retrieve items from the cache for the current prefix.
   * If it's a known Zazuko prefix but not cached yet, we cache it now.
   */
  private vocabFromPrefix(currentPrefix: string, replaceRange: any): CompletionItem[] {
    if (!currentPrefix) return [];

    // If we haven't cached this valid prefix yet, try to load it on the fly
    if (!this.cachedVocabs.has(currentPrefix) && (currentPrefix in vocabularies)) {
      this.loadVocabularyIntoCache(currentPrefix);
    }

    const items = this.cachedVocabs.get(currentPrefix) || [];

    // We must map the cached items to new objects to apply the specific 
    // replaceRange (TextEdit) for the current cursor position
    return items.map(item => ({
      ...item,
      textEdit: replaceRange ? { newText: item.label, range: replaceRange } : undefined,
      insertText: replaceRange ? undefined : item.label,
    }));
  }
}
