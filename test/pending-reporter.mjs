// A node:test reporter that counts pending (`todo`) tests by tag: the todo
// text's first word (`jsonld:phase2`). Pending tests that pass are
// listed by name and fail the run (`process.exitCode = 1`): promote them by
// removing their `todo`.
//
// The summary is written when the event stream ends, which never happens under
// `--test-force-exit` (the process exits first), so `npm run test:node` does
// not use that flag.
//
//   node --test --test-reporter=spec --test-reporter-destination=stdout \
//     --test-reporter=./test/pending-reporter.mjs --test-reporter-destination=stdout "test/*.test.mjs"
export default async function* pendingReporter(source) {
  const tags = new Map()
  const passing = []
  for await (const event of source) {
    if (event.type !== "test:pass" && event.type !== "test:fail") continue
    const { todo, name, details } = event.data
    if (!todo || details?.type === "suite") continue
    const text = typeof todo === "string" ? todo : "(untagged)"
    const tag = text.split(/\s/)[0]
    const entry = tags.get(tag) ?? { total: 0, passing: 0 }
    entry.total++
    if (event.type === "test:pass") {
      entry.passing++
      passing.push(`${tag}: ${name}`)
    }
    tags.set(tag, entry)
  }
  if (tags.size === 0) return
  let out = "\npending tests by tag:\n"
  for (const [tag, e] of [...tags].sort(([a], [b]) => a.localeCompare(b))) {
    out += `  ${tag}: ${e.total}${e.passing ? `, ${e.passing} passing` : ""}\n`
  }
  if (passing.length) {
    out += `pending tests that pass, to promote:\n${passing.map((p) => `  ${p}\n`).join("")}`
    process.exitCode = 1
  }
  yield out
}
