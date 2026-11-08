# Parity status — after the first full run

`node parity/run.mjs` → **2 pass / 67 divergent** against
`holistics/dbml@6b0c25b` (`packages/dbml-parse`, the open compiler).
Full per-case detail: `parity/parity-report.json`.

## Class 1 — their-parser-gap: 47 cases

Their new compiler (dbml-parse master) REJECTS inline `ref: > ...`
column settings ("Inline column settings can only be `pk` or
`unique`") and misparses some standalone `Ref:` forms — every corpus
case using those syntaxes produces THEIR parse errors instead of
semantic diagnostics. No comparison is possible against an engine that
cannot parse the input; these cases stay in the corpus and flip green
automatically when their compiler matures. (dbdiagram.io production
clearly runs a more complete engine — the user-visible divergence
there is smaller.)

## Class 2 — semantic: 20 cases (the port list)

Genuine divergences, each portable against their open source:

- **The reported case** (`reported-index-pk`): ours 2 warnings, theirs
  0 — their `isColumnsUnique` scans the table's interpreted indexes for
  ALL cardinalities; our single-column branch reads column settings
  only. (Fix designed; see the conversation.)
- **Records constraints** (`records-fk-*`, `records-null-in-required`):
  severity (theirs `info`, ours `warning`), message phrasing ("must not
  be NULL" vs "does not exist in"), count, and their extra quick fixes
  ("Change to >?", "Add not null") — port their
  `ref/constraint_fixes.ts` + records interpretation.
- **Missing-endpoint phrasing**: they qualify the schema
  ("does not exist in Schema 'public'", "'public.b'") — port the
  schema-qualified phrasing (our parser has no schema model; decide:
  hardcode `public` or add schemas).
- **Duplicate members**: `dup-enum`, `enum-duplicate-member` — their
  engine's duplicate handling differs; check their enum/table bind.
- **Row offsets**: several cases are row-off-by-N (their anchors point
  at different constructs) — the spec compares rows; where their anchor
  row differs by convention, the port aligns ours to theirs.

## Path to green

1. Port the semantic divergences (the 20) — each: read their source
   module, match message/severity/quickfixes/anchor rows, add the
   regression test.
2. Re-run until green.
3. Re-point the class-1 cases when their compiler matures (the suite
   flips them automatically).


## Their engine's semantics, read from the open source (the port reference)

All fetched from `packages/dbml-parse/src/core/global_modules/` at the pin:

- **`ref/constraint_fixes.ts: isColumnsUnique`** — single-column fast path checks
  ONLY the `unique` setting; then an index scan over the table's
  `mergedIndexes` accepting any `unique || pk` index whose columns match the
  ref's column set EXACTLY — for ALL cardinalities. Our single-column branch
  lacks the index scan: the reported divergence.
- **`table/interpret.ts`** — 2+ column-level `[pk]` columns are MERGED into a
  synthetic composite pk index and the column flags RESET; a single
  column-level `[pk]` stays a column flag (their `isColumnsUnique` fast path
  does not check it — the index scan is what covers single column-pks...
  it only covers them if the interpreter also synthesizes an index for
  single pks: unresolved, needs their schema normalization reading).
- **Records FK messages** — two variants, backticked, severity `info`:
  "`FK violation: `l` = `v` must not be NULL`" (null/empty values) and
  "`FK violation: `l` = `v` does not exist in `u``" (missing values). Ours:
  unbackticked, severity `warning`, one variant.
- **Nullable-operator** — severity `info` (ours `warning`), quick fixes
  `Change to >?`/`<?` (relax the operator) and `Add not null`/`Add unique`
  (tighten the column). Ours: no quick fixes on this check.
- **Missing endpoints** — schema-qualified: "does not exist in Table
  'public.b'", "in Schema 'public'". Ours: unqualified.
- **Duplicate names** — "already exists in schema 'public'", anchored at the
  duplicate declaration's END offset (row 2 for a 2-line file); ours anchors
  the name (row 1).
- **`Invalid column type`** — their `<` ref with a column-setting pk errors
  with this; the SAME schema with the pk as an index produces no diagnostic.
  Their `<` operator semantics need `ref/interpret.ts` reading before
  porting (possible expression-type-checking or inverted direction).
- **Composite refs** — their engine warns on `parent.(id,code) >
  child.(parent_id,code)` when the CHILD lacks the composite key, with the
  `Change to <>` quick fix (making it many-to-many removes the uniqueness
  requirement). Our findings agree; only severity/quickfixes differ.
