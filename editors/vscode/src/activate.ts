// What the extension does once it can build a language client, on the
// desktop and in the browser alike: the restart, references and graph
// commands, the start on the first served document, and the settings sent to
// the server. Each entry passes the documents it serves and a factory that
// builds the client (src/extension.ts runs the binary, src/web/extension.ts
// a worker).
import * as vscode from "vscode"
import type { BaseLanguageClient, LanguageClientOptions } from "vscode-languageclient"
import { configurationChange, layered, RDF_GLOB, ServerHandle, settingsFrom, type DocumentFilter } from "./client"
import { advertisesGraph, GraphProbe, graphProbe, registerGraphCommand } from "./graph"
import { QueryResults, queryResultsFeature, type QueryResultsProbe } from "./query-results"

export type { DocumentFilter }

export interface ClientSetup {
  /** The documents the server handles. The server starts once one is open or the workspace has an RDF file. */
  documentSelector: DocumentFilter[]
  /**
   * Builds a new, not yet started client from `options`, or returns
   * `undefined` after telling the user why there is no server to run. Called
   * on every start and restart.
   */
  createClient(options: LanguageClientOptions): BaseLanguageClient | undefined
  /**
   * Settings beneath the user's, where the server cannot read them itself
   * (the browser's `turtle-lsp.json`). The server is sent the new settings
   * when they change.
   */
  baseSettings?: { current(): Record<string, unknown>; onDidChange: vscode.Event<void> }
}

/** Exposed for the integration tests. */
export interface TurtleApi {
  client(): BaseLanguageClient | undefined
  /** Settles when every start, restart and stop asked so far has finished. */
  idle(): Promise<void>
  /** The open graph panel, if any. */
  graph(): GraphProbe | undefined
  /** The Query Results panels. */
  queryResults(): QueryResultsProbe
}

/**
 * The `turtle.*` settings the server reads, as nested objects: the ones the
 * user set (not the defaults the manifest contributes, which would override
 * `turtle-lsp.json`), with network imports off and the reasoning rules the
 * user's own in an untrusted workspace, over the entry's `base` settings.
 */
function settingsFor(context: vscode.ExtensionContext, setup: ClientSetup): Record<string, unknown> {
  const manifest = context.extension.packageJSON
  const config = vscode.workspace.getConfiguration("turtle")
  const user = settingsFrom({
    properties: manifest.contributes.configuration.properties,
    restricted: manifest.capabilities.untrustedWorkspaces.restrictedConfigurations,
    inspect: (key) => config.inspect(key),
    trusted: vscode.workspace.isTrusted,
  })
  return layered(setup.baseSettings?.current() ?? {}, user)
}

/** Sends the server its settings; the section replaces the ones it started with. */
async function sendSettings(context: vscode.ExtensionContext, setup: ClientSetup, client: BaseLanguageClient | undefined): Promise<void> {
  if (client?.isRunning()) await client.sendNotification("workspace/didChangeConfiguration", configurationChange(settingsFor(context, setup)))
}

/**
 * The client options both entries share; the configuration middleware
 * notifies `client()`, and `turtle.runQuery`'s answer goes to `results`.
 */
function clientOptions(
  context: vscode.ExtensionContext,
  setup: ClientSetup,
  client: () => BaseLanguageClient | undefined,
  results: QueryResults,
): LanguageClientOptions {
  return {
    documentSelector: setup.documentSelector,
    initializationOptions: settingsFor(context, setup),
    // Later changes reach the server as workspace/didChangeConfiguration.
    synchronize: { configurationSection: "turtle" },
    outputChannelName: "Turtle",
    middleware: {
      // Run query's result object opens in the query's Query Results panel.
      executeCommand: results.middleware,
      workspace: {
        // The client would send the whole section, defaults included, which
        // beats turtle-lsp.json at the server. Send the user's settings instead.
        didChangeConfiguration: () => sendSettings(context, setup, client()),
        // The same settings for a server that asks for its section.
        configuration: (params, token, next) =>
          params.items.every((i) => i.section === "turtle") ? params.items.map(() => settingsFor(context, setup)) : next(params, token),
      },
      window: {
        // A server without the result object writes the results as a
        // Markdown document: it opens beside the query, which keeps the focus.
        showDocument: async (params, token, next) => {
          if (!/\/query-results\/[^/]+\.md$/.test(params.uri)) return next(params, token)
          await vscode.window.showTextDocument(vscode.Uri.parse(params.uri), {
            viewColumn: vscode.ViewColumn.Beside,
            preserveFocus: true,
            preview: false,
          })
          return { success: true }
        },
      },
    },
  }
}

