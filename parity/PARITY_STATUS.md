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
