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

## State (as of the campaign round 2)

- Grammar-shaped fuzz corpus (parity/fuzz, 300 cases): **300/300
  span-identical**.
- The span battery (56 probes — c*/d*/e* families): **all
  span-identical** (compare with ABSOLUTE paths in the cases.json — the
  driver resolves `path` from cwd).
- Corpus gates: 78/78 + 78/78. Focused tests 42/42 + editor 61/61.
- The user's three desktop files: span-identical, folded into the corpus.
- The mutation campaign (round 10, seed 43): **278/600 span-identical** —
  rounds 2-9 landed (the mechanisms below).

## Round-2 mechanisms (ported; do not re-derive)

- Their operand() at EOF logs 'Unexpected EOF' on the EOF token (a
  zero-width point at the SOURCE end) and returns an EmptyNode WITHOUT
  throwing — the junk case errs 'Invalid start of operand "value"' on
  the junk token's full span. leftExpression_bp then throws.
- Their discardUntil errs on the CURRENT token's FULL span (the EOF
  token at EOF) and fires even when nothing follows; returning false
  means no body parses.
- canHandle(e) = handlerContext === undefined || handlerContext ===
  top — an undefined handler is handled by the CURRENT context (a
  top-level list swallows to EOF itself: 'Expect a closing bracket ']''
  at the EOF point, the list RETURNS NORMALLY — closed=false — and the
  enclosing element still runs discardUntil).
- Their node spans = the UNION OF THE ARG SPANS (nodeFactory): swallowed
  junk (markInvalid'ed) rides in NO arg — the settings list/element/
  block spans end at the last REAL token (custom_element real_end; the
  unclosed block body = the open brace + the last real token; their
  blockExpression close consume errs 'Expect a closing brace '}' at the
  EOF point when unclosed).
- convertFuncAppToElem: a table-body field line converts to a nested
  ELEMENT when the LAST arg is a block and the remaining args (the type
  included) count 0 or 1 (3 needs `as`) — the element = {type: the
  CALLEE (= the column name), name: the next arg, attributeList: a
  trailing list, body: the block} and the CustomElementValidator runs
  where it stands. Otherwise the field path: remains = args[1..];
  remains[:-1] all identifiers + last identifier-or-list, else 'These
  fields must be...' PER PART (each part's own span); a trailing list
  pops as the setting list; the words set pk/unique or err 'Inline
  column settings...' per word.
- Their FK-lint infos anchor the RESOLVED COLUMN NODE (name .. the
  settings end — Column.full_end), not name..type-end.
- Their project fields: `key: value` lines are fine (the custom-element
  context check ALLOWS a Project parent — `note: 'x'` binds silently);
  an application field errs 'A Project can not have inline fields' on
  the application node (the union of its tokens); `word { … }` /
  `word word { … }` converts to a nested element (only the name/
  inline-field checks fire); nested keyword elements validate where
  they stand ('An Enum can only appear top-level' + EMPTY_ENUM, 'Table
  must appear top-level', ...) with the element span = the keyword ..
  the last real token (last_real_token_end). Their enum's unclosed
  block errs 'Expect a closing brace '}' at the EOF point.

## Remaining work, in order

