# 05: Formatter: one pass reaches the fixed point (class C)

**What to build:** In src/layout/doc.rs, a way to build the token document with a set of token indexes treated as absent from comment attachment (a second constructor such as `Doc::without(src, all, omit)` or the attachment loop as a method run with an omit set): an omitted token gets no lead and no trail and does not advance `prev_end` for token gaps, so the gap for the next token is measured from the previous kept token; `lead_pref` and `trail_pref` are rebuilt. Two details the review found: `own_line` for a comment after an omitted token is decided from the gap between that token and the comment (so `<o>\n ; # c\n .` still prints `<o> . # c` and does not leave `.` alone on a line), and `blank` counts whitespace only (so `<o>\n;\n# c\n.` does not gain a blank line the user never wrote). In src/features/format.rs, after the CST is built, collect the redundant semicolons (`semis[1..]` of every `Po`, and all `semis` of the last `Po` of each list: top level, `[ ]`, `{| |}`, graph blocks) and, when any exist, rebuild the doc with them omitted. In src/layout/printer.rs, `Semicolons::Needed` writes only the first `;` between two pairs and never the others; delete `extra_semi` and update the enum doc and the module doc line in format.rs. `Semicolons::AsWritten` is unchanged. The exemption and the pending test in tests/chaos.rs are deleted by ticket 09, which is blocked on this one.

**Blocked by:** nothing.

**Issue:** #17. **Wave:** 1. **Files (lock in bold):** src/layout/doc.rs; src/layout/printer.rs; src/features/format.rs; tests/format.rs; test/format.test.mjs

**Status:** proposed

- [ ] tests/format.rs `a_redundant_semicolon_with_a_comment_is_dropped_in_one_pass`: exact output and an unchanged second format for `<a> <p> <o> ; # c\n.\n` (expected `<a> <p> <o> . # c\n`), `<a> <p> <o> ;\n  # c\n  ;\n.\n`, `<a> <p> <o> # c1\n ; # c2\n .\n`, `<a> <p> [ <q> <r> ; # c\n ] .\n`, `<a> <p> <o> ; # c1\n ; # c2\n <q> <r> .\n`, TriG `<g> { <a> <p> <o> ; # c\n }\n`, an annotation `{| <q> <r> ; # c\n |}`, `<o>\n;\n# c\n.` (no blank line invented), `<o>\n ; # c\n .` (gives `<o> . # c`), and a file with two `; # c\n.` statements (the 195134 shape).
- [ ] src/layout/doc.rs test `an_omitted_token_hands_its_comments_to_its_neighbours`: `<o> ; # c\n.` with `;` omitted trails `<o>`; `<o> # c1\n; # c2\n.` has c2 leading `.`; an own-line comment above an omitted `;` leads the next token; the two shapes above.
- [ ] `tests/format.rs:298-301` (a needed `;` with `# second`) unchanged; every golden in test/format.test.mjs and the formatter fixtures containing `; #` updated and listed in the report.
- [ ] `same_graph` still holds on every formatter test (comment count unchanged).
- [ ] CHANGELOG note drafted for ticket 13: `format --check` now fails on files with `; # c` before `]`, `}` or `.` that were stable before.
- [ ] Replays of 195134, 123532 and 49915 pass (`CHAOS_ONLY`, under `timeout 120`; before ticket 09 lands, by formatting the saved files twice with the CLI).
