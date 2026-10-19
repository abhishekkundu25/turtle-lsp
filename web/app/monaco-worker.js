// Monaco's own worker (diffs, links), loaded from the copy next to the page.
self.MonacoEnvironment = { baseUrl: new URL("vendor/monaco/", self.location).href }
importScripts("vendor/monaco/vs/base/worker/workerMain.js")
