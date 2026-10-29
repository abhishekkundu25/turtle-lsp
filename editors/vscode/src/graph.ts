// The workspace graph panel: a webview running the shared graph view
// (web/app/graph, bundled with src/graph-webview.js into out/graph/ by the
// "build" script). The webview asks for `turtle/graph` over postMessage and
// this side forwards it to the language server, opens definitions and hands
// over the diagnostics of the files in each answer.
import { randomBytes } from "node:crypto"
import * as vscode from "vscode"
import type { LanguageClient } from "vscode-languageclient/node"

export const GRAPH_METHOD = "turtle/graph"
export const SHOW_GRAPH = "turtle.showGraph"
const TITLE = "Workspace Graph"
/** Ask again this long after the last change to a workspace RDF or SPARQL file. */
const REFRESH_MS = 500
const WATCHED = "**/*.{ttl,turtle,trig,nt,nq,rq,sparql,ru,jsonld}"

/** Whether the server advertises the graph request (`experimental["turtle/graph"]`). */
export function advertisesGraph(client: LanguageClient | undefined): boolean {
  return client?.initializeResult?.capabilities.experimental?.[GRAPH_METHOD] === true
}

/** What the integration tests read of the open panel. */
export interface GraphProbe {
  /** Requests from the webview the server has answered. */
  answers: number
  /** Nodes in the last answer. */
  nodes: number
  /** The view's status line after its last draw ("Showing N of M nodes"). */
  status: string
  /** Handles `message` as if the webview had posted it. */
  receive(message: unknown): Promise<void>
}

interface Host {
  context: vscode.ExtensionContext
  client(): LanguageClient | undefined
  served(doc: vscode.TextDocument): boolean
}

let current: GraphPanel | undefined

/** The open panel's probe, for the integration tests. */
export function graphProbe(): GraphProbe | undefined {
  return current?.probe
}

/** Registers `turtle.showGraph`: opens the panel, or shows the open one. */
export function registerGraphCommand(host: Host): vscode.Disposable {
  const command = vscode.commands.registerCommand(SHOW_GRAPH, () => {
    if (current) current.panel.reveal()
    else current = new GraphPanel(host)
  })
  return new vscode.Disposable(() => {
    command.dispose()
    current?.panel.dispose()
  })
}

type Position = { line: number; character: number }
type Range = { start: Position; end: Position }

const isPosition = (p: unknown): p is Position =>
  typeof p === "object" && p !== null && Number.isInteger((p as Position).line) && Number.isInteger((p as Position).character)
const isRange = (r: unknown): r is Range => typeof r === "object" && r !== null && isPosition((r as Range).start) && isPosition((r as Range).end)

const toRange = (r: Range) => new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character)
const fromRange = (r: vscode.Range): Range => ({
  start: { line: r.start.line, character: r.start.character },
  end: { line: r.end.line, character: r.end.character },
})

class GraphPanel {
  readonly panel: vscode.WebviewPanel
  readonly probe: GraphProbe
  private readonly host: Host
  private readonly disposables: vscode.Disposable[] = []
  private timer: ReturnType<typeof setTimeout> | undefined

  constructor(host: Host) {
    this.host = host
    const root = vscode.Uri.joinPath(host.context.extensionUri, "out", "graph")
    // No retainContextWhenHidden: the webview keeps its filters and mode with
    // setState, and asks again when it is shown.
    this.panel = vscode.window.createWebviewPanel("turtle.graph", TITLE, vscode.ViewColumn.Active, {
      enableScripts: true,
      localResourceRoots: [root],
    })
    this.panel.webview.html = page(this.panel.webview, root)
    this.probe = { answers: 0, nodes: 0, status: "", receive: (m) => this.receive(m) }
    const refreshOn = (doc: vscode.TextDocument) => {
      if (host.served(doc)) this.scheduleRefresh()
    }
    const watcher = vscode.workspace.createFileSystemWatcher(WATCHED)
    this.disposables.push(
      this.panel.webview.onDidReceiveMessage((m) => this.receive(m)),
      vscode.workspace.onDidChangeTextDocument((e) => refreshOn(e.document)),
      vscode.workspace.onDidSaveTextDocument(refreshOn),
      watcher,
      watcher.onDidCreate(() => this.scheduleRefresh()),
      watcher.onDidChange(() => this.scheduleRefresh()),
      watcher.onDidDelete(() => this.scheduleRefresh()),
      vscode.window.onDidChangeActiveColorTheme(() => void this.post({ type: "theme" })),
    )
    this.panel.onDidDispose(() => this.dispose())
  }

