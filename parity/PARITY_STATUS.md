# Parity status

`node parity/run.mjs` → **75 pass / 0 divergent** against
`holistics/dbml@6b0c25b` (`packages/dbml-parse`, the open compiler).
Full per-case detail: `parity/parity-report.json`.

The suite gates the full-fidelity diagnostic contract
(`parity/PARITY_SPEC.md`): type, text, row, category, explanation, and
the quick-fix multiset (title/shortTitle/newText/isPreferred). Both
engines' outputs are compared as sorted multisets; one-based rows
(their provider serves Monaco).

## What the engine now ports (source-read, not output-guessed)

- **`ref/constraint_fixes.ts` → `validate.rs`** — the whole
  `validateCardinality`: per-side runs with the four checks (nullable
  mismatch, required mismatch, unique-when-many, should-be-unique),
  the `otherIsSource` gating (one-to-one checked from the left only),
  `pushDiag` endpoint+declaration anchors with the inline-ref collapse
  (the host column IS the left token), and `suggestChangeOp` computed
  by re-composing the transformed side with the untouched other side —
  which is how `<?` suggests `<>?` and `-` suggests `>` / `<`.
- **`core/types/relation.ts`** — the 16-form operator/cardinality map
  plus the four raw bounds (`1`, `0..1`, `*`, `0..*`) and the
  make-optional/required/many transforms.
- **`core/utils/setting.ts`** — `addSettingEdit` (`, x` into the
  bracket list, ` [x]` at declaration end) and `removeSettingEdit`
  (empty newText; a sole setting takes the brackets with it).
- **`symbols.ts` `nullable()`** — pk flag, auto-increment, membership
  in any pk index, and the `not null` setting all imply NOT NULL.
- **`table/interpret.ts` records + `isColumnsUnique`** — backticked
  two-variant FK messages, the `null` label for empty cells, the
  "Record type errors" category, and the index-scan uniqueness check.
- **Enum duplicates flag EVERY occurrence** (first twin included).
- **Parse-error suppression** — their pipeline surfaces ONLY parse
  errors while any stand; the semantic pass never joins them
  (corpus-verified: no case mixes the two). The app and the harness
  follow the same rule.
- **The column grammar's inline-settings fields** (their
  column-grammar fields): bare `pk`/`unique` words are legal; other
  bare words err ("Inline column settings can only be `pk` or
  `unique`"); a word dragging a `{ … }` block errs twice ("These
  fields must be some inline settings optionally ended with a setting
  list", at the word and the brace) and swallows the balanced block.
  Types are ONE word + args — no multi-word types.
- **TableGroup comma members** report once, anchored at the first
  member.

## The SQL importer's cardinality rule (ported from `dbml-core`)

Their import pipeline (`@dbml/core`): the ANTLR ASTGens build every FK
as optional — FK side `0..*`, referenced side `0..1` — then
`database.ts adjustCardinalitiesFromColumns` tightens the referenced
side to required-one when EVERY FK column is `not_null || pk ||
increment`. The DbmlExporter puts the one side on the left and reads
the operator off the pair: **`?<?`** (nullable FK), **`<?`** (required
FK). MSSQL's table-level and ALTER constraints are the exception —
their ASTGen builds those endpoints as `*`/`1` directly, which the
adjust step can no longer distinguish, so they always render as plain
`<`. The fixture goldens carry upstream's verbatim marks (the old
harness stripped them).

## The schema gate (M-P) — live

`node parity/run_schema.mjs` → **75 pass / 0 divergent** on the corpus,
**148/150** on the fuzz corpus. The parser's OUTPUT is now gated: our
Document renders as the normalized `Database` JSON their
`interpretFile` produces (see `parity/PARITY_SCHEMA_SPEC.md`), and the
runner deep-diffs with path-qualified reports. Ports: the type
splitter, default/metadata value typing (their boolean-"null" quirk),
the record value pipeline (`getRecordValueType` + the sqlTypes
classifier union, the datetime format ladder), the 2+-col-pk merge
into a synthetic composite pk index (flags reset, `{-1,-1}` sentinel
token), inline refs stored as the reversed ref, and the empty-string-
in-a-non-string-column null rule.

Token positions joined the contract as OFFSETS ONLY (their Position is
UTF-16-indexed; the offset is the language-neutral part) — table,
column, index (columns included), ref endpoints (inline refs carry
their attribute span; the host endpoint carries the column span),
enum values, groups, sticky notes, records and record values.
Known residual (fuzz-only): custom-metadata entries, enum-value notes
and column checks carry their settings-entry tokens on their side —
our occurrences track key offsets but not the full `key: value` span
yet.

## Next: the schema gate (M-P)

The parser's OUTPUT is not yet gated — only its errors and the
semantic findings are. `interpretFile` returns the full `Database`
JSON (the object dbdiagram renders from); the plan is a second
runner gate diffing a normalized `Database` per corpus case
(`parity/driver/theirs-schema-many.ts` already dumps their side), plus
a parser-focused corpus section (quoted identifiers, schemas, aliases,
type args, notes, records forms). Token positions join the contract
once structural parity is green.
