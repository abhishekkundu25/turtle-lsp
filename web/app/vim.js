// Vim mode (Settings: Editor: Vim Mode), with monaco-vim, loaded the first
// time it is turned on. Every editor group's editor is attached; the mode
// and the `:` command line show in one status-bar element.

/**
 * @param {object} o
 * @param {object} o.monaco
 * @param {HTMLElement} o.status the status-bar element for the mode and the command line
 * @param {{ save(): void, saveAll(): void, close(): void, open(path: string): boolean }} o.commands
 *   what :w, :wa, :q and :e <path> do
 * @returns {{ set(enabled: boolean): void, attach(editor: object): void, detach(editor: object): void }}
 */
export function createVim({ monaco, status, commands }) {
  // Placeholder until the Vim slice lands.
  return { set() {}, attach() {}, detach() {} }
}
