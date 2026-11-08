# Diagnostic parity spec — dbml-ui vs dbdiagram's open compiler

The reference engine: `holistics/dbml` @ the commit in `parity/upstream-pin.txt`,
package `@dbml/parse` (`packages/dbml-parse/src`), driven headlessly by
`parity/driver/theirs-dump-many.ts`. Our engine: `src/dbml/validate.rs`, driven by
`examples/parity_dump.rs`. Both emit the same normalized JSON for a given `.dbml`
input; the runner (`parity/run.mjs`) diffs them per case.

## The normalized diagnostic

Both engines emit, for every diagnostic:

| field      | ours → normalized                                  | theirs → normalized            |
|------------|----------------------------------------------------|--------------------------------|
| `type`     | `Severity::Error` → `"error"`, `Warning` → `"warning"` | `type` verbatim (`error`/`warning`/`info`) |
| `text`     | `message` verbatim                                 | `text` verbatim                |
| `row`      | byte offset → 0-based line of the source           | `startRow` (already 0-based)   |
| `quickFixes` | `short_title` + `new_text` per fix               | `shortTitle` + each edit's `newText` |

Parse errors from both engines are diagnostics too (`type: "error"`, no quick fixes).

## Comparison contract (what the runner gates on)

Per corpus case, both diagnostic lists are compared as **multisets of
`(type, text, row)`** — order-free, multiplicity-significant (a finding anchored
twice must be anchored twice in both engines). Any difference fails the case.

Per diagnostic, the **quick-fix multisets of `(shortTitle, newText)`** must match
exactly. A missing quick fix, an extra one, or a different edit text fails the
case.

## Recorded, not gated (yet)

- `category`, `explanation` (multi-line), the full quick-fix `title`, and the
  column within the row — dumped into the report for the porting work. Their
  phrasings differ from ours in places (the new compiler rephrased explanations);
  each mismatch becomes a port when explanations are promoted to gated.

## Known intentional gaps (spec v1)

- **Column parity inside a row is not compared**: the two engines anchor at
  different constructs (our byte-offset spans vs their token spans), so columns
  can differ legitimately while pointing at the same finding. The ROW must match.
- **Explanations are recorded but not gated** until the phrasing port lands.

## Running

```sh
parity/setup.sh                       # clone + build the pinned upstream
cargo build --release --example parity_dump
node parity/run.mjs                   # full corpus run, exits 1 on divergence
```
