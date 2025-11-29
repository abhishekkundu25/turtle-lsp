import path from "path"
import fs from "fs"
import { Range, TextDocument } from "vscode-languageserver/node"

export function toFsPath(uri: string): string {
	if (uri.startsWith("file://")) return path.normalize(new URL(uri).pathname)
	return path.normalize(uri)
}

export function offsetToLineInfo(text: string, offset: number) {
	if (offset < 0) return { line: 0, character: 0 }
	const lines = text.slice(0, offset).split(/\r?\n/)
	const line = lines.length - 1
	const character = lines[lines.length - 1]?.length ?? 0
	return { line, character }
}

export function detectPrefixAtPosition(
	doc: TextDocument,
	position: { line: number; character: number },
	currentToken: string
): string {
	const direct = extractPrefix(currentToken)
	if (direct) return direct
	const lineText = doc.getText({ start: { line: position.line, character: 0 }, end: position })
	const match = lineText.match(/([A-Za-z][\w-]*):[\w-]*$/)
	return match ? match[1] : ""
}

export function extractPrefix(token: string): string {
	const match = token.match(/^([A-Za-z][\w-]*):/)
	return match ? match[1] : ""
}

export function currentWordRange(doc: TextDocument, position: { line: number; character: number }) {
	const text = doc.getText()
	const offset = doc.offsetAt(position)
	const safeOffset = Math.max(0, Math.min(offset, text.length))
	let start = safeOffset
	let end = safeOffset
	const isWord = (ch: string) => /[\w:]/.test(ch)
	while (start > 0 && isWord(text.charAt(start - 1))) start--
	while (end < text.length && isWord(text.charAt(end))) end++
	return {
		start: doc.positionAt(start),
		end: doc.positionAt(end),
	}
}

export function currentWord(doc: TextDocument, position: { line: number; character: number }): string | null {
	const range = currentWordRange(doc, position)
	const text = doc.getText({ start: range.start, end: range.end })
	return text || null
}

export function abbreviateWithPrefixes(value: string, map: Record<string, string>): string {
	if (!(value.startsWith("http://") || value.startsWith("https://"))) return value
	for (const [prefix, base] of Object.entries(map)) {
		if (value.startsWith(base) && value.length > base.length) {
			const local = value.slice(base.length)
			return `${prefix}:${local}`
		}
	}
	return value
}

export function readFileSafe(fsPath: string): string | null {
	try {
		return fs.readFileSync(fsPath, "utf8")
	} catch {
		return null
	}
}

export function rangeMatchesWord(uri: string, range: Range, word: string, doc: TextDocument | null): boolean {
	if (doc) {
		const text = doc.getText({ start: range.start, end: range.end })
		return text === word
	}
	if (uri.startsWith("file://")) {
		const fsPath = toFsPath(uri)
		const content = readFileSafe(fsPath)
		if (!content) return true
		const lines = content.split(/\r?\n/)
		const startLine = lines[range.start.line] || ""
		const snippet = startLine.slice(range.start.character, range.end.character)
		return snippet === word
	}
	return true
}
