# RDF Editor: Vim mode and colour theme

Status: in progress, on branch `web-showcase` (not merged to main).

## Problem Statement

People who edit with Vim keys everywhere else have to drop them in the
RDF Editor, and the editor's colours always follow the system's light or
dark setting, with no way to choose. VS Code users get both from a setting
(the Vim extension, and Color Theme).

## Solution

Two settings in the editor's Settings page (Ctrl+,), kept with the
workspace like every other setting:

- **Editor: Vim Mode** (off by default). When on, every editor works with
  Vim keys: normal, insert and visual modes, counts, operators, text
  objects, registers, marks, `.`, `/` search, `:s`, undo and redo. The mode
  and the `:` command line show in the status bar, and `:w`, `:wa`, `:q`
  and `:e <file>` save, save all, close the tab and open a file. Turning it
  off gives plain editing back at once.
- **Workbench: Color Theme**: Auto (follow the system, the default), Light
  or Dark. It applies to the whole page (explorer, tabs, panels, status
  bar, dialogs, Settings, Search, diff) and to the editors, at once.

## User Stories

1. As a Vim user, I want a Vim Mode setting, so that I edit RDF with the keys my hands know.
2. As a user who does not use Vim, I want it off by default, so that nothing changes for me.
3. As a Vim user, I want the setting kept across visits, so that I set it once.
4. As a Vim user, I want it to take effect at once in every open editor, so that I need no reload.
5. As a Vim user, I want normal, insert, visual and visual-line modes, so that editing works as in Vim.
6. As a Vim user, I want motions, counts, operators and text objects (`w`, `b`, `e`, `gg`, `G`, `3dd`, `ciw`, `di"`, `>>`), so that I move and change text quickly.
7. As a Vim user, I want `u` and Ctrl+R, so that I undo and redo with Vim keys.
8. As a Vim user, I want yank, put, registers and marks, so that copying and jumping work.
9. As a Vim user, I want `/`, `?`, `n`, `N`, `*` and `:s`, so that I search and replace in the file.
10. As a Vim user, I want the current mode in the status bar, so that I know which mode I am in.
11. As a Vim user, I want the `:` command line in the status bar, so that I see what I type.
12. As a Vim user, I want `:w` to save the file (to disk for a folder opened in place), so that saving works as in Vim.
13. As a Vim user, I want `:wa` to save every file, so that I can save all at once.
14. As a Vim user, I want `:q` to close the tab, asking first when it is unsaved, so that I do not lose work.
15. As a Vim user, I want `:e <file>` to open a workspace file by path, so that I can switch files without the mouse.
16. As a Vim user, I want both editors of a split to use Vim keys, so that the split behaves the same.
17. As a Vim user, I want completion, hover, quick fixes, rename and formatting to keep working, so that Vim mode costs me none of the language server.
18. As a Vim user, I want the page's own shortcuts (Ctrl+P, Ctrl+S, F1, Ctrl+Shift+F, Ctrl+\) to keep working, so that the workbench stays usable.
19. As a user, I want turning Vim mode off to restore plain typing and remove the mode from the status bar, so that switching is safe.
20. As a user who never turns it on, I want the Vim code not loaded at all, so that the page stays as fast.
21. As a user, I want a Color Theme setting with Auto, Light and Dark, so that I choose the editor's colours.
22. As a user, I want Auto to follow my system and change when the system does, so that the default stays as today.
23. As a user, I want Light or Dark to apply to the whole page and to every editor, the diff view included, so that nothing is left in the other colours.
24. As a user, I want the theme applied before the page shows on a reload, so that it does not flash in the other colours.
25. As a user, I want the theme kept across visits, so that I set it once.
26. As a user, I want "Reset All" in Settings to return both settings to their defaults, so that I can undo my choices.

## Implementation Decisions

- **Settings.** Two entries in the Settings list: `editor.vimMode` (bool,
  default false, group Editor) and `workbench.colorTheme` (enum
  auto/light/dark, default auto, a new group Workbench). The Settings
  module takes per-key effects: a function per key, called with the value
  when the page starts and on every change. The editor registers one for
  each new key.
- **Theme module.** Owns the colour theme: `set(value)` resolves Auto
  against the system (and follows its changes), sets the page's theme
  attribute (`light` or `dark`) and Monaco's theme, and `monacoTheme()`
  names the Monaco theme for editors created later. The page's colours are
  tokens on the root; the dark tokens apply under the system preference
  unless the attribute says light, and always when it says dark. The
  attribute is set from the kept settings before the first paint.
- **Vim module.** Owns Vim mode: `set(enabled)` loads `monaco-vim` the first
  time it is turned on and starts or stops it in every attached editor;
  `attach(editor)` and `detach(editor)` follow the editor groups. One
  status-bar element shows the mode and the command line. Ex commands call
  back into the editor through `{ save, saveAll, close, open }`.
- **monaco-vim 0.4.4**, its UMD build, from the npm package, copied next to
  the page at build time like Monaco (no CDN). The page's AMD loader
  supplies Monaco's API under the name the build asks for. A check before
  this spec found it working with Monaco 0.52: `dd`, `u` and the mode
  display, with no console errors.
- **Keys.** The page's own shortcuts are handled before the editor sees
  them and keep working in Vim mode; every other key in an editor goes to
  Vim. The browser keeps a few keys for itself (Ctrl+W, Ctrl+N, Ctrl+T).
- No change to the language server.

## Testing Decisions

- Browser tests (Playwright, installed Chrome) through the page only, as
  the other editor tests do (prior art: the Settings and diff test files):
  - Vim: turn it on in Settings; `ciw`, `dd`, `u`, a count and `.` change
    the text as Vim does; the mode shows in the status bar; `:w` saves (the
    in-place folder mock); `:e` opens a file; both editors of a split use
    Vim; completion still opens in insert mode; Ctrl+P still opens Go to
    File; turning it off restores plain typing; the setting survives a
    reload; no Vim code is fetched while it is off.
  - Theme: Light and Dark set the page's colours and the editor's theme
    whatever the system says (the test sets the system preference); Auto
    follows the system and its change; the choice survives a reload with no
    frame in the other colours; Reset All returns to Auto.
- Every run: no console errors, no outside requests.
- The existing web suites stay green; the native suites are untouched
  (no server change).

## Out of Scope

- Vim key remapping and a `.vimrc`.
- Vim keys in the diff view, the dialogs and the inputs.
- Further themes (high contrast, custom colours), and a theme for the
  toolbox page.

## Further Notes

Both features are page-only; the turtle-lsp binary and the VS Code
extension do not change.
