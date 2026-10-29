## Problem Statement

`turtle-lsp.json` holds a project's settings for every editor, the command line, coding agents and CI. Today it gives the people who write it no help:

- A misspelled key does nothing and says nothing. `"lint": { "undescribedTerm": false }` (missing the `s`) is silently ignored, and the lint stays on.
- A value of the wrong type or outside the allowed set is also silently ignored. Examples are `"lineWidth": "80"` (a string) and `"unresolved": "skip"` (not one of `soften` or `ignore`). The setting keeps its default, and the user doesn't know why.
- The only things reported are the keys the file may not set, such as `imports.network`. That is one message in the server log or on `check`'s stderr, with no position, which is easy to miss in an editor.
- Nothing tells a user which keys exist, what they do or what values they take, except the README. There is no completion and no hover documentation in the file.
- A file that isn't valid JSON is reported once, as a message, not at the place where it breaks.

## Solution

turtle-lsp checks `turtle-lsp.json` itself, the way it checks Turtle files:

- Unknown keys, values of the wrong type, values outside the allowed set, unknown diagnostic codes in lists of codes, and keys the file may not set become warnings on the exact key or value in the file. They show in the Problems panel of every editor turtle-lsp runs in, with no setup.
- A file that isn't valid JSON gets one error at the place it breaks.
- `turtle-lsp check` prints the same warnings, so CI notices a broken config.
- VS Code users also get completion and hover documentation in the file, from a JSON Schema the extension ships and registers. Users of other editors can point a `"$schema"` key at the same schema, and turtle-lsp accepts that key without complaint.

The settings still apply exactly as before. A bad entry is still skipped, so a bad setting never stops the server, but now it is reported where it is.

## User Stories

1. As a user, I want a warning on a misspelled key in `turtle-lsp.json`, so that a typo doesn't silently do nothing.
2. As a user, I want the warning placed on the key itself, so that I can see which one is wrong.
3. As a user, I want the warning to suggest the closest known key, so that I can fix a typo without opening the README.
4. As a user, I want a warning when a value has the wrong type, so that I know the setting isn't taking effect.
5. As a user, I want the type the key expects named in the message, so that I know what to write.
6. As a user, I want a warning when a value isn't one of the allowed choices, with the choices listed, so that I can pick a valid one.
7. As a user, I want a warning when a number is out of range (a negative timeout, a line width of zero), so that I know it was not applied.
8. As a user, I want a warning on an unknown diagnostic code in a list of codes, so that a misspelled check name is caught.
9. As a user, I want a warning on a key the file may not set, such as `imports.network`, so that I see why it was refused.
10. As a user, I want that warning to say where the setting can be set instead (editor settings or a command-line flag), so that I can still turn it on.
11. As a user, I want a warning when the file raises a limit past its default, saying the file may only lower it, so that I understand the cap.
12. As a user, I want a file that isn't valid JSON to get one error at the place it breaks, so that I can fix the syntax.
13. As a user, I want a file that is valid JSON but not an object to get one error, so that I know the file's shape is wrong.
14. As a user, I want the dotted form (`"lint.undescribedTerms": false`) and the nested form checked the same way, so that both styles get the same help.
15. As a user, I want the optional top-level `"turtle"` wrapper understood, so that a wrapped file isn't reported as one unknown key.
16. As a user, I want the `prefixes` map's entries checked as prefix names and IRIs, so that a broken prefix is caught here, not later in every file.
17. As a user, I want a top-level `"$schema"` key accepted without a warning, so that adding it for my editor creates no noise.
18. As a Neovim user, I want all these warnings with no setup, so that I get the same help as a VS Code user.
19. As a browser-editor user, I want the same warnings when my workspace has a `turtle-lsp.json`, so that the browser behaves like the desktop.
20. As a VS Code user, I want completion of keys and values in `turtle-lsp.json`, so that I can discover what's configurable.
21. As a VS Code user, I want hover documentation on each key, including its default, so that I know what it does.
22. As a user of another editor with JSON schema support, I want a URL I can put in `"$schema"`, so that my editor gives completion too.
23. As a user, I want the warnings to update as I type in the file, so that I see a fix take effect.
24. As a user, I want a corrected setting to take effect as soon as the file is saved, as it does today, so that checking adds no new step.
25. As a CI maintainer, I want `turtle-lsp check` to print the settings warnings, so that a broken config is noticed in the build.
26. As a CI maintainer, I want them printed in the same `path:line:col: severity[code] message` form as other problems, so that GitHub annotations and SARIF can show them.
27. As a CI maintainer, I want settings warnings to leave the exit code alone unless `--max-warnings` counts them, so that upgrading doesn't break CI.
28. As a CI maintainer, I want a settings file that can't be parsed to fail `check` as it does today, so that a broken file is never silently skipped.
29. As a user, I want warnings only for the `turtle-lsp.json` at the workspace root, so that a file of that name elsewhere (a fixture, a vendored project) isn't checked.
30. As a user, I want other JSON files left alone, so that turtle-lsp doesn't comment on `package.json` or my data.
31. As an existing user with a valid file, I want no new warnings, so that upgrading is quiet.
32. As a maintainer of turtle-lsp, I want one description of every setting in the server, so that applying, checking and documenting settings can't disagree.
33. As a maintainer of turtle-lsp, I want a test to fail when a setting or a diagnostic code is missing from the schema, or when the schema lists one the server doesn't read, so that the schema never drifts.
34. As a maintainer, I want new settings added in one place, so that each new setting gets checking, completion and documentation for free.

