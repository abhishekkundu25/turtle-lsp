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

// UPDATED: Import the main factory function and prefixes
import { vocabularies, prefixes } from '@zazuko/rdf-vocabularies'

// Common vocabularies we want to preload for instant access
const COMMON_VOCABS = ['rdf', 'rdfs', 'owl', 'xsd', 'skos', 'sh', 'dcterms', 'foaf', 'schema'];

export class CompletionEngine {
  // Cache the processed completion items.
  private cachedVocabs: Map<string, CompletionItem[]> = new Map();

  // Track ongoing fetch requests to prevent duplicate loading
  private loadingPromises: Map<string, Promise<void>> = new Map();

  constructor(private indexer: Indexer) {
    // Fire-and-forget preload
    this.preloadVocabularies();
  }

  /**
   * Pre-computes completion items for common vocabularies.
   * This ensures the first autocomplete trigger is snappy.
   */
  private async preloadVocabularies() {
    console.log('Pre-loading common RDF vocabularies for autocomplete...');
    // Load in parallel
    await Promise.all(COMMON_VOCABS.map(prefix => this.loadVocabularyIntoCache(prefix)));
  }

  /**
   * PUBLIC API: Call this method from your `documents.onDidOpen` or `documents.onDidChangeContent` handler.
   * It scans the document for prefixes (e.g. "@prefix foaf: ...") and loads the corresponding vocabulary
   * data into memory so it's ready when the user types.
   */
  public async preloadFromDocument(doc: TextDocument) {
    const text = doc.getText();
    const foundPrefixes = this.scanForPrefixes(text);

    for (const prefix of foundPrefixes) {
      if (!this.cachedVocabs.has(prefix) && !this.loadingPromises.has(prefix)) {
        console.log(`Auto-detected prefix '${prefix}' in file. Pre-loading vocabulary...`);
        // We don't await this loop to avoid blocking the main thread; 
        // let them load in the background
        this.loadVocabularyIntoCache(prefix).catch(err => console.error(`Failed to load ${prefix}`, err));
      }
    }
  }

  /**
   * Simple regex scanner to find prefixes used in the document.
   * Supports both Turtle ("@prefix") and SPARQL ("PREFIX") styles.
   */
  private scanForPrefixes(text: string): string[] {
    const found = new Set<string>();
    // Regex explanation:
    // (?:@prefix|PREFIX) -> match either style, non-capturing group
    // \s+                -> whitespace
    // ([\w-]+)           -> CAPTURE GROUP 1: The prefix key (e.g., "foaf")
    // :                  -> the colon separator
    const regex = /(?:@prefix|PREFIX)\s+([\w-]+):/gi;

    let match;
    while ((match = regex.exec(text)) !== null) {
      found.add(match[1]);
    }
    return Array.from(found);
  }

