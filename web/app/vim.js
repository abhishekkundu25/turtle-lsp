// Vim mode (Settings: Editor: Vim Mode), with monaco-vim, loaded the first
// time it is turned on. Every editor group's editor is attached; the mode
// and the `:` command line show in one status-bar element, one line per
// editor, the focused editor's shown. The page's own shortcuts get past Vim.

// The name monaco-vim's UMD build asks the AMD loader for Monaco's API by.
const API = "monaco-editor/esm/vs/editor/editor.api"

// The page's shortcuts (editor.js's document keydown listener). Vim binds
// some of them (Ctrl+P is `k`), so they are marked handled before Vim sees
// them and go on to the page.
function pageKey(e) {
  const mod = e.ctrlKey || e.metaKey
  const k = e.key.toLowerCase()
  return e.key === "F1" || (mod && (["p", "t", "s", "\\"].includes(k) || (e.shiftKey && k === "f")))
}

/**
 * @param {object} o
 * @param {object} o.monaco
 * @param {HTMLElement} o.status the status-bar element for the mode and the command line
 * @param {{ save(): void, saveAll(): void, close(): void, open(path: string): boolean }} o.commands
 *   what :w, :wa, :q and :e <path> do
 * @returns {{ set(enabled: boolean): void, attach(editor: object): void, detach(editor: object): void }}
 */
export function createVim({ monaco, status, commands }) {
  const editors = new Map() // editor → its listeners
  const sessions = new Map() // editor → { vim, node }
  let enabled = false
  let focused = null
  let lib = null
  let loading = null

  function load() {
    loading ??= new Promise((resolve, reject) => {
      window.define(API, [], () => monaco)
      window.require(["vendor/monaco-vim/monaco-vim"], resolve, reject)
    }).then((MV) => {
      const { Vim } = MV.VimMode
      Vim.defineEx("write", "w", () => commands.save())
      Vim.defineEx("wall", "wa", () => commands.saveAll())
      Vim.defineEx("quit", "q", () => commands.close())
      Vim.defineEx("edit", "e", (cm, params) => {
        const path = (params.argString ?? "").trim().replace(/^\.\//, "")
        if (!path) cm.openNotification("E32: No file name")
        else if (!commands.open(path)) cm.openNotification(`E484: Can't open file ${path}`)
      })
      lib = MV
    })
    return loading
  }

  function show() {
    for (const [ed, s] of sessions) s.node.hidden = ed !== focused
    status.hidden = !sessions.size
  }

  function start(ed) {
    if (sessions.has(ed)) return
    const node = document.createElement("span")
    node.className = "vim-status"
    status.append(node)
    sessions.set(ed, { vim: lib.initVimMode(ed, node), node })
  }

  function stop(ed) {
    const s = sessions.get(ed)
    if (!s) return
    sessions.delete(ed)
    s.vim.dispose()
    s.node.remove()
  }

  return {
    set(on) {
      enabled = on
      if (!on) {
        for (const ed of [...sessions.keys()]) stop(ed)
        return show()
      }
      load().then(
        () => {
          if (!enabled) return
          for (const ed of editors.keys()) start(ed)
          show()
        },
        (err) => {
          loading = null
          console.error("Vim mode could not be loaded:", err)
        }
      )
    },
    attach(ed) {
      if (editors.has(ed)) return
      // Registered before Vim's own key listener, so it runs first.
      const keys = ed.onKeyDown((e) => sessions.has(ed) && pageKey(e.browserEvent) && e.browserEvent.preventDefault())
      // Text focus as well as widget focus: when a split's new editor takes
      // the focus from inside this one's keydown, Monaco can leave this
      // editor's widget focus set, and focusing it again then fires no
      // widget-focus event.
      const onFocus = () => ((focused = ed), show())
      const focus = [ed.onDidFocusEditorWidget(onFocus), ed.onDidFocusEditorText(onFocus)]
      editors.set(ed, [keys, ...focus])
      focused ??= ed
      if (enabled && lib) (start(ed), show())
    },
    detach(ed) {
      for (const d of editors.get(ed) ?? []) d.dispose()
      editors.delete(ed)
      stop(ed)
      if (focused === ed) focused = editors.keys().next().value ?? null
      show()
    },
  }
}
