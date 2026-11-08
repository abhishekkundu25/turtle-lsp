// The Query Results panel: the client declares
// experimental["turtle/queryResults"], so the server answers
// `turtle.runQuery` with a result object instead of writing a Markdown file
// (docs/specs/query-results-panel.md, "Server seam"). A middleware on
// executeCommand passes the command on and shows the answer in its query
// document's panel, one per document, beside the editor. The page and the
// exports are src/query-results-view.js, bundled into out/query-results/ by
// the "build" script; its messages are read here, at the edge.
import * as vscode from "vscode"
import type { Middleware, StaticFeature } from "vscode-languageclient"
import { csvOf, jsonOf, parseAnswer, parseMessage, parseReasoning, tsvOf } from "./query-results-view.js"
import type { Message, Reasoning, Result, RunAnswer, View } from "./query-results-view.js"

export const RUN_QUERY = "turtle.runQuery"
export const QUERY_RESULTS_CAPABILITY = "turtle/queryResults"
const VIEW_TYPE = "turtle.queryResults"

/** Declares experimental["turtle/queryResults"] in the client's initialize. */
export const queryResultsFeature: StaticFeature = {
  fillClientCapabilities(capabilities) {
    capabilities.experimental = { ...(capabilities.experimental as object | undefined), [QUERY_RESULTS_CAPABILITY]: true }
  },
  initialize() {},
  getState: () => ({ kind: "static" }),
  clear() {},
}

type Next = (args: unknown[]) => Promise<unknown>

/** What the integration tests read of the open panels. */
export interface QueryResultsProbe {
  /** The open panels. */
  count(): number
  /** The panel of the query document `uri`, if open. */
  panel(uri: string): PanelProbe | undefined
}

export interface PanelProbe {
  /** Results and cancellations shown. */
  answers: number
  /** The page's summary line after its last draw. */
  status: string
  /** Handles `message` as if the webview had posted it. */
  receive(message: unknown): Promise<void>
}

const failure = (err: unknown) => (err instanceof Error ? err.message : String(err))

/** The panels, by query document URI, and the executeCommand middleware that fills them. */
export class QueryResults implements vscode.Disposable {
  private readonly panels = new Map<string, ResultsPanel>()
  private readonly context: vscode.ExtensionContext

  constructor(context: vscode.ExtensionContext) {
    this.context = context
  }

  readonly middleware: Middleware["executeCommand"] = (command, args, next) =>
    command === RUN_QUERY ? this.run(args, (a) => Promise.resolve(next(command, a))) : next(command, args)

  readonly probe: QueryResultsProbe = {
    count: () => this.panels.size,
    panel: (uri) => this.panels.get(uri)?.probe,
  }

  /** Runs `turtle.runQuery` with `args` (`[uri, {reasoning}?]`) and shows a result object in the document's panel. */
  private async run(args: unknown[], next: Next): Promise<unknown> {
    const [uri, options] = args
    if (typeof uri !== "string") return next(args)
    // Without an explicit choice the run uses the setting, so the panel starts there.
    const reasoning =
      parseReasoning((options as { reasoning?: unknown } | undefined)?.reasoning) ??
      parseReasoning(vscode.workspace.getConfiguration("turtle").get("query.reasoning")) ??
      "off"
    const existing = this.panels.get(uri)
    const panel = existing ?? this.open(uri)
    const run = panel.start(reasoning)
    let answer: unknown
    try {
      answer = await next(args)
    } catch (err) {
      if (existing) panel.fail(run, failure(err))
      else panel.dispose()
      throw err
    }
    const read = parseAnswer(answer)
    if (read && read.uri === uri) panel.finish(run, read)
    else if (existing) panel.abandon(run)
    // A server without the result object writes the Markdown document instead.
    else panel.dispose()
    return answer
  }

  private open(uri: string): ResultsPanel {
    const panel = new ResultsPanel(this.context, uri, () => this.panels.delete(uri))
    this.panels.set(uri, panel)
    return panel
  }

  dispose(): void {
    for (const p of [...this.panels.values()]) p.dispose()
  }
}

/** One query document's panel. Each run gets a number; only the latest one's answer is shown. */
class ResultsPanel {
  readonly panel: vscode.WebviewPanel
  readonly probe: PanelProbe
  private readonly uri: string
  private view: View = { phase: "running" }
  private runs = 0
  /** The reasoning of the last run started: the Inferred choice until its result comes back. */
  private reasoning: Reasoning = "off"
  private disposed = false

