# Parity plan — dbml-ui vs the dbdiagram compiler (agent handoff)

The objective, set by the user and non-negotiable: **complete parity with
the upstream dbml-parse compiler** — diagnostics (message + type + FULL
span row/column/endRow/endColumn), diagnostic count, quick fixes, squiggle
rendering — with NO corners cut. Every claim of "complete" must be scoped
to what was probed; the probe surface grows mechanically via the campaign
tools below. Never say done when the sweep still finds shapes.

## Environment

- Repo: `/home/boilingsoup/Dev/dbml-ui` (pushes/tags happen ONLY on the
  user's command; the fuzz + full suite run at the user's pre-tag pass).
- Upstream sources: `parity/upstream/dbml/packages/dbml-parse/src` — READ
  them when reconstructing behavior; guessing diverges.
- All verification runs happen under `ulimit -v 2000000` + `timeout`. A
  parser loop that allocates forever has killed the user's machine once;
  the caps are mandatory for any run parsing adversarial input.
- Builds: release profile = thin LTO + 16 codegen units + incremental
  (2m21s -> 37s app rebuilds). Use `cargo check` for compile gates; build
  the app binary only when the user will run it.
- `parity/driver/*.ts`: `theirs-dump-many.ts <cases.json>` (their
  diagnostics), `theirs-schema-many.ts` (their model), `rowsprobe.ts` /
  `tokprobe.ts` / `elemprobe.ts` (AST/token structure probes). Run via
  `npx tsx` from `parity/driver/`.
- `examples/parity_dump.rs` = OUR wire contract: JSON diagnostics with
  **1-based rows, UTF-16 columns** (their Monaco positions count UTF-16
  code units; ours are bytes — convert per line), quickFixes as
  {title, shortTitle, isPreferred, newText}.

## The gates (all must stay green; run under caps for the dumps)

- `node parity/run.mjs` — diagnostics parity, 78 corpus cases
  (includes the user's own files: desktop-errs/errs2/test — DO NOT let
  these regress).
- `node parity/run_schema.mjs` — model parity (needs
  `cargo build --release --example parity_schema`).
- `cargo test --lib -- dbml::parser dbml::validate` (42) and
  `cargo test --bin dbml-ui -- editor::tests` (61).
- The span battery: `parity/battery/*.dbml` (39 probes, the c*/d*
  families — PERSISTENT copies; /tmp is volatile). Compare with:
  `./target/release/examples/parity_dump <files> > ours.json`, then their
  engine via a cases.json, then diff the FULL span tuples:
  (type, text, row, column, endRow, endColumn).

## Hard-won rules of their compiler (do not re-derive)

- Their anchors carry NODE/TOKEN spans, never points: the missing-table
  diagnostic anchors the table TOKEN, missing-column the column token,
  shape errors the callee/side/arg nodes, duplicates the full column
  node, etc. Endpoint/BinderChain/SettingOcc/Column all carry fragment
  spans now.
- Their positions are UTF-16 code units.
- The lexer's `gatherInvalid` REMOVES invalid tokens and they take their
  trailing trivia (the newline) with them — so the enclosing application
  keeps gathering across the line boundary (the "swallow"). This is the
  mechanism behind most of the recovery shapes. Our port: line-chain
  while the tail stays dropped (`swallow_gather` / the records, group,
  ref-block loops). The gathered token stream MUST end with an Eof token
  (advance() clamps; without it the application loop spins forever —
  the machine-killer).
- `consume(message, kind)` LOGS the message then throws
  PartialParsingError. `findHandlerContext` picks the handler by the
  nearest `,`/`]`/`}`: the list handles `,`/`]`; a `}` hands to the
  enclosing BLOCK (the block closes, the field dies, partial_occs ride
  back); no closer = the current context. Ported as
  `list_brace_dispatch` + the empty-message Fail sentinel.
- `attributeValue`: IDENT IDENT-led values parse as an identifier
  STREAM (stops at `:`/`,`/`]`; a `}` is JUNK to it → 'Expect an
  identifier' + the handler scan), other values = the Pratt expression.
- `EmptyNode` spans = the prevToken's END point; their operand-start
  errors quote the token VALUE (`Invalid start of operand "}"`) — the
  punctuation tokens carry literal values.
- ANY identifier is an element type: unknown types bind as custom
  elements (their CustomElementValidator: context/name/alias/settings/
  block-body/non-string-value checks) with
  `synchronizeElementDeclarationName` recovery. A typo'd `Tabl oops {`
  = a custom element whose block swallows the rest — three binder
  errors, no parse errors, the nested table never binds.
- Their interpret gate: any parse/binder error -> `database: null` ->
  the records never interpret. Our app gates the record icons/panel on
  blocking errors (app.rs sync).
- consume_one_token advances WHOLE UTF-8 sequences (one byte left
  mid-character offsets that crashed the dump).
- `records (a b)` tuples: one element, comma-terminated loop, the
  missing comma errs once at the offending TOKEN (token width), the
  sync skips the rest silently.
- The program-level junk: 'Expect an identifier' on the junk token
  (+ 'Unexpected EOF' at EOF), the sync advances ONE token.

## State (as of the a5fb039 commits)

- Grammar-shaped fuzz corpus (parity/fuzz, 300 cases): **300/300
  span-identical**.
- The span battery (39 probes): **all span-identical**.
- Corpus gates: 78/78 + 78/78. Focused tests 42/42 + editor 61/61.
- The user's three desktop files: span-identical, folded into the corpus.

## Remaining work, in order

1. **The mutation campaign convergence** (the standing tool:
   `parity/mutate.py N SEED` writes mutated cases to /tmp/mut; run ours
   in 100-file batches under caps, their engine in one tsx pass, diff at
   span fidelity, cluster by message). State: 210/600 — the remaining
   390 diverge on the structural-recovery cascade: mutations that sever
   braces make element boundaries resolve differently (their general
   element parse: settings lists containing glued braces, elements
   continuing across lines via discardUntil's colon/brace stop, the
   element-vs-column disambiguation inside bodies). Port their
   general-element recovery mechanism by mechanism; after each fix
   promote the reproducer into parity/battery/ and re-run. Iterate until
   a full campaign round adds zero new mechanisms.
2. **Quick-fix EDIT-range parity**: their quickFixes carry edits
   [{start, end, newText}] in UTF-16 offsets; our QuickFix has no ranges
   and the dump emits only the joined text. Add the edit ranges to our
   QuickFix (byte offsets internally), emit them converted to UTF-16,
   extend the comparison, port the divergences.
3. **Keep the batteries honest**: any new divergence class found anywhere
   (user reports included) becomes a battery case + a corpus case when
   user-shaped, permanently.

## Workflow rules

- Builds are the user's hardware. Use `cargo check` (or
  `cargo check --release`) as the compile gate after every edit — it
  skips codegen and is many times faster than any build. Only BUILD what
  the current step actually runs:
    - `cargo test --lib ...` builds the test profile (required for the
      focused tests; ~10s).
    - `cargo build --release --example parity_dump` (and parity_schema)
      only when dumping/comparing — ~20s each, cached.
    - `cargo build --release` (the app binary) ONLY when the user will
      run the app — never as a routine verification step.
- NEVER declare the port complete beyond what the sweeps prove; state
  the probe surface covered and what the tools will find next.
- Every run parsing adversarial input: memory caps + timeouts.
- The user's machine: builds are 37s incremental now — use them freely,
  but full clean rebuilds are rare events (profile change only).
- Their engine's real behavior beats source-reading guesses: probe first
  (the driver tools), then port.
