// Monaco and turtle-lsp: the page's side of the language server worker
// (LspClient) and the Monaco providers that turn each editor feature into
// its LSP request (connect). Positions match: LSP counts UTF-16 units from
// 0, Monaco from 1.

import { track } from "./lsp-host.js"

export const LANGUAGES = ["turtle", "trig", "ntriples", "nquads", "sparql"]

// The language of a path, as the server and Monaco know it. JSON-LD is
// Monaco's own JSON (its highlighting); the server knows a `.jsonld` name
// is JSON-LD.
export function languageOf(path) {
  const ext = path.toLowerCase().split(".").pop()
  return { ttl: "turtle", turtle: "turtle", trig: "trig", nt: "ntriples", nq: "nquads", rq: "sparql", sparql: "sparql", ru: "sparql", jsonld: "json" }[ext] ?? "turtle"
}

// Monaco's JSON support keeps only what the server does not do: colours,
// brackets and selection. Its own checks, hovers and completion would
// repeat (or contradict) the server's on JSON-LD.
export function quietJson(monaco) {
  const json = monaco.languages.json?.jsonDefaults
  if (!json) return
  json.setDiagnosticsOptions({ validate: false })
  json.setModeConfiguration({
    tokens: true,
    selectionRanges: true,
    foldingRanges: true,
    colors: false,
    completionItems: false,
    hovers: false,
    documentSymbols: false,
    documentFormattingEdits: false,
    documentRangeFormattingEdits: false,
    diagnostics: false,
  })
}

// The settings the page last sent the server (`workspace/didChangeConfiguration`),
// for the operations that run apart from it: Run Query and Fix All.
let lastSettings = {}
export const currentSettings = () => lastSettings

// What Monaco expects of a request it gave up on.
const canceled = () => Object.assign(new Error("Canceled"), { name: "Canceled" })

export class LspClient {
  constructor(url) {
    this.url = url
    this.nextId = 0
    this.pending = new Map()
    this.inflight = 0
    this.timers = false
    this.handlers = { diagnostics() {}, serverRequest() {}, crash() {}, busy() {} }
    // What a new worker needs to take over from a dead one.
    this.initParams = null
    this.config = null
    this.open = new Map() // uri -> { languageId, version, text }
    this.crashes = [] // when the worker died, the last minute
    this.worker = null
    this.start()
  }

  start() {
    const w = (this.worker = new Worker(this.url, { type: "module" }))
    w.onmessage = ({ data }) => this.worker === w && this.received(data)
    w.onerror = w.onmessageerror = (e) => {
      e.preventDefault?.()
      this.died(w, e.message)
    }
  }

  // A worker that failed to load or died answers nothing more: fail what
  // waits on it, and start another with the documents (the third death within
  // a minute leaves language features off until the page reloads).
  died(w, cause) {
    if (this.worker !== w) return
    w.terminate()
    this.worker = null
    const message = "the language server stopped" + (cause ? `: ${cause}` : "")
    for (const p of this.pending.values()) p.reject(new Error(message))
    this.pending.clear()
    this.inflight = 0
    this.timers = false
    this.handlers.busy(false)
    const now = Date.now()
    this.crashes = [...this.crashes.filter((t) => now - t < 60_000), now]
    if (!this.initParams || this.crashes.length >= 3) {
      this.handlers.crash(message, { state: "off", dropped: [...this.open.keys()] })
      return
    }
    this.start()
    this.post({ kind: "initialize", params: this.initParams }).catch(() => {})
    if (this.config) this.send("workspace/didChangeConfiguration", this.config)
    for (const [uri, d] of this.open) this.send("textDocument/didOpen", { textDocument: { uri, ...d } })
    this.handlers.crash(message, { state: "restarted", dropped: [] })
  }

  on(name, f) {
    this.handlers[name] = f
  }

  get busy() {
    return this.inflight > 0 || this.timers
  }

  initialize(params) {
    this.initParams = params
    return this.post({ kind: "initialize", params })
  }

  // `token`: Monaco's cancellation token; once it is cancelled the request
  // fails with "Canceled", and the worker skips it if it has not begun.
  request(method, params, token) {
    return this.post({ kind: "request", method, params }, token)
  }

