import { DefinitionParams, Location, ReferenceParams, RenameParams, WorkspaceEdit, Range } from "vscode-languageserver/node"
import { Indexer } from "./indexer"
import { currentWord, rangeMatchesWord } from "./util"
import { TextDocuments } from "vscode-languageserver"
import { TextDocument } from "vscode-languageserver-textdocument"

export class NavigationEngine {
	constructor(private indexer: Indexer, private documents: any) {}

	definition(params: DefinitionParams): Location[] {
		const doc = this.documents.get(params.textDocument.uri)
		if (!doc) return []
		const word = currentWord(doc, params.position)
		if (!word) return []

		const results: Location[] = []
		results.push(...this.indexer.getPrefixes(word))
		if (word.endsWith(":")) {
			const trimmed = word.replace(/:$/, "")
			results.push(...this.indexer.getPrefixes(trimmed))
		}
		results.push(...this.indexer.getSubjects(word))
		results.push(...this.indexer.getSymbols(word))
		return results
	}

	references(params: ReferenceParams): Location[] {
		const doc = this.documents.get(params.textDocument.uri)
		if (!doc) return []
		const word = currentWord(doc, params.position)
		if (!word) return []

		const locs: Location[] = []
		locs.push(...this.indexer.getSubjects(word))
		locs.push(...this.indexer.getPrefixes(word))
		if (word.endsWith(":")) {
			const trimmed = word.replace(/:$/, "")
			locs.push(...this.indexer.getPrefixes(trimmed))
		}
		locs.push(...this.indexer.getSymbols(word))
		return locs
	}

	rename(params: RenameParams): WorkspaceEdit | null {
		const doc = this.documents.get(params.textDocument.uri)
		if (!doc) return null
		const word = currentWord(doc, params.position)
		if (!word) return null

		const changes: Record<string, { range: Range; newText: string }[]> = {}
		const addEdits = (list?: Location[]) => {
			if (!list) return
			for (const loc of list) {
				const openDoc = this.documents.get(loc.uri)
				if (!rangeMatchesWord(loc.uri, loc.range, word, openDoc ?? null)) continue
				if (!changes[loc.uri]) changes[loc.uri] = []
				changes[loc.uri].push({ range: loc.range, newText: params.newName })
			}
		}

		addEdits(this.indexer.getSubjects(word))
		addEdits(this.indexer.getPrefixes(word))
		if (word.endsWith(":")) {
			const trimmed = word.replace(/:$/, "")
			addEdits(this.indexer.getPrefixes(trimmed))
		}
		addEdits(this.indexer.getSymbols(word))

		if (Object.keys(changes).length === 0) return null
		return { changes }
	}
}