  private dispose(): void {
    clearTimeout(this.timer)
    for (const d of this.disposables.splice(0)) d.dispose()
    if (current === this) current = undefined
  }

  /** A hidden webview is gone (no retained context) and asks afresh when shown. */
  private scheduleRefresh(): void {
    clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      if (this.panel.visible) void this.post({ type: "refresh" })
    }, REFRESH_MS)
  }

  private post(message: unknown): Thenable<boolean> {
    return this.panel.webview.postMessage(message)
  }

  private async receive(message: unknown): Promise<void> {
    if (typeof message !== "object" || message === null) return
    const m = message as Record<string, unknown>
    if (m.type === "request" && typeof m.id === "number") await this.answer(m.id, m.method, m.params)
    else if (m.type === "reveal" && typeof m.uri === "string" && isRange(m.range)) await reveal(m.uri, m.range)
    else if (m.type === "drawn" && typeof m.status === "string") this.probe.status = m.status
  }

  /** Forwards request `id` to the server; the answer goes back with the diagnostics of its files. */
  private async answer(id: number, method: unknown, params: unknown): Promise<void> {
    const client = this.host.client()
    try {
      if (method !== GRAPH_METHOD) throw new Error(`unknown request ${String(method)}`)
      if (!client?.isRunning() || !advertisesGraph(client)) throw new Error("the Turtle language server is not running")
      const result = await client.sendRequest<{ nodes?: { uri?: string }[] }>(GRAPH_METHOD, params ?? {})
      this.probe.answers++
      this.probe.nodes = result?.nodes?.length ?? 0
      await this.post({ type: "response", id, result, diagnostics: diagnosticsOf(result?.nodes ?? []) })
    } catch (err) {
      await this.post({ type: "response", id, error: err instanceof Error ? err.message : String(err) })
    }
  }
}

/** Each node file's diagnostics, as LSP ranges and severities (1 = error … 4 = hint). */
function diagnosticsOf(nodes: { uri?: string }[]): Record<string, { range: Range; severity: number }[]> {
  const out: Record<string, { range: Range; severity: number }[]> = {}
  for (const uri of new Set(nodes.map((n) => n.uri).filter((u): u is string => typeof u === "string"))) {
    const diags = vscode.languages.getDiagnostics(vscode.Uri.parse(uri))
    if (diags.length) out[uri] = diags.map((d) => ({ range: fromRange(d.range), severity: d.severity + 1 }))
  }
  return out
}

/** Opens `uri` at `range`, beside the panel. */
async function reveal(uri: string, range: Range): Promise<void> {
  const target = vscode.Uri.parse(uri)
  if (target.scheme !== "file" && target.scheme !== "untitled") return
  await vscode.window.showTextDocument(target, { selection: toRange(range), viewColumn: vscode.ViewColumn.Beside, preview: false })
}

/** The webview's page: the bundled view and its styles, nothing inline but the nonce'd script tag. */
function page(webview: vscode.Webview, root: vscode.Uri): string {
  const nonce = randomBytes(16).toString("base64")
  const src = webview.cspSource
  const csp = `default-src 'none'; script-src ${src} 'nonce-${nonce}'; style-src ${src}; img-src ${src} data:; font-src ${src}`
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${TITLE}</title>
<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.joinPath(root, "graph.css"))}">
</head>
<body>
<div id="graph"></div>
<script nonce="${nonce}" src="${webview.asWebviewUri(vscode.Uri.joinPath(root, "graph.js"))}"></script>
</body>
</html>`
}