  // Texts after every preferred quick fix: [{ name, text }] -> same, with the
  // settings last sent to the server.
  fixAll(files) {
    return this.post({ kind: "fixAll", params: { files, settings: lastSettings } })
  }

  notify(method, params) {
    track(this, method, params)
    if (method === "workspace/didChangeConfiguration") lastSettings = params?.settings ?? {}
    this.send(method, params)
  }

  send(method, params) {
    if (!this.worker) return
    this.inflight++
    this.handlers.busy(true)
    this.worker.postMessage({ kind: "notify", method, params })
  }

  post(message, token) {
    if (!this.worker) return Promise.reject(new Error("the language server is off"))
    if (token?.isCancellationRequested) return Promise.reject(canceled())
    const id = ++this.nextId
    this.inflight++
    this.handlers.busy(true)
    return new Promise((resolve, reject) => {
      const watch = token?.onCancellationRequested?.(() => {
        if (!this.pending.delete(id)) return
        reject(canceled())
        this.worker?.postMessage({ kind: "cancel", id })
      })
      const done = (f) => (v) => (watch?.dispose?.(), f(v))
      this.pending.set(id, { resolve: done(resolve), reject: done(reject) })
      this.worker.postMessage({ ...message, id })
    })
  }

  received(m) {
    if (m.type === "response") {
      const p = this.pending.get(m.id)
      this.pending.delete(m.id)
      if (m.error !== undefined) p?.reject(new Error(m.error))
      else p?.resolve(m.result)
    } else if (m.type === "idle") {
      // One acknowledgement per message sent (`ack`: how many this one is
      // for); a timer's end is not one.
      if (m.ack) this.inflight = Math.max(0, this.inflight - (m.ack === true ? 1 : m.ack))
      this.timers = m.busy
      this.handlers.busy(this.busy)
    } else if (m.type === "diagnostics") this.handlers.diagnostics(m.uri, m.list)
    else if (m.type === "serverRequest") this.handlers.serverRequest(m.method, m.params)
    else if (m.type === "crash") this.handlers.crash(m.message, m)
  }
}

// ---- conversions -------------------------------------------------------------------

const toRange = (monaco, r) => new monaco.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1)
const toPosition = (p) => ({ line: p.lineNumber - 1, character: p.column - 1 })
const fromRange = (r) => ({
  start: { line: r.startLineNumber - 1, character: r.startColumn - 1 },
  end: { line: r.endLineNumber - 1, character: r.endColumn - 1 },
})
const markdown = (c) =>
  c == null ? [] : Array.isArray(c) ? c.flatMap(markdown) : typeof c === "string" ? [{ value: c }] : [{ value: c.value ?? "" }]
const LSP_COMPLETION_KINDS = [
  null, "Text", "Method", "Function", "Constructor", "Field", "Variable", "Class", "Interface", "Module", "Property", "Unit", "Value",
  "Enum", "Keyword", "Snippet", "Color", "File", "Reference", "Folder", "EnumMember", "Constant", "Struct", "Event", "Operator", "TypeParameter",
]

function edits(monaco, workspaceEdit) {
  const out = []
  const add = (uri, list) => {
    for (const e of list) out.push({ resource: monaco.Uri.parse(uri), textEdit: { range: toRange(monaco, e.range), text: e.newText }, versionId: undefined })
  }
  for (const [uri, list] of Object.entries(workspaceEdit?.changes ?? {})) add(uri, list)
  for (const dc of workspaceEdit?.documentChanges ?? []) if (dc.edits) add(dc.textDocument.uri, dc.edits)
  return { edits: out }
}

function locations(monaco, result) {
  const list = result == null ? [] : Array.isArray(result) ? result : [result]
  return list.map((l) => ({ uri: monaco.Uri.parse(l.targetUri ?? l.uri), range: toRange(monaco, l.targetSelectionRange ?? l.range) }))
}

function symbol(monaco, s) {
  return {
    name: s.name,
    detail: s.detail ?? "",
    kind: Math.max(0, (s.kind ?? 1) - 1),
    tags: [],
    range: toRange(monaco, s.range ?? s.location.range),
    selectionRange: toRange(monaco, s.selectionRange ?? s.location.range),
    children: (s.children ?? []).map((c) => symbol(monaco, c)),
  }
}