1. **The mutation campaign convergence** (the standing tool:
   `parity/mutate.py N SEED`; compare at span fidelity, cluster by the
   message-delta signature). State: 242/600 (seed 43) — the remaining
   358 spread over a long tail: the column-name/type junk cascade
   ('A column must have a type' + the name error, our 'expected column
   type' extra), the dot-expression cascade, 'Expect a closing brace '}'
   / 'Unexpected EOF' gaps, the 'default' value cascade ('EOF reached
   while parsing'), our invented messages on secondary subsystems
   (indexes block, note body, enum header), 'A Ref must appear
   top-level' vs 'Table must appear top-level' dispatch, and pure
   span-only cases. Iterate: cluster -> probe their engine on the
   minimal shape -> port -> promote the reproducer into
   parity/battery/ -> gates. Iterate until a full round adds zero new
   mechanisms.
2. **Round-3 findings so far** (probed, ported):
   - "'not null' must not have a value" anchors the VALUE node (the
     key when the value never parsed).
   - the table-loop's nested-element context error anchors the element
     node = the keyword .. the last REAL token (last_real_token_end on
     the captured region — the same rule as the project loop).
   - the member-expression dot binds ACROSS spaces in the type
     (`user . x` = one member node — probe t_dot: ZERO diagnostics);
     type_str's dot chain sp-binds but restores the un-spaced input on
     a failed dot so the glued `int[2]` array rule still holds
   - `Enum` at EOF: the name = the operand at EOF — 'Unexpected EOF' +
     the brace error + 'An Enum name must be of the form <enum> or
     <schema>.<enum>' ALL at the EOF point, no body (probe t_enumhdr)
   - the broken-string default: a `'`-led default whose token dies at a
     newline routes to the plain-value arm's broken-string machinery
     (default_string_is_broken gates it so backtick/bare defaults keep
     their verbatim arm — probe t_default PASS)
   - `Note {` in a table body = the nested Note element: 'A Note must
     have a content' on the element node + the string's newline error
     (probe t_note PASS)
   - their validateColumn (probes u1/u2/u3 PASS): args=[] errs 'A
     column must have a type' at the CALLEE; a non-variable callee errs
     'A column name must be...' at the CALLEE — a `(` at the type
     position binds to the callee (the call expression `decimal(10,2)`
     alone = the broken callee, both errors at name..paren-end); a `(`
     at the NAME position = the tuple-as-callee (no operand error, the
     pair at the tuple node); the duplicate-column anchor = the column
     NODE (name_span..full_end — a type-less column's node = the name)
   - the failed-expression body child: the operand error's EmptyNode
     (prevToken = the junk token) rides the body as a broken column —
     BOTH validateColumn errors at the junk's END point, riding the
     parse-error channel (the Err would drop the binder vec — probe
     m43_0096 PASS)
   - their attribute node = the union of its args: the SettingOcc's
     end = max(value_span.end, key_end) — the trivia ws the list
     crossed stays out (probe m43_0251 PASS: the ref value ends at
     `nope.n`, not at the swallowed next line); the ref arm's value
     span ends at the endpoint (its optional settings bracket), not
     after the trailing ws
   - the indexes-block tail (m43_0216/0402/0484 PASS): the unclosed
     indexes block errs the indexes' OWN close error at the EOF point +
     the table's (two total) — our invented 'unterminated indexes
     block' replaced; the junk FIELD inside indexes errs their
     IndexesValidator field message on the TOKEN span (probe w3), and
     the arm's Err push now preserves e.end
   - a top-level KNOWN keyword type (indexes/tablepartial/diagramview/
     dep) routes to its own validator: the per-kind context message on
     the element node, no inline-field check (probe v3)
   - the dot at a name/operand position errs UNKNOWN_PREFIX_OP
     ("Unexpected '.' in an expression") — their leftExpression_bp
     checks isOpToken FIRST (probes m43_0059/0091/0522/0580 PASS; the
     RParser dispatcher + the column/custom-element name-junk sites)
   - OPEN (probed, port next):
     * m43_0242 (+0206/0378/0521): 'Expect a comma' anchors the token
       AFTER the line end (8,1-4) where ours anchors the last token +
       trailing trivia (7,13-8,1) — their consume anchors the CURRENT
       token; ours is in the records-row tuple machinery
   - their validateColumn: the call-callee's TYPE error fires only when
     args stay EMPTY (a settings list at args[0] takes the 'Invalid
     column type' check instead — probe m43_0267 PASS); the enum's
     post-name check = their discardUntil (the current token's span,
     the body still parses after the scan — m43_0034/0113 PASS); the
     nested-dispatch kind = the LEADING KEYWORD, not the recovered item
     (m43_0108/0357/0406 PASS, both loops)
   - the table-body `name:` lines = their fieldDeclaration (canBeField:
     IDENT + COLON): the element {type, bodyColon, body} validated where
     it stands — the CustomElementValidator context error on the element
     node; validateBody: the callee must be a quoted string (`'…'` AND
     `"…"` both count) else the error on the WHOLE body node, and each
     arg errs PER ARG — the args start after the first operand's INFIX
     chain (the op tokens bind into the first expression — probe
     m43_0384: `parent.id >? child.parent_id` = one infix body, no
     args); both PASS
   - OPEN (probed, port next):
     * m43_0110/0122/0177 ('Expect a closing brace }' gap),
       m43_0111/0479/0517 ('Unexpected EOF' gap),
       m43_0240/0254/0526 (the 'default' must-be + 'EOF reached while
       parsing' cascade), m43_0108/0357/0406 ('A Ref must appear
       top-level' vs our 'Table must appear top-level' — the nested
       keyword dispatch's kind detection), m43_0179/0364/0511 ('Invalid
       newline encountered while parsing' gaps), m43_0223/0379/0534
       (the top-level sticky note: the close error PORTED — the invented
       "expected '}' after note body" replaced by their 'Expect a
       closing brace '}'; the remaining gap = 'A Note must have a
       content' on the element node (type .. the `{`, the last-real-token
       rule) when the triple-quoted content is broken),
       m43_0267/0480/0553 (our EXTRA 'A column must have a type'),
       m43_0015/0201 ('Expect an identifier' gap), and 12 pure-span
       cases (the endpoint-fragment column span m43_0193, the
       custom-setting value end m43_0230/0255, the swallow-tail
       element end m43_0251)
   - the bare-word endpoint's missing-column anchors the COLUMN TOKEN
     (column_spans[0]) — the hard-won rule applied to the inline-ref
     check (validate.rs 534; probe m43_0193 PASS: `poref` spans 44-49)
   - OPEN (probed, port next):
     * the attr-name node = the identifier STREAM: the key span trims
       the trailing trivia spaces, and a value-less attribute's node is
       the NAME stream alone (the value never parses when junk follows
       — probe x1 PASS: `uniEnum e` spans 42..51, the `{` junk point
       excluded); m43_0230/0255 PASS
     * the remaining ~340 spread across small clusters — keep
       clustering from the round dump (the compare tool:
       /tmp/cluster2.py pattern — ours in 6 batch files, theirs in
       theirs_mut.json, the signature = the per-message count delta;
       use Counter diffs — plain membership misses count mismatches)
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
