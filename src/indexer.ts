import fs from "fs"
import path from "path"
import { pathToFileURL } from "url"
import { Location, Range, TextDocument } from "vscode-languageserver/node"
import { Parser as N3Parser } from "n3"
import { abbreviateWithPrefixes, offsetToLineInfo, toFsPath } from "./util"

export type DocCache = {
	prefixes: Array<{ prefix: string; range: Range }>
	subjects: Array<{ label: string; range: Range }>
	symbols: Array<{ label: string; range: Range }>
	map: Record<string, string>
}

export class Indexer {
	private workspaceRoot: string | undefined
	private workspaceIndexed = false
	private maxFiles = 200
	private prefixIndex: Record<string, Location[]> = {}
	private subjectIndex: Record<string, Location[]> = {}
	private symbolIndex: Record<string, Location[]> = {}
	private docCache: Record<string, DocCache> = {}

	constructor(root: string | undefined) {
		this.workspaceRoot = root
	}

	public getPrefixes(word: string): Location[] {
		return this.prefixIndex[word] || []
	}

	public getSubjects(word: string): Location[] {
		return this.subjectIndex[word] || []
	}

	public getSymbols(word: string): Location[] {
		return this.symbolIndex[word] || []
	}

	public getPrefixMap(uri: string): Record<string, string> {
		return this.docCache[uri]?.map || {}
	}

	public collectGlobalSubjects(prefixFilter: string): string[] {
		const results: string[] = []
		for (const label of Object.keys(this.subjectIndex)) {
			if (prefixFilter && this.extractPrefix(label) && this.extractPrefix(label) !== prefixFilter) continue
			results.push(label)
		}
		return results
	}

	public reindexDocument(uri: string, text: string) {
		this.removeFromIndexes(uri)

		const prefixes = this.collectNamespaceEntries(text)
		const nsMap = this.collectNamespaceMap(text, prefixes)
		const subjects = this.collectSubjectEntries(text, nsMap)
		const symbols = this.collectSymbolEntries(text)

		this.docCache[uri] = {
			prefixes: prefixes.map((p) => ({ prefix: p.prefix, range: p.range })),
			subjects: subjects.map((s) => ({ label: s.label, range: s.range })),
			symbols: symbols.map((s) => ({ label: s.label, range: s.range })),
			map: nsMap,
		}

		for (const p of prefixes) {
			if (!this.prefixIndex[p.prefix]) this.prefixIndex[p.prefix] = []
			this.prefixIndex[p.prefix].push({ uri, range: p.range })
		}
		for (const s of subjects) {
			if (!this.subjectIndex[s.label]) this.subjectIndex[s.label] = []
			this.subjectIndex[s.label].push({ uri, range: s.range })
		}
		for (const sym of symbols) {
			if (!this.symbolIndex[sym.label]) this.symbolIndex[sym.label] = []
			this.symbolIndex[sym.label].push({ uri, range: sym.range })
		}
	}

	public removeFromIndexes(uri: string) {
		const cached = this.docCache[uri]
		if (cached) {
			for (const p of cached.prefixes) {
				if (this.prefixIndex[p.prefix]) {
					this.prefixIndex[p.prefix] = this.prefixIndex[p.prefix].filter((loc) => loc.uri !== uri)
					if (this.prefixIndex[p.prefix].length === 0) delete this.prefixIndex[p.prefix]
				}
			}
			for (const s of cached.subjects) {
				if (this.subjectIndex[s.label]) {
					this.subjectIndex[s.label] = this.subjectIndex[s.label].filter((loc) => loc.uri !== uri)
					if (this.subjectIndex[s.label].length === 0) delete this.subjectIndex[s.label]
				}
			}
			for (const sym of cached.symbols) {
				if (this.symbolIndex[sym.label]) {
					this.symbolIndex[sym.label] = this.symbolIndex[sym.label].filter((loc) => loc.uri !== uri)
					if (this.symbolIndex[sym.label].length === 0) delete this.symbolIndex[sym.label]
				}
			}
		}
		delete this.docCache[uri]
	}

	public indexWorkspace() {
		if (this.workspaceIndexed || !this.workspaceRoot) return
		this.workspaceIndexed = true
		const files = this.listTurtleFiles(this.workspaceRoot, this.maxFiles)
		for (const file of files) {
			const text = this.readFileSafe(file)
			if (!text) continue
			const uri = pathToFileURL(file).toString()
			this.reindexDocument(uri, text)
		}
	}

	private listTurtleFiles(root: string, maxFiles: number): string[] {
		const results: string[] = []
		const stack: string[] = [root]
		const skip = new Set([".git", "node_modules", ".venv", "dist", "build"])

		while (stack.length && results.length < maxFiles) {
			const dir = stack.pop()
			if (!dir) break
			let entries: fs.Dirent[] = []
			try {
				entries = fs.readdirSync(dir, { withFileTypes: true })
			} catch {
				continue
			}

			for (const entry of entries) {
				if (skip.has(entry.name)) continue
				const full = path.join(dir, entry.name)
				if (entry.isDirectory()) {
					stack.push(full)
				} else if (entry.isFile() && (entry.name.endsWith(".ttl") || entry.name.endsWith(".turtle"))) {
					results.push(full)
					if (results.length >= maxFiles) break
				}
			}
		}

		return results
	}