// ---- providers ---------------------------------------------------------------------------

// Register every provider the server's capabilities offer. `diagnosticsOf(uri)`
// gives the LSP diagnostics last published for a document (code actions
// need them, `data` included). Returns hooks for the server's refreshes.
export function connect(monaco, client, capabilities, diagnosticsOf) {
  const doc = (model) => ({ textDocument: { uri: model.uri.toString() } })
  const at = (model, position) => ({ ...doc(model), position: toPosition(position) })
  const semanticChanged = new monaco.Emitter()
  const inlayChanged = new monaco.Emitter()
  const triggers = capabilities.completionProvider?.triggerCharacters ?? []
  const legend = capabilities.semanticTokensProvider?.legend ?? { tokenTypes: [], tokenModifiers: [] }

  // The server's features also serve JSON-LD files (Monaco's json language;
  // only `.jsonld` files are ever opened in it).
  for (const lang of [...LANGUAGES, "json"]) {
    monaco.languages.registerHoverProvider(lang, {
      async provideHover(model, position, token) {
        const h = await client.request("textDocument/hover", at(model, position), token)
        if (!h) return null
        return { contents: markdown(h.contents), range: h.range ? toRange(monaco, h.range) : undefined }
      },
    })

    monaco.languages.registerCompletionItemProvider(lang, {
      triggerCharacters: triggers,
      async provideCompletionItems(model, position, context, token) {
        const r = await client.request("textDocument/completion", {
          ...at(model, position),
          context: context.triggerCharacter ? { triggerKind: 2, triggerCharacter: context.triggerCharacter } : { triggerKind: 1 },
        }, token)
        const items = r == null ? [] : Array.isArray(r) ? r : r.items
        const word = model.getWordUntilPosition(position)
        const fallback = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, position.column)
        return {
          incomplete: Boolean(r?.isIncomplete),
          suggestions: items.map((i) => ({
            label: i.label,
            kind: monaco.languages.CompletionItemKind[LSP_COMPLETION_KINDS[i.kind] ?? "Text"],
            detail: i.detail,
            documentation: i.documentation ? markdown(i.documentation)[0] : undefined,
            sortText: i.sortText,
            filterText: i.filterText,
            insertText: i.textEdit?.newText ?? i.insertText ?? i.label,
            insertTextRules: i.insertTextFormat === 2 ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
            range: i.textEdit ? toRange(monaco, i.textEdit.range ?? i.textEdit.replace) : fallback,
            additionalTextEdits: (i.additionalTextEdits ?? []).map((e) => ({ range: toRange(monaco, e.range), text: e.newText })),
            lsp: i,
          })),
        }
      },
      async resolveCompletionItem(item, token) {
        const r = await client.request("completionItem/resolve", item.lsp, token)
        if (r?.documentation) item.documentation = markdown(r.documentation)[0]
        if (r?.detail) item.detail = r.detail
        return item
      },
    })

    monaco.languages.registerDefinitionProvider(lang, {
      provideDefinition: async (model, position, token) => locations(monaco, await client.request("textDocument/definition", at(model, position), token)),
    })
    monaco.languages.registerReferenceProvider(lang, {
      provideReferences: async (model, position, context, token) =>
        locations(monaco, await client.request("textDocument/references", { ...at(model, position), context: { includeDeclaration: context.includeDeclaration } }, token)),
    })
    monaco.languages.registerDocumentHighlightProvider(lang, {
      async provideDocumentHighlights(model, position, token) {
        const r = await client.request("textDocument/documentHighlight", at(model, position), token)
        return (r ?? []).map((h) => ({ range: toRange(monaco, h.range), kind: (h.kind ?? 1) - 1 }))
      },
    })

    monaco.languages.registerRenameProvider(lang, {
      async resolveRenameLocation(model, position, token) {
        const r = await client.request("textDocument/prepareRename", at(model, position), token)
        if (!r) return { range: new monaco.Range(1, 1, 1, 1), text: "", rejectReason: "Nothing to rename here" }
        const range = toRange(monaco, r.range ?? r)
        return { range, text: r.placeholder ?? model.getValueInRange(range) }
      },
      async provideRenameEdits(model, position, newName, token) {
        return edits(monaco, await client.request("textDocument/rename", { ...at(model, position), newName }, token))
      },
    })

    monaco.languages.registerDocumentFormattingEditProvider(lang, {
      async provideDocumentFormattingEdits(model, options, token) {
        const r = await client.request("textDocument/formatting", { ...doc(model), options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces } }, token)
        return (r ?? []).map((e) => ({ range: toRange(monaco, e.range), text: e.newText }))
      },
    })

    monaco.languages.registerCodeActionProvider(lang, {
      async provideCodeActions(model, range, _context, token) {
        const wanted = fromRange(range)
        const touches = (d) =>
          !(d.range.end.line < wanted.start.line || d.range.start.line > wanted.end.line ||
            (d.range.end.line === wanted.start.line && d.range.end.character < wanted.start.character) ||
            (d.range.start.line === wanted.end.line && d.range.start.character > wanted.end.character))
        const diagnostics = diagnosticsOf(model.uri.toString()).filter(touches)
        const r = await client.request("textDocument/codeAction", { ...doc(model), range: wanted, context: { diagnostics } }, token)
        // Actions with an edit apply it; those with only a command (fetch an
        // import) run it in the page (registered with monaco.editor.registerCommand).
        const actions = (r ?? [])
          .filter((a) => a.edit || a.command)
          .map((a) => ({
            title: a.title,
            kind: a.kind,
            isPreferred: a.isPreferred,
            edit: a.edit ? edits(monaco, a.edit) : undefined,
            command: a.command ? { id: a.command.command, title: a.command.title, arguments: a.command.arguments } : undefined,
            diagnostics: (a.diagnostics ?? []).map((d) => ({ ...toRange(monaco, d.range), message: d.message, severity: 8 })),
          }))
        return { actions, dispose() {} }
      },
    })

    monaco.languages.registerDocumentSymbolProvider(lang, {
      provideDocumentSymbols: async (model, token) => (await client.request("textDocument/documentSymbol", doc(model), token) ?? []).map((s) => symbol(monaco, s)),
    })
    monaco.languages.registerFoldingRangeProvider(lang, {
      async provideFoldingRanges(model, _context, token) {
        const r = await client.request("textDocument/foldingRange", doc(model), token)
        return (r ?? []).map((f) => ({ start: f.startLine + 1, end: f.endLine + 1 }))
      },
    })

    monaco.languages.registerDocumentSemanticTokensProvider(lang, {
      onDidChange: semanticChanged.event,
      getLegend: () => legend,
      async provideDocumentSemanticTokens(model, _lastResultId, token) {
        const r = await client.request("textDocument/semanticTokens/full", doc(model), token)
        return r ? { data: new Uint32Array(r.data), resultId: r.resultId } : null
      },
      releaseDocumentSemanticTokens() {},
    })

    // "N references" above each subject; a click opens the references view.
    // The query lens ("▶ Run query") is left out: the Run button does that.
    monaco.languages.registerCodeLensProvider(lang, {
      async provideCodeLenses(model, token) {
        const r = await client.request("textDocument/codeLens", doc(model), token)
        const lenses = (r ?? []).filter((l) => !l.command).map((l) => ({ range: toRange(monaco, l.range), lsp: l }))
        return { lenses, dispose() {} }
      },
      async resolveCodeLens(model, lens, token) {
        const r = await client.request("codeLens/resolve", lens.lsp, token)
        if (!r?.command) return lens
        const [, position, refs] = r.command.arguments
        const p = new monaco.Position(position.line + 1, position.character + 1)
        return { ...lens, command: { id: "editor.action.showReferences", title: r.command.title, arguments: [model.uri, p, locations(monaco, refs)] } }
      },
    })

    monaco.languages.registerInlayHintsProvider(lang, {
      onDidChangeInlayHints: inlayChanged.event,
      async provideInlayHints(model, range, token) {
        const r = await client.request("textDocument/inlayHint", { ...doc(model), range: fromRange(range) }, token)
        return {
          hints: (r ?? []).map((h) => ({
            label: typeof h.label === "string" ? h.label : h.label.map((p) => p.value).join(""),
            position: { lineNumber: h.position.line + 1, column: h.position.character + 1 },
            kind: h.kind ?? 1,
            paddingLeft: h.paddingLeft,
            paddingRight: h.paddingRight,
            tooltip: typeof h.tooltip === "string" ? h.tooltip : h.tooltip?.value,
          })),
          dispose() {},
        }
      },
    })
  }

  return {
    // The server asked for a refresh of semantic tokens or inlay hints.
    refresh(method) {
      if (method === "workspace/semanticTokens/refresh") semanticChanged.fire()
      if (method === "workspace/inlayHint/refresh") inlayChanged.fire()
    },
    // A file was put in an editor (or the providers came after it was): ask
    // for its semantic tokens now. Monaco would wait 300 ms or more after
    // attaching a model; a change event from the provider asks at once.
    shown: () => semanticChanged.fire(),
  }
}

