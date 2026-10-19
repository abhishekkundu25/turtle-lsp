// The colour theme (Settings: Workbench > Appearance: Color Theme): dark
// (the default) or light, or auto to follow the system. It sets the page's
// `data-theme` attribute to the resolved "light" or "dark" (also for auto),
// which the CSS tokens follow, and Monaco's theme. The choice is kept in
// localStorage under KEY, which editor.html reads before the first paint
// (nothing kept is dark).

export const KEY = "rdf-editor-theme"

/**
 * @param {object} o
 * @param {object} o.monaco
 * @returns {{ set(value: "auto" | "light" | "dark"): void, monacoTheme(): string }}
 */
export function createTheme({ monaco }) {
  const system = matchMedia("(prefers-color-scheme: dark)")
  const root = document.documentElement
  // Until set() is called, what editor.html applied: dark, light, or none for auto.
  let choice = root.dataset.theme === "light" || root.dataset.theme === "dark" ? root.dataset.theme : "auto"
  const resolved = () => (choice === "auto" ? (system.matches ? "dark" : "light") : choice)
  const monacoTheme = () => (resolved() === "dark" ? "rdf-dark" : "rdf-light")

  function apply() {
    root.dataset.theme = resolved()
    monaco.editor.setTheme(monacoTheme())
  }
  system.addEventListener("change", () => choice === "auto" && apply())

  return {
    set(value) {
      choice = value === "light" || value === "dark" ? value : "auto"
      apply()
      try {
        localStorage.setItem(KEY, choice)
      } catch {}
    },
    monacoTheme,
  }
}
