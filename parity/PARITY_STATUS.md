# Parity status

`node parity/run.mjs` → **78 pass / 0 divergent** against
`holistics/dbml@6b0c25b` (`packages/dbml-parse`, the open compiler).
Full per-case detail: `parity/parity-report.json`. The corpus includes
the user's own files (`desktop-errs/errs2/test`) so that shape class is
gate-covered permanently.

The suite gates the full-fidelity diagnostic contract
(`parity/PARITY_SPEC.md`): type, text, and the FULL SPAN (row, column,
endRow, endColumn at UTF-16 fidelity), category, explanation, and the
quick-fix multiset (title/shortTitle/newText/isPreferred). Both
engines' outputs are compared as sorted multisets; Monaco-style
1-based rows and UTF-16 columns.

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

## The differential-fuzz convergence

`python3 parity/fuzz.py 300 && CORPUS_DIR=... node parity/run.mjs` →
**237/300** and climbing (19 at the tier's birth, 44 → 86 → 137 → 199 →
230 → 237 as the classes land). Every fix is a source read first:

- the ref-shape classifier (their parse NEVER fails on broken bodies —
  the Pratt parser wraps them as application args; validateFields
  classifies) — probe-verified on the g1–g8 matrix, mw3, fuzz0022;
- the binder-class channel (their errors() chains parse + binder
  reports; the interpret layer's infos gate on `shouldInterpretNode`);
- the group grammar (a comma-separated line is a comma-expression
  callee → the form error; a juxtaposed word is an application arg →
  per-juxtaposed-word errors, the membership on the callee only);
- escape-aware quoted identifiers (`raw: false` — `\"` decodes and
  cannot end the token) + escaped value emissions;
- symbol-keyed same-endpoint/pair machinery (duplicates resolve to the
  last declaration; unresolved sides stay out of the pair map).

CONVERGED at message level first, then at SPAN level: corpus 75/75 and
the 300-case fuzz corpus 300/300. The span-parity mission added: their
anchors carry node/token spans everywhere (endpoint fragment spans,
chain fragment ends, the callee/side/arg node spans), their positions
are UTF-16 columns, and the 39-probe span battery lives in
`parity/battery/` — all span-identical. The path there: (1) the records
header ports their tupleExpression exactly (one element, then a
comma-terminated loop; a missing comma errs once at the offending
token and synchronizeTuple skips silently to the next `,`/`)`; the
header column carries its span — their binder anchors the missing-
column error at the `columnBindee`), (2) the ref-body pipeline became
a real port of their grammar: the lexer (operator munching, unicode
identifiers), the Pratt expression parser with their binding powers,
the application wrapper, validateFields (must-be-binary / per-side
Invalid column reference / Unequal / the args walk), and bindFields'
bindee scan feeding structured binder chains resolved per
nodeRefereeOfRefEndpoint (schema/table/column contexts, the
unresolved-left silences), and (3) the TableGroup body parses through
the same application parser with the group binder's one-pop rule and
its own resolver (nodeRefereeOfTableGroupField, the unresolved-left
fall-through to a schema lookup).

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

Token positions joined the contract — table, column, index (columns
included), ref endpoints (inline refs carry their attribute span; the
host endpoint carries the column span), enum values, groups, sticky
notes, records and record values, PLUS the settings-entry tokens:
custom metadata entries, enum-value notes, column checks, and the
table/group `Note:` blocks all carry their attribute spans. Their
Position indexes UTF-16 CODE UNITS, so our byte offsets convert
per-char at emission (a multi-byte char like `ñ` is one unit there,
two bytes here — caught by the fuzz corpus).

CONVERGED: the schema gates are at FULL parity too — corpus 78/78 and
the fuzz corpus 300/300.

## Standing state

Both gates green end to end: diagnostics 78/78 + 300/300 (span-level),
schema 78/78 + 300/300, and the Rust suites all pass. Since this
document's last full pass, the recovery surfaces landed too (the
swallow/gather mechanism, the handler dispatch, the custom-element
general parse, the identifier-stream values, the top-level and
nested-element rules) — see `parity/parity.md` for the standing plan:
the mutation campaign (`parity/mutate.py`, 210/600 and converging) and
the quick-fix edit-range parity are the remaining units. The app
now feeds the FULL diagnostic pipeline (validate_with_parse with the
binder channels) — the editor's panel matches the gate. The strict ref path now runs their application parse over the whole
line when trailing junk follows the clean relationship: word junk
becomes args (one single-binary per comma-joined ARG, their nodeSettings
walk), an op-leading tail REBINDS the callee (`a.b > c.d - e.f` →
infix(`-`) is not isBinaryRelationship → must-be-binary + Unequal + the
callee's chains, while the ref leaves the same-endpoint registry —
their checkStandaloneRefs skip). Their validateFieldSettings runs on
every ref's settings list: delete/update once-only + the FK-action
policies, color/note literals, inactive valueless, custom metadata
value kinds.