// LSP diagnostics as Monaco markers.
export function markers(monaco, list) {
  const severity = { 1: monaco.MarkerSeverity.Error, 2: monaco.MarkerSeverity.Warning, 3: monaco.MarkerSeverity.Info, 4: monaco.MarkerSeverity.Hint }
  return list.map((d) => ({
    ...toRange(monaco, d.range),
    severity: severity[d.severity ?? 1],
    message: d.message,
    code: d.code == null ? undefined : String(d.code),
    source: d.source,
  }))
}

// Monarch colouring before (and under) the server's semantic tokens.
export function registerSyntax(monaco) {
  const common = {
    defaultToken: "",
    tokenizer: {
      root: [
        [/#.*$/, "comment"],
        [/"""/, "string", "@long2"],
        [/'''/, "string", "@long1"],
        [/"([^"\\]|\\.)*"/, "string"],
        [/'([^'\\]|\\.)*'/, "string"],
        [/<[^<>"{}|^`\\\s]*>/, "string.link"],
        [/@[a-zA-Z]+(-[a-zA-Z0-9]+)*/, "annotation"],
        [/[?$][\w]+/, "variable"],
        [/[+-]?(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?/, "number"],
        [/_:[\w.-]+/, "type"],
        [/([A-Za-z][\w.-]*)?:([\w%-]([\w.%-]*[\w%-])?)?/, "type.identifier"],
        [/[A-Za-z_]\w*/, { cases: { "@keywords": "keyword", "@default": "identifier" } }],
      ],
      long2: [[/"""/, "string", "@pop"], [/[^"]+|"/, "string"]],
      long1: [[/'''/, "string", "@pop"], [/[^']+|'/, "string"]],
    },
  }
  const keywords =
    "a true false PREFIX BASE prefix base SELECT DISTINCT REDUCED CONSTRUCT DESCRIBE ASK FROM NAMED WHERE GRAPH OPTIONAL UNION MINUS FILTER BIND VALUES SERVICE SILENT ORDER BY ASC DESC GROUP HAVING LIMIT OFFSET AS INSERT DELETE DATA WITH USING DEFAULT ALL LOAD CLEAR DROP CREATE ADD MOVE COPY INTO TO UNDEF NOT IN EXISTS".split(" ")
  for (const id of LANGUAGES) {
    monaco.languages.register({ id })
    monaco.languages.setMonarchTokensProvider(id, { ...common, keywords, ignoreCase: id === "sparql" })
    monaco.languages.setLanguageConfiguration(id, {
      comments: { lineComment: "#" },
      brackets: [["{", "}"], ["[", "]"], ["(", ")"]],
      autoClosingPairs: [{ open: "{", close: "}" }, { open: "[", close: "]" }, { open: "(", close: ")" }, { open: '"', close: '"', notIn: ["string"] }, { open: "<", close: ">", notIn: ["string"] }],
      // Every alternative matches at least one character: Monaco loops
      // forever on a word pattern that can match an empty string.
      wordPattern: /(-?\d*\.\d\w*)|([?$]\w+)|([\w-]*:[\w.%-]*[\w%-])|([\w-]+:?)/,
    })
  }
}
