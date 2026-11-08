# Schema parity spec — the parser's output gate

The second gate: our **parsed model** against their **interpreted
schema**. Their side: `Compiler.interpretFile(fp)` → the `Database`
JSON (the object dbdiagram renders from), dumped by
`parity/driver/theirs-schema-many.ts`. Our side:
`dbml_ui::dbml::schema_json::to_database_json(&doc, &src)`, dumped by
`examples/parity_schema.rs`. The runner (`parity/run_schema.mjs`)
canonicalizes both (object keys sorted) and deep-diffs with
path-qualified reports.

## Null rule

Their `interpretFile` returns `undefined` while the file has ANY error
(parse or binder) — and so does our side (parse errors, or any
error-severity diagnostic). Both-null is a PASS; one-sided null is a
divergence reported with the blocking errors listed.

## The normalized contract (v2 — token offsets included)

Structural shape per `packages/dbml-parse/src/core/types/schemaJson/types.ts`:
`tables` (fields with `type {schemaName, type_name, args}`, `pk` /
`unique` always boolean, `not_null` only when set (true AND explicit
false), `increment`, `dbdefault`, `note`, `metadata`, `inline_refs`),
`indexes` (columns with `type: "column" | "expression"`, presence-only
`unique` / `pk` / `name` / `type`), `refs` (endpoints with `relation`
strings `"1" | "0..1" | "*" | "0..*"`), `enums`, `tableGroups`,
`aliases`, `records` (values typed by their `getRecordValueType`), the
`project` mirror (empty element arrays + settings), sticky `notes`,
and empty `deps` / `diagramViews` / `externals`.

Ports with quirks preserved:
- types split as their grammar reads them: quotes stripped, args
  space-collapsed (`decimal(10, 2)` → type_name `decimal(10,2)`, args
  `"10,2"`), `int[]` keeps its brackets, dotted names split into
  schemaName/type_name;
- defaults: quoted → string, backticked → expression, numbers → JSON
  numbers, `true`/`false`/`null` → the boolean type with STRING values
  (their quirk);
- 2+ column-level `[pk]` columns merge into a synthetic composite pk
  index — the flags RESET on the fields, the synthetic index carries
  their `{-1,-1}` sentinel token;
- inline refs store as the REVERSED ref: target endpoint first, the
  op's cardinalities swapped; the host endpoint's token is the column
  declaration, the target endpoint's is the `ref:` attribute;
- an empty-string record value in a non-string column reads as NULL.

## Token positions (v2)

Every node carries `token: {start: {offset}, end: {offset}}` — their
`Position` is `{offset, line, column}` over UTF-16 code units, so only
the offset is language-neutral; `line`/`column` and `filepath` drop.
Covered: the whole-document token, tables, columns, indexes (and their
columns), refs and endpoints, enums and values, groups, sticky notes,
records and record values. Known residual (fuzz-only): custom-metadata
entries, enum-value notes, and column checks carry their settings-entry
tokens on their side.