## Implementation Decisions

- **One settings table.** The server keeps one table of every setting it reads, with each setting's key, type (boolean, integer, number, string, string choice, list of codes, prefix map), allowed values or range, default, a one-line description, and whether the settings file may set it (or only lower it). Applying settings walks this table instead of its own match on paths. The file checks and the schema test read the same table. Applying a valid file gives exactly the settings it gives today.
- **The check is a pure function.** It takes the settings file's text and returns diagnostics with ranges: unknown key, wrong type, value not allowed, out of range, unknown code, refused key, raised limit, invalid JSON and not an object. It uses the existing JSON parser with positions, the one the JSON-LD support uses. Unknown keys get a "did you mean" from the existing bounded edit distance. Diagnostics use a new code, `settings`, as warnings. Invalid JSON keeps `json-syntax`, as an error.
- **The settings file is a document kind.** The workspace's single file-name table gains the settings file, matched only at a workspace root. The server publishes the check's diagnostics for it when it is open, and with workspace diagnostics on. Other JSON files stay unrecognised.
- **Editors attach to it.** The VS Code extension adds the file name to its document selector. The Neovim plugin attaches the server to a buffer of that name at the workspace root. The browser editor sends it like any other workspace file.
- **Settings take effect as today.** The server still re-reads the file on save, and on a change notification when the editor reports file changes. Checking an open, unsaved buffer doesn't change the settings in force.
- **Restricted keys move into the table.** The rule that the file can't turn on the network or raise a limit is expressed in the table. Its message becomes a diagnostic on the key, and it still goes to the log once, so nothing is lost for clients that don't open the file.
- **`check` prints them.** When `check` reads the settings file, it runs the same function and prints each diagnostic in the existing text, JSON, GitHub and SARIF forms, under the settings file's path. The exit code follows the existing rules: warnings count only toward `--max-warnings`, and a file that can't be parsed still exits 2.
- **`$schema`.** A top-level `$schema` key is accepted and ignored.
- **JSON Schema.** The VS Code extension ships a hand-written JSON Schema (draft 2020-12) for the settings file, with each key's description and default. It registers the schema for the file name through the editor's JSON validation contribution, so completion and hover come from VS Code's JSON support with no server code. The same file, served from the repository, is the URL documented for `"$schema"`. The schema allows the nested form, the dotted form and the `"turtle"` wrapper.

## Testing Decisions

- A good test gives settings text and asserts what the user sees: the diagnostics' codes, ranges, severities and messages, or the settings in force. It doesn't assert on the table's internals.
- **Highest seam: the settings module, given JSON text.** Unit tests cover:
  - each diagnostic kind: unknown key (with and without a suggestion), wrong type, value not allowed, out of range, unknown code, refused key, raised limit, invalid JSON, not an object;
  - the dotted, nested and wrapped forms;
  - `$schema` accepted;
  - a valid file giving no diagnostics.

  They also check that the settings applied from every existing test file are unchanged. Prior art: the existing unit tests in the settings module, such as the one where later layers override and bad values are ignored, and the one for keys the file may not set.
- **Schema drift.** A test reads the extension's schema and compares it with the settings table and the known diagnostic codes: every setting is present with a matching type, default and allowed values; the code lists match; and the schema has no key the server doesn't read. Prior art: the test that checks the watcher glob and the file-name table agree.
- **LSP end to end.** The settings-file Node tests open `turtle-lsp.json` and assert the published diagnostics and their positions, a fix while typing clearing them, and no diagnostics for a `turtle-lsp.json` outside the root or for another JSON file. Prior art: the existing settings-file Node tests.
- **CLI.** CLI tests cover the warnings in each output format, the exit code with and without `--max-warnings`, and exit 2 for a file that can't be parsed. Prior art: the CLI test file's settings-file cases.
- **VS Code.** An extension test checks that the schema is contributed for the file name. Prior art: the existing extension manifest tests.

## Out of Scope

- Completion and hover inside the settings file from the server itself. VS Code gets them from the schema; other editors can use `$schema`.
- Quick fixes in the settings file (for example, replacing a misspelled key).
- Checking settings files outside the workspace root, and settings from editor configuration or `init_options`.
- Listing the schema on SchemaStore, and publishing it on the Pages site.
- A generated schema. It is written by hand and kept honest by the drift test.
- New settings. The lint settings (`lint.ignore`, `lint.enable`) are in the label-lints spec and join the table when that work lands.

## Further Notes

- This is how other tools treat their config files. TypeScript reports unknown compiler options as errors inside `tsconfig.json`, clangd reports errors inside `.clangd` files, and VS Code gives completion for both from a schema. Biome, Deno and lua-language-server ship a schema and accept `$schema`.
- The label-lints spec depends on this one only loosely. Its new keys go into the table, and unknown codes in `lint.ignore` and `lint.enable` are reported by this check.
