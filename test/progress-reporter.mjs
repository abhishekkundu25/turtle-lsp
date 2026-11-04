// A node:test reporter that appends a line to $TEST_PROGRESS_LOG as each test
// starts and ends, with the time and its file. The spec reporter prints a
// file's results only when the file ends, so a run stopped by a CI step
// timeout names no test; the tail of this log names the one that hung.
// Written synchronously, so `--test-force-exit` or a kill loses nothing.
//
//   TEST_PROGRESS_LOG=progress.log node --test --test-reporter=spec --test-reporter-destination=stdout \
//     --test-reporter=./test/progress-reporter.mjs --test-reporter-destination=stdout "web/test/*.test.mjs"
import fs from "node:fs"

export default async function* progressReporter(source) {
  const log = process.env.TEST_PROGRESS_LOG
  for await (const event of source) {
    if (!log) continue
    const verb = { "test:dequeue": "start", "test:pass": "pass", "test:fail": "FAIL" }[event.type]
    if (!verb) continue
    const file = event.data.file?.split(/[\\/]/).pop() ?? ""
    fs.appendFileSync(log, `${new Date().toISOString().slice(11, 23)} ${verb} ${file} :: ${event.data.name}\n`)
  }
}
