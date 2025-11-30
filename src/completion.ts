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
    // We still preload the basics because they are often used without explicit prefixes
    // or implied in many environments.
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
   * PUBLIC API: Call this method from your `documents.onDidOpen` or `documents.onDidChangeContent` handler.
   * It scans the document for prefixes (e.g. "@prefix foaf: ...") and loads the corresponding vocabulary
   * data into memory so it's ready when the user types.
   */
  public preloadFromDocument(doc: TextDocument) {
    const text = doc.getText();
    const foundPrefixes = this.scanForPrefixes(text);

    for (const prefix of foundPrefixes) {
      // Only load if we haven't cached it already
      if (!this.cachedVocabs.has(prefix)) {
        console.log(`Auto-detected prefix '${prefix}' in file. Pre-loading vocabulary...`);
        this.loadVocabularyIntoCache(prefix);
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
        // FIXED: Cast CompletionItemKind to any to allow numeric indexing for the label
        detail: `${prefix} ${(CompletionItemKind as any)[data.kind]}`,
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
    // UPDATED: Use fuzzy logic by returning ALL vocab items if strict prefix isn't found
    const vocabItems = vocabItemsRaw.map(normalizeVocab)
      .concat(this.vocabFromPrefix(currentPrefix, replaceRange))

    const allItems: CompletionItem[] = [...vocabItems, ...subjectItems, ...prefixItems, ...keywordItems]

    // 6. Professional Sorting Logic
    // We want exact matches to appear first, then prefix matches, then others.
    const queryLower = current.toLowerCase();

    const withSort = allItems.map((item) => {
      const label = item.label || ""
      const labelLower = label.toLowerCase();
      const itemPrefix = extractPrefix(label);

      // BUCKET STRATEGY
      // 000: User has typed a prefix (e.g. "owl") and item matches that prefix exactly (e.g. "owl:")
      // 010: Item is inside the currently active prefix (e.g. "owl:Class" when "owl" is active)
      // 020: Item starts with exactly what user typed (e.g. "owl" -> "owl:Class")
      // 050: References (Local subjects)
      // 100: Keywords
      // 900: Fallback

      let bucket = "900";

      if (currentPrefix && itemPrefix === currentPrefix) {
        // We are strictly inside a namespace (e.g. "owl:...")
        bucket = "010";

        // Boost strict matches within the namespace (e.g. "owl:Cl" -> "owl:Class")
        if (labelLower.startsWith(queryLower)) {
          bucket = "005";
        }
      }
      else if (!currentPrefix && labelLower.startsWith(queryLower)) {
        // No specific prefix active, but label starts with query
        // e.g. typed "ow" -> match "owl:"
        bucket = "020";
      }
      else if (item.kind === CompletionItemKind.Reference) {
        bucket = "050";
      }
      else if (item.kind === CompletionItemKind.Keyword) {
        bucket = "100";
      }

      // Final sort text: Bucket + Label
      // This ensures "010_owl:Class" comes before "010_owl:Thing" (alphabetical within bucket)
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

    // Standard string sort on the generated sortText
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
   * * LOGIC UPDATE:
   * 1. If 'currentPrefix' is provided (e.g. "owl"), we strictly return "owl:..." items.
   * 2. If 'currentPrefix' is EMPTY (user just typing "Class"), we return ALL cached vocab items.
   * This allows VS Code's fuzzy matcher to find "owl:Class" when you type "Class".
   */
  private vocabFromPrefix(currentPrefix: string, replaceRange: any): CompletionItem[] {
    let items: CompletionItem[] = [];

    if (currentPrefix) {
      // Strict mode: User has typed "owl:", so we only show "owl" terms
      if (!this.cachedVocabs.has(currentPrefix) && (currentPrefix in vocabularies)) {
        this.loadVocabularyIntoCache(currentPrefix);
      }
      items = this.cachedVocabs.get(currentPrefix) || [];
    } else {
      // Fuzzy mode: User hasn't chosen a prefix yet. Show EVERYTHING.
      // This ensures typing "Action" suggests "schema:Action", "owl:Action", etc.
      for (const vocabItems of this.cachedVocabs.values()) {
        items = items.concat(vocabItems);
      }
    }

    // Map to apply the textEdit (replacement range) dynamically
    return items.map(item => ({
      ...item,
      textEdit: replaceRange ? { newText: item.label, range: replaceRange } : undefined,
      insertText: replaceRange ? undefined : item.label,
    }));
  }
}
