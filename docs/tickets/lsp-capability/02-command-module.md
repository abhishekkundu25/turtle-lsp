# 02: Command module

**What to build:** a `commands` module that lists every command the server advertises and parses `ExecuteCommandParams` into a typed enum (today: fetch an import). The server's `workspace/executeCommand` arm becomes "parse, then act". Same error codes and messages as now.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] The advertised command list comes from the module.
- [ ] Unit tests for parsing: good arguments, wrong arity, unknown command.
- [ ] Existing executeCommand tests (test/*.test.mjs) pass unchanged.
- [ ] Keystroke rows of bench/perf.mjs unchanged.