interface LspPosition {
  line: number
  character: number
}
interface LspLocation {
  uri: string
  range: { start: LspPosition; end: LspPosition }
}

const asPosition = (p: LspPosition) => new vscode.Position(p.line, p.character)

/**
 * A reference lens's command (`turtle.showReferences`, from the server, with
 * the document URI, the lens position and the reference locations): opens
 * the references view there. The client runs it itself: a lens command the
 * server does not offer as an `executeCommand` reaches VS Code's command
 * registry as is, with the protocol's JSON as arguments.
 */
async function showReferences(uri: string, position: LspPosition, locations: LspLocation[]): Promise<void> {
  await vscode.commands.executeCommand(
    "editor.action.showReferences",
    vscode.Uri.parse(uri),
    asPosition(position),
    locations.map((l) => new vscode.Location(vscode.Uri.parse(l.uri), new vscode.Range(asPosition(l.range.start), asPosition(l.range.end)))),
  )
}

/**
 * Whether to start the server now. Opening any JSON file activates the
 * extension (`onLanguage:json`, for `.jsonld` files): start only once the
 * workspace has a file the server handles, or one is open.
 */
async function wanted(served: (doc: vscode.TextDocument) => boolean): Promise<boolean> {
  if (vscode.workspace.textDocuments.some(served)) return true
  const files = await vscode.workspace.findFiles(RDF_GLOB, "**/node_modules/**", 1)
  return files.length > 0
}

let serverHandle: ServerHandle<BaseLanguageClient> | undefined

export async function activate(context: vscode.ExtensionContext, setup: ClientSetup): Promise<TurtleApi> {
  /** Whether the client sends `doc` to the server. */
  const served = (doc: vscode.TextDocument): boolean => vscode.languages.match(setup.documentSelector, doc) > 0
  const results = new QueryResults(context)
  context.subscriptions.push(results)
  const handle = (serverHandle = new ServerHandle<BaseLanguageClient>(() => {
    let created: BaseLanguageClient | undefined
    created = setup.createClient(clientOptions(context, setup, () => created, results))
    // Declares experimental["turtle/queryResults"] in initialize.
    created?.registerFeature(queryResultsFeature)
    return created
  }))
  let wantedServer = false
  // The workspace graph panel (src/graph.ts): the command exists only while
  // the server advertises experimental["turtle/graph"], and the context key
  // turtle.graphAvailable shows its palette entry, editor title button and
  // Explorer menu item only then.
  let graphCommand: vscode.Disposable | undefined
  const syncGraphCommand = (): void => {
    const advertised = advertisesGraph(handle.client())
    void vscode.commands.executeCommand("setContext", "turtle.graphAvailable", advertised)
    if (advertised && !graphCommand) {
      graphCommand = registerGraphCommand({ context, client: () => handle.client(), served })
    } else if (!advertised && graphCommand) {
      graphCommand.dispose()
      graphCommand = undefined
    }
  }
  context.subscriptions.push({ dispose: () => graphCommand?.dispose() })
  /** Runs a start or restart; a failure is shown, not thrown, so the restart command still works. */
  const run = async (step: () => Promise<unknown>): Promise<void> => {
    try {
      await step()
    } catch (err) {
      void vscode.window.showErrorMessage(`The Turtle language server did not start: ${err instanceof Error ? err.message : String(err)}`)
    }
    syncGraphCommand()
  }
  const start = (): Promise<void> => {
    wantedServer = true
    return run(() => handle.ensureStarted())
  }
  context.subscriptions.push(
    vscode.commands.registerCommand("turtle.restartServer", () => {
      wantedServer = true
      return run(() => handle.restart())
    }),
    vscode.commands.registerCommand("turtle.showReferences", showReferences),
    ...(setup.baseSettings ? [setup.baseSettings.onDidChange(() => sendSettings(context, setup, handle.client()))] : []),
    // Trust decides what the server may fetch, and on the desktop which binary may run.
    vscode.workspace.onDidGrantWorkspaceTrust(async () => {
      if (wantedServer) await vscode.commands.executeCommand("turtle.restartServer")
    }),
  )
  if (await wanted(served)) {
    await start()
  } else {
    const waiting = vscode.workspace.onDidOpenTextDocument(async (doc) => {
      if (wantedServer || !served(doc)) return
      waiting.dispose()
      await start()
    })
    context.subscriptions.push(waiting)
  }
  return { client: () => handle.client(), idle: () => handle.idle(), graph: graphProbe, queryResults: () => results.probe }
}

export async function deactivate(): Promise<void> {
  await serverHandle?.dispose()
}