	private readFileSafe(file: string): string | null {
		try {
			return fs.readFileSync(file, "utf8")
		} catch {
			return null
		}
	}

	private collectNamespaceEntries(text: string): Array<{ prefix: string; iri: string; range: Range }> {
		const detailed: Array<{ prefix: string; iri: string; range: Range }> = []
		try {
			const parser = new N3Parser()
			parser.parse(text)
			const prefixes: Record<string, string> = (parser as any)._prefixes || {}
			for (const [prefix, iri] of Object.entries(prefixes)) {
				const idx = text.indexOf(prefix + ":")
				if (idx < 0) continue
				const lineInfo = offsetToLineInfo(text, idx)
				detailed.push({
					prefix,
					iri,
					range: {
						start: { line: lineInfo.line, character: lineInfo.character },
						end: { line: lineInfo.line, character: lineInfo.character + prefix.length },
					},
				})
			}
		} catch {
			const lines = text.split(/\r?\n/)
			lines.forEach((line, idx) => {
				const match = line.trim().match(/^@?prefix\s+([A-Za-z][\w-]*):\s*<([^>]+)>/i)
				if (match) {
					const [, prefix, iri] = match
					const start = line.indexOf(prefix)
					const end = start + prefix.length
					detailed.push({
						prefix,
						iri,
						range: {
							start: { line: idx, character: start },
							end: { line: idx, character: end },
						},
					})
				}
			})
		}

		return detailed
	}

	private collectNamespaceMap(text: string, prefixes?: Array<{ prefix: string; iri: string }>): Record<string, string> {
		const map: Record<string, string> = {}
		for (const p of prefixes || this.collectNamespaceEntries(text)) {
			map[p.prefix] = p.iri
		}
		return map
	}

	private collectSubjectEntries(text: string, nsMap: Record<string, string>): Array<{ label: string; range: Range }> {
		const detailed: Array<{ label: string; range: Range }> = []
		try {
			const parser = new N3Parser()
			const quads = parser.parse(text)
			const seen = new Set<string>()
			for (const quad of quads) {
				const subj = quad.subject.value
				if (seen.has(subj)) continue
				seen.add(subj)
				let idx = text.indexOf(subj)
				if (idx < 0) {
					for (const [prefix, base] of Object.entries(nsMap)) {
						if (subj.startsWith(base)) {
							const pref = `${prefix}:${subj.slice(base.length)}`
							idx = text.indexOf(pref)
							if (idx >= 0) {
								const lineInfo = offsetToLineInfo(text, idx)
								detailed.push({
									label: pref,
									range: {
										start: { line: lineInfo.line, character: lineInfo.character },
										end: { line: lineInfo.line, character: lineInfo.character + pref.length },
									},
								})
								break
							}
						}
					}
				} else {
					const lineInfo = offsetToLineInfo(text, idx)
					detailed.push({
						label: subj,
						range: {
							start: { line: lineInfo.line, character: lineInfo.character },
							end: { line: lineInfo.line, character: lineInfo.character + subj.length },
						},
					})
				}
			}
			const lines = text.split(/\r?\n/)
			lines.forEach((line, idx) => {
				const trimmed = line.trim()
				if (!trimmed || trimmed.startsWith("@") || trimmed.startsWith("#")) return
				const subjectMatch = trimmed.match(/^([A-Za-z][\w-]*:[\w-]+)/)
				if (subjectMatch) {
					const label = subjectMatch[1]
					if (seen.has(label)) return
					seen.add(label)
					const start = line.indexOf(label)
					detailed.push({
						label,
						range: {
							start: { line: idx, character: start },
							end: { line: idx, character: start + label.length },
						},
					})
				}
			})
		} catch {
			const lines = text.split(/\r?\n/)
			lines.forEach((line, idx) => {
				const trimmed = line.trim()
				if (!trimmed || trimmed.startsWith("@") || trimmed.startsWith("#")) return

				const subjectMatch = trimmed.match(/^([<A-Za-z][^\s>]*)/)
				if (subjectMatch) {
					const label = subjectMatch[1]
					const start = line.indexOf(label)
					detailed.push({
						label,
						range: {
							start: { line: idx, character: start },
							end: { line: idx, character: start + label.length },
						},
					})
				}
			})
		}

		return detailed
	}

	private collectSymbolEntries(text: string): Array<{ label: string; range: Range }> {
		const results: Array<{ label: string; range: Range }> = []
		const lines = text.split(/\r?\n/)
		lines.forEach((line, idx) => {
			const matches = line.matchAll(/([A-Za-z][\w-]*:[\w-]+)/g)
			for (const m of matches) {
				const label = m[1]
				const start = m.index ?? 0
				results.push({
					label,
					range: {
						start: { line: idx, character: start },
						end: { line: idx, character: start + label.length },
					},
				})
			}
		})
		return results
	}

	private extractPrefix(token: string): string {
		const match = token.match(/^([A-Za-z][\w-]*):/)
		return match ? match[1] : ""
	}
}
