// The colour theme (Settings: Workbench > Appearance: Color Theme): auto
// follows the system, light and dark force it. It sets the page's
// `data-theme` attribute, which the CSS tokens follow, and Monaco's theme.
// The choice is mirrored in localStorage under KEY, which editor.html reads
// before the first paint.

export const KEY = "rdf-editor-theme"

/**
 * @param {object} o
 * @param {object} o.monaco
 * @returns {{ set(value: "auto" | "light" | "dark"): void, monacoTheme(): string }}
 */
export function createTheme({ monaco }) {
  // Placeholder until the theme slice lands: auto only, as before.
  const dark = matchMedia("(prefers-color-scheme: dark)")
  const monacoTheme = () => (dark.matches ? "rdf-dark" : "rdf-light")
  dark.addEventListener("change", () => monaco.editor.setTheme(monacoTheme()))
  return { set() {}, monacoTheme }
}
