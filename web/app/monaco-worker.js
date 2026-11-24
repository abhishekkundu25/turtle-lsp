// Monaco's own workers (diffs, links; JSON's folding and selection), loaded
// from the copy next to the page. The page names each worker by its label.
// The file names carry Monaco's build hash: app-modules.test.mjs checks that
// the build copies them.
importScripts(self.name === "json" ? "vendor/monaco/vs/assets/json.worker-CoJx_OPf.js" : "vendor/monaco/vs/assets/editor.worker-lj3bdIIn.js")