  /**
   * Loads a vocabulary from Zazuko using the async factory pattern.
   * This aligns with the documentation: vocabularies({ only: [...] })
   */
  private async loadVocabularyIntoCache(prefix: string): Promise<void> {
    if (this.cachedVocabs.has(prefix)) return;
    if (this.loadingPromises.has(prefix)) return this.loadingPromises.get(prefix);

    const loadPromise = (async () => {
      // Check if Zazuko supports this prefix before trying to fetch
      // Note: 'prefixes' object gives us quick synchronous check
      if (!((prefixes as any)[prefix])) {
        return;
      }

      try {
        // DOCS: "Loading only some Vocabularies as Datasets"
        // This returns a Promise<Record<string, Dataset>>
        const datasets = await vocabularies({ only: [prefix] });
        const dataset = datasets[prefix];
        const namespaceUri = (prefixes as any)[prefix];

        if (!dataset || !namespaceUri) return;

        const items: CompletionItem[] = [];
        const termData = new Map<string, { kind: CompletionItemKind, docs: string[], label: string }>();

        for (const quad of dataset) {
          const subject = quad.subject.value;

          // Only process terms strictly within this namespace
          if (!subject.startsWith(namespaceUri)) continue;

          // Extract local name (e.g., "Person" from "http://xmlns.com/foaf/0.1/Person")
          const localName = subject.substring(namespaceUri.length);
          if (!localName) continue;

          if (!termData.has(localName)) {
            termData.set(localName, {
              kind: CompletionItemKind.Property,
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
            detail: `${prefix} ${(CompletionItemKind as any)[data.kind]}`,
            documentation: {
              kind: MarkupKind.Markdown,
              value: data.docs.join('\n\n') || `Term from ${prefix} vocabulary.`
            },
            data: { localName, prefix }
          });
        }

        this.cachedVocabs.set(prefix, items);
      } catch (err) {
        console.error(`Error loading vocabulary ${prefix}:`, err);
      }
    })();

    this.loadingPromises.set(prefix, loadPromise);

    // Cleanup promise from map when done (success or fail)
    loadPromise.finally(() => this.loadingPromises.delete(prefix));

    return loadPromise;
  }

  // UPDATED: Build is now async to allow waiting for lazy-loaded vocabs
  async build(params: TextDocumentPositionParams, doc: TextDocument, namespaces: any[], namespaceMap: Record<string, string>): Promise<CompletionItem[]> {
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
      }
    })

    // 2. Syntax Keywords
    const keywordItems = [
      { label: "@prefix", kind: CompletionItemKind.Keyword, detail: "Declare namespace prefix", insertText: "@prefix " },
      { label: "@base", kind: CompletionItemKind.Keyword, detail: "Set base IRI", insertText: "@base " },
      { label: "a", kind: CompletionItemKind.Keyword, detail: "rdf:type shortcut", insertText: "a " },
      { label: "BASE", kind: CompletionItemKind.Keyword, insertText: "BASE " },
      { label: "PREFIX", kind: CompletionItemKind.Keyword, insertText: "PREFIX " },
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
      const textEdit = replaceRange ? { newText: label, range: replaceRange } : undefined
      const insertText = replaceRange ? undefined : item.insertText ?? label
      return {
        ...item,
        label,
        textEdit,
        insertText,
      }
    }

    // 5. Merge all items
    const loadedVocabItems = await this.vocabFromPrefix(currentPrefix, replaceRange);

    const vocabItems = vocabItemsRaw.map(normalizeVocab)
      .concat(loadedVocabItems)

    const allItems: CompletionItem[] = [...vocabItems, ...subjectItems, ...prefixItems, ...keywordItems]

    // 6. Professional Sorting Logic
    const queryLower = current.toLowerCase();

    const withSort = allItems.map((item) => {
      const label = item.label || ""
      const labelLower = label.toLowerCase();
      const itemPrefix = extractPrefix(label);

      // BUCKET STRATEGY
      let bucket = "900";

      if (currentPrefix && itemPrefix === currentPrefix) {
        // We are strictly inside a namespace
        bucket = "010";
        if (labelLower.startsWith(queryLower)) {
          bucket = "005";
        }
      }
      else if (!currentPrefix && labelLower.startsWith(queryLower)) {
        // Fuzzy match on prefix
        bucket = "020";
      }
      else if (item.kind === CompletionItemKind.Reference) {
        bucket = "050";
      }
      else if (item.kind === CompletionItemKind.Keyword) {
        bucket = "100";
      }

      return {
        ...item,
        sortText: `${bucket}_${label}`
      };
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
   * UPDATED: Returns Promise because loading might happen on demand.
   */
  private async vocabFromPrefix(currentPrefix: string, replaceRange: any): Promise<CompletionItem[]> {
    let items: CompletionItem[] = [];

    if (currentPrefix) {
      // Strict mode: User has typed "owl:", so we only show "owl" terms
      // If not in cache, try to load it now
      if (!this.cachedVocabs.has(currentPrefix) && (currentPrefix in vocabularies || (prefixes as any)[currentPrefix])) {
        await this.loadVocabularyIntoCache(currentPrefix);
      }
      items = this.cachedVocabs.get(currentPrefix) || [];
    } else {
      // Fuzzy mode: Return EVERYTHING in cache.
      for (const vocabItems of this.cachedVocabs.values()) {
        items = items.concat(vocabItems);
      }
    }

    return items.map(item => ({
      ...item,
      textEdit: replaceRange ? { newText: item.label, range: replaceRange } : undefined,
      insertText: replaceRange ? undefined : item.label,
    }));
  }
}