  constructor(context: vscode.ExtensionContext, uri: string, onDispose: () => void) {
    this.uri = uri
    const root = vscode.Uri.joinPath(context.extensionUri, "out", "query-results")
    // No retainContextWhenHidden: the page asks for the view again when shown.
    this.panel = vscode.window.createWebviewPanel(
      VIEW_TYPE,
      titleOf(uri.slice(uri.lastIndexOf("/") + 1)),
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, localResourceRoots: [root] },
    )
    this.panel.webview.html = page(this.panel.webview, root)
    this.probe = { answers: 0, status: "", receive: (m) => this.receive(m) }
    const listener = this.panel.webview.onDidReceiveMessage((m) => this.receive(m))
    this.panel.onDidDispose(() => {
      this.disposed = true
      listener.dispose()
      onDispose()
    })
  }

  dispose(): void {
    if (!this.disposed) this.panel.dispose()
  }

  /** A run starts: "Running…" over the last result. Returns the run's number. */
  start(reasoning: Reasoning): number {
    this.reasoning = reasoning
    this.show({ phase: "running", result: this.view.result, reasoning })
    if (this.runs > 0) this.panel.reveal(undefined, true)
    return ++this.runs
  }

  finish(run: number, answer: RunAnswer): void {
    if (run !== this.runs) return
    this.probe.answers++
    if (answer.kind === "cancelled") return this.show({ phase: "cancelled", result: this.view.result, reasoning: this.reasoning })
    this.panel.title = titleOf(answer.name)
    this.show({ phase: "done", result: answer })
  }

  fail(run: number, message: string): void {
    if (run === this.runs) this.show({ phase: "failed", message, result: this.view.result, reasoning: this.reasoning })
  }

  /** An answer that is not a result: back to the last one. */
  abandon(run: number): void {
    if (run !== this.runs) return
    const result = this.view.result
    this.show(result ? { phase: "done", result } : { phase: "failed", message: "The server did not answer with results.", reasoning: this.reasoning })
  }

  private show(view: View): void {
    this.view = view
    if (!this.disposed) void this.panel.webview.postMessage({ type: "show", view })
  }

  private async receive(raw: unknown): Promise<void> {
    const m = parseMessage(raw)
    if (!m) return
    switch (m.type) {
      case "ready":
        return this.show(this.view)
      case "drawn":
        this.probe.status = m.status
        return
      case "rerun":
        return this.rerun(m.reasoning)
      case "copy":
      case "export":
        return this.export(m)
    }
  }

  private async rerun(reasoning: Reasoning): Promise<void> {
    try {
      // Through the client's command, so the middleware shows the answer here.
      await vscode.commands.executeCommand(RUN_QUERY, this.uri, { reasoning })
    } catch (err) {
      // A run that reached the middleware already shows its failure.
      if (this.view.phase !== "failed") this.show({ phase: "failed", message: failure(err), result: this.view.result, reasoning: this.reasoning })
    }
  }

  private async export(m: Extract<Message, { type: "copy" | "export" }>): Promise<void> {
    const result: Result | undefined = this.view.phase === "done" ? this.view.result : undefined
    if (!result) return
    const answer = result.answer
    const size = answer.kind === "select" ? answer.rows.length : answer.kind === "construct" ? answer.triples.length : 1
    // The order is the page's: every row once.
    const order = m.order && m.order.length === size && new Set(m.order).size === size && m.order.every((i) => i < size) ? m.order : undefined
    if (m.type === "copy") {
      await vscode.env.clipboard.writeText(tsvOf(answer, order))
      vscode.window.setStatusBarMessage("Query results copied", 3000)
      return
    }
    const ext = m.format
    const stem = result.name.replace(/\.[^.]*$/, "")
    const query = vscode.Uri.parse(this.uri)
    const target = await vscode.window.showSaveDialog({
      defaultUri: query.scheme === "untitled" ? undefined : vscode.Uri.joinPath(query, "..", `${stem}.${ext}`),
      filters: ext === "csv" ? { CSV: ["csv"] } : { JSON: ["json"] },
      saveLabel: "Export",
    })
    if (!target) return
    const text = ext === "csv" ? csvOf(answer, order) : jsonOf(answer, order)
    try {
      await vscode.workspace.fs.writeFile(target, new TextEncoder().encode(text))
      vscode.window.setStatusBarMessage(`Query results exported to ${target.path.slice(target.path.lastIndexOf("/") + 1)}`, 3000)
    } catch (err) {
      void vscode.window.showErrorMessage(`The query results were not exported: ${failure(err)}`)
    }
  }
}

const titleOf = (name: string) => `Query Results: ${name}`

/** The webview's page: the bundled view and its styles, nothing inline but the nonce'd script tag. */
function page(webview: vscode.Webview, root: vscode.Uri): string {
  const nonce = btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(16))))
  const src = webview.cspSource
  const csp = `default-src 'none'; script-src ${src} 'nonce-${nonce}'; style-src ${src}; font-src ${src}`
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Query Results</title>
<link rel="stylesheet" href="${webview.asWebviewUri(vscode.Uri.joinPath(root, "query-results.css"))}">
</head>
<body>
<div id="results"></div>
<script nonce="${nonce}" src="${webview.asWebviewUri(vscode.Uri.joinPath(root, "query-results.js"))}"></script>
</body>
</html>`
}
