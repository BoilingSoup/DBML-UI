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

## State (as of the campaign round 18)

- The fresh-seed rounds: seed 77 (300 cases) measures 114/300 — the
  fresh seeds keep finding new shapes; the convergence = the standing
  iteration with the tools below (mutate.py N SEED writes to /tmp/mut,
  the driver cases.json = the {name, path} array).

## Historical state (round 2)

- Grammar-shaped fuzz corpus (parity/fuzz, 300 cases): **300/300
  span-identical**.
- The span battery (56 probes — c*/d*/e* families): **all
  span-identical** (compare with ABSOLUTE paths in the cases.json — the
  driver resolves `path` from cwd).
- Corpus gates: 78/78 + 78/78. Focused tests 42/42 + editor 61/61.
- The user's three desktop files: span-identical, folded into the corpus.
- The mutation campaign (round 37, seed 43): the body child loop with
  the snapshot brace-scan LANDED (the spans match: nl1 2,3..4,19 /
  3,5..4,19; e43 5,1..7,14) — the remaining deltas: nl1's newline
  error lost, e43's extra close, e44's spans; the next round traces
  the nested parse's field-walk diagnostics and the close-count
  interplay. The historical counters below (the pre-child-loop
  rounds).
  rounds 2-19 landed (the mechanisms below); the identifier-stream
  attribute-name flow PORTED (the key scan crosses the trivia newlines,
  the real_end uses the occ's own end — the battery grew to e41-e42;
  seed 77 re-measures 120/300).

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
       m43_0111/0479/0517 (the 'Unexpected EOF' gap PORTED: EOF at a
       colon-led value errs their operand 'Unexpected EOF' at the EOF
       point, attr.value undefined — probes z1/z2 PASS),
       m43_0240/0254/0526 (the default cascade PORTED: the string-token
       'EOF reached while parsing' + the operand 'Unexpected EOF' + the
       'default' check at the EOF point — probe w1 PASS; the remaining
       delta needs the full default-validity check — their list:
       enum/string/number/function/true/false/null); m43_0254/0526
       (the TRUNCATED BACKTICK PORTED: the backtick fail consumes the
       partial token and errs 'EOF reached while parsing' on
       [backtick..the input end) — their lexer DROPS the token — then
       the round-13 operand-EOF + checkDefault cascade fires at the true
       EOF (probes b1/b3 PASS: five errors each, span-identical; b3's
       scan crosses newlines); b2 (the closed backtick) passes via the
       verbatim arm),
       (the TRUNCATED BACKTICK default at EOF: their FUNCTION token
       scan errs 'EOF reached while parsing' on the partial token +
       the operand EOF + the default check — our default arm's
       verbatim backtick scan is silent; route the unclosed backtick
       through the broken-value flow with their EOF message), m43_0108/0357/0406
       (PORTED — the nested dispatch maps the kind by the leading
       keyword),
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
   - the partial-attribute semantics: the SettingOcc carries `broken` —
     the swallow-gather/dispatch routes mark their occurrences broken
     and the validity checks skip them: a dispatched-broken default
     never reaches their check (probe e21 PASS); checkDefault PORTED
     (their isValidDefaultValue: a quoted string, a signed number,
     true/false/null, a backtick function, or a 2-3 fragment enum
     reference; the anchor = the value node ?? the name — probes a1-a8
     PASS)
   - the REF field with a swallowed BLOCK arg (round 23, probed q1):
     the RParser block_expression now errs the unclosed close at ITS
     EOF — but the ref's per-line token stream clamps at the line end,
     so the close lands at (2,46) where theirs fires at the TRUE EOF
     (6,1); their field node also spans to the swallowed block's close
     (2,38..5,2 — the app end = the block's `}` end) where ours anchors
     the quoted operand's span (2,38-2,46). REMAINING: the ref's
     application tokens must span to the file end for the unclosed
     block args (the line-clamped stream is the blocker — RESOLVED in
     rounds 24-26: the chain condition landed in the ref-fields loop
     and the standalone ref's line chain, and the inline form's
     file-end span landed in ref_body_pipeline_mode (the unclosed-`{`
     value extends the stream past the line to the file end) — the
     swallowed block's close now errs at the TRUE EOF, probe q1 PASS
     on the close). REMAINING: the field-node span — their field error
     anchors the application node (2,38..5,2, the block's `}` end)
     where ours anchors the arg span (2,38-2,46). ROUND-29 RESULT: the
     tree DUMP proved the round-28 split-difference trace WRONG — our
     args = [p, the-Block] exactly like theirs (the elemprobe confirmed
     their AST: {callee: the infix a.name < "missing".Project, args:
     [p, the Block]}); our arg errors now = TWO: (2,36..2,37) MATCHES
     and (2,38..2,46) vs theirs (2,38..5,2) — the REMAINING delta = the
     block arg's error end. ROUND-30: the ref_tokens multi-line mode
     LANDED (ref_tokens_ml: the newlines are trivia when the stream
     spans the file — the round-26 extension now actually crosses the
     lines) and the swallowed block's close now errs at the TRUE EOF
     (6,1) MATCHING theirs; the last delta = the block arg's error end
     still reads 46 — ROUND-31 RESOLVED: the round-30 ref_tokens_ml
     wiring landed at the ACTUAL extension site (the round-30 edit
     patched the wrapper but the extension still called the
     line-clamped ref_tokens), and the block arg's error end now
     reaches the block's `}` end — probe q1 PASSES FULLY (the field
     error 2,38..5,2 + the close 6,1; q2 PASS); the battery grew to
     e45 (83 probes).
     ROUND-24 TRACE: the ref-fields
     loop already chains the lines into one stream — the chain breaks at
     `if !dropped || input.is_empty()` (the clean line end stops the
     chain); the unclosed-`{` line must chain the following lines the
     same way the dropped case does (the depth > 0 check after the
     line-token build: keep chaining while the braces stay open, the
     stream ends at the block's balance or the EOF), then the
     application's block_expression sees the whole file and the
     round-23 close lands at the true EOF. ROUND-24/25 RESULT: the
     chain condition landed in BOTH the ref-fields loop and the
     standalone ref's line chain (the block form's unclosed-`{` lines
     chain, the gates green) — q1's ref is the INLINE named form
     (`Ref r0: ...`, the `:`-form path in ref_stmt), whose application
     runs on the SINGLE-LINE stream — the inline path's block-arg
     crossing is the remaining blocker (the inline application needs
     the same file-end span for its unclosed block args; the path sits
     in ref_stmt after the `:` eat, before the field-error walk)
   - the unclosed-block close errors = ONE PER UNCLOSED DEPTH: their
     blockExpression nesting errs every unclosed block's close consume
     at the EOF token (probe m43_0110/0122: a swallowed custom element
     with a nested `Table good2 {` errs TWO closes at the same point;
     the n1 control with good2 CLOSED errs one) — the custom-element
     body's unclosed arm pushes `depth` errors; the battery grew to
     e43/e44
   - the identifier-stream attribute-name flow PORTED (round 19): the
     settings-list key scan crosses the trivia newlines (the idents
     with any trivia between, the whitespace normalized in the key
     text), the real_end uses the occ's own end (the value_span's INIT
     point is not a node), and the `}` junk errs 'Expect an identifier'
     at the current token (probes c1/c2 PASS; the battery grew to
     e41/e42; seed-43 284→292, seed-77 re-measured below)
   - OPEN (probed, port next):
     * the newline cluster (m43_0179/0364/0511, probe nl1 = `Table a {
       id int }` + `  c.id > d.id }` + `Ref { a.id > 'b.id` + `Table b
       { id int }`): their parse = the top-level custom element `c.id
       > d.id` swallowing the REF BLOCK, the element node = (2,3)-(4,19)
       (the last REAL token — the union rule) with the inline-field
       error (3,5)-(4,19), and the ref block's field string `'b.id`
       errs 'Invalid newline encountered while parsing' at the partial
       token (3,14)-(3,19); ours = the element spans to the swallow
       tail (5,1), no newline error. ROUND-33 ATTEMPT: the child loop
       landed and the newline error matched (the ref block's field
       string errs via the note_value path), BUT the loop broke FIVE
       battery shapes (d4/d5/d7/e43/e44) and the element/inline-field
       spans ended at (3,6) — the span tracking (the nested element's
       consumed end + the last real token) isn't ready; the child loop
       is REVERTED and the port waits for the span-tracking design.
       ROUND-34 DESIGN (the span-tracking requirements): (1) the child
       loop's nested parse must NOT suppress the nested element's own
       close-at-EOF — the e43/e44 shapes need TWO closes (the
       Tabl-oops' + the nested good2's) at the same point, and the
       naive loop's nested top_item parse consumed the nested block so
       the depth rule counted one; (2) the enclosing element's span =
       the union ending at the last real token (the nested element's
       end when it parsed, else the last junk-free token); (3) the
       body child loop must run ONLY for the shapes their parser
       treats as block children — the clean single-application bodies
       (e42, the corpus) keep the existing path. IMPLEMENTATION NOTES
       (the gate): the child loop fires when the body's first content
       line after the `{` starts a KEYWORD element (the Ref/Table/
       Enum/... dispatch — probe nl1's `Ref {` line) — the gate keeps
       the clean single-application bodies on the existing path. The
       round-33 child-loop code is RECOVERABLE from the history
       (1215953; the patch extracted to /tmp/round33_child_loop.patch)
       — the round-35 implementation = the resurrected loop + the
       three-part design on top. ROUND-35 ATTEMPT: the resurrect +
       the gate verified e42/e44 but the e43 span shifted the OTHER
       way ((5,1)-(5,12) — the nested parse consumed the nested block
       and the span lost the body) and the corpus_lib tests broke —
       the depth rule's interaction with the nested parse needs the
       close-count design worked through the table parse before the
       loop lands. REVERTED; the state stays at 1c78dba. ROUND-36
       TRACE (the depth rule x the nested parse, the e43 close-count):
       the naive loop's nested parse consumed the nested `Table good2 {`
       and the ENCLOSING scan then ran on the post-nested content —
       the e43's enclosing body saw NOTHING after the nested block, so
       its own depth rule counted ONE close where theirs counts TWO
       (the nested good2's close + the Tabl-oops' close, both at the
       EOF point). THE FIX SHAPE: the child loop's nested parse must
       run on a SCRATCH (the diagnostics ride) AND the enclosing body's
       brace-scan must still see the nested block's `{` in its own
       balance count — i.e. the enclosing scan counts the braces
       INDEPENDENTLY of what the child loop consumed, so both the
       nested good2's close (from the nested parse) and the enclosing
       Tabl-oops' close (from the depth rule, depth = 2 after the
       nested `{`) fire. ROUND-37 ATTEMPT: the resurrected loop + the
       snapshot brace-scan + the union end landed the SPANS (nl1
       2,3..4,19 / 3,5..4,19; e43 5,1..7,14) but the same five battery
       shapes diverged (d4/d5/d7/e43/e44: the nested parse's
       diagnostic interplay — nl1's newline error lost, e43's extra
       close) and the committed loop PANICKED on the battery shapes
       (the snapshot's end applied to the post-child-loop input — the
       delta-advance fix + the revert = 47fae5c). ROUND-38 PLAN (the
       fifth attempt's two deltas, from the round-37 traces): (a)
       e43's extra close — the depth rule counted the snapshot's
       braces AND the nested parse emitted its own close: the
       ENCLOSING depth rule must count ONLY the braces OUTSIDE the
       nested element's consumed span (the nested parse emits the
       nested close itself); (b) nl1's newline error — the nested ref
       parse consumed the field string without the newline error: the
       nested parse's field walk (the ref_stmt's block-form loop)
       needs the unterminated-string handling the same way the body
       child loop's junk-line path does (the note_value/sq_string
       route). ROUND-42 RESULT — THE CHILD LOOP LANDED (abe734b):
       seven attempts, all-or-nothing met. The final mechanics: (a)
       the child_spans depth scoping (the nested parse's consumed
       region's braces skip the enclosing count — e43/e44's good2
       brace must not deepen the Tabl-oops body's own scan); (b)
       delta (b) RESOLVED DIFFERENTLY than sketched — the newline
       error was never the nested ref parse's field walk: the `Ref`
       token + `{` ride the custom element's ARG walk (their element
       = {type: c, name: id, body: BlockExpression (3,5)-(4,19)}),
       the body child loop's NON-KEYWORD line walk tokenizes the
       line and the unterminated string errs via note_value on the
       REST-OF-INPUT slice (the line slice alone would read the
       newline as EOF → the wrong message; the real newline anchors
       3,14-3,19); the quote walk steps past well-terminated strings
       so their close quotes do not re-scan; (c) the child_last_end
       union end — the body block's unclosed-branch union = the LAST
       REAL TOKEN across ALL child lines, keyword or not (e44's
       `T`/`able good2 {`/`id in` lines end the block at `in` 9,8;
       nl1's nested `Table b { id int }` ends it at 4,19) — the
       keyword arm AND the non-keyword arm both feed it;
       (d) the closed-nested suppression (d7): their blockExpression
       child = an APPLICATION with a block argument — a CLOSED
       nested element swallows its field-level diagnostics entirely
       (the `!!!broken` junk line inside the nested good2 errs
       nothing); an UNCLOSED one keeps its close consume error
       (e43: two closes at the EOF token); the closed test = the
       consumed region's last real char is the element's own `}`;
       (e) the round-37 snapshot-end panic fixed for good: the
       depth-0 `}`'s snapshot index resyncs to the advanced input by
       the absolute delta (close_abs = bs+1+end; the skip clamps so
       a close line already consumed as a junk child fails the eat
       safely). Battery = 84/84 (nl1 PROMOTED), corpus 78/78 with
       ranges, lib tests 84/84. The t1 operand-storm cluster RESOLVED
       in round 43 (see the field-name dispatch below); the
       nested-CLOSED-with-junk-string shapes unmeasured — the
       fresh-seed clustering will refine. The port (round 33): the
       custom-element body's child parse — their element = {type: c,
       name: id, body: the BlockExpression (3,5)-(4,19)} (the block =
       the `{` at 3,5 through the last real token), the inline-field
       error anchors the BODY block, and the ref block's field string
       errs the newline error — the structure: the body = the child
       loop (the ref block = a nested element parsed inside), the
       element span = the union ending at the last real token (4,19),
       the battery probe = nl1
     * ROUND 43 — the field-name junk dispatch (column()) PORTED,
       15/16 probe shapes identical, battery 91/91 (g1-g7 promoted),
       corpus 78/78. The dispatch map (the whitespace-delimited
       GATHER at the name position is the key):
       (a) an ALL-OPERATOR gather (`!`, `!!!`, `! !`, `-` — is_op_char
       runs, further op tokens folding across trivia incl blank
       lines) = their PREFIX-OP APPLICATION: the name node = the run
       .. the next operand's END (v1 `!`→2,3-3,5 through the next
       line's `id`; x2/x3/v5 identical; w2 skips blank lines); the
       name check errs at the node and the field SURVIVES — the type
       parses after the operand (v1: `int`, no type error); a `}`
       operand errs 'Invalid start of operand "}"' at the FULL token
       WITHOUT consuming it (it is the block's own close; w1: the
       pair anchors through the `}` 2,3-3,2); EOF errs 'Unexpected
       EOF' zero-width and the pair anchors the RUN only (x1:
       2,3-2,4); NO operand-start error otherwise, NO retry storm.
       (b) a PUNCT+IDENT gather (`!x`, `!!!broken`, `!broken`) = ONE
       invalid token: the name = the junk node, no operand error,
       the walk CONTINUES — the type + settings ride (w4:
       `!!!broken int [pk]` = the name error only); no type on the
       line → the type error anchors the junk node too — the PAIR
       (v2 2,3-2,5, v3 2,3-2,12).
       (d) a DIGIT-LEADING token = a NUMBER node: the name check errs
       at it and the walk continues (v4 `123 broken` = the name
       error only; w3 `123 456` = the name + 'Invalid column type'
       at the numeric type node) — ours previously parsed
       digit-lead names SILENTLY (ident() accepts them).
       (e) a numeric TYPE token (all-ascii-digits) errs 'Invalid
       column type' at the type node (w3: 2,7-2,10).
       (.) a lone `.` keeps UNKNOWN_PREFIX_OP ('Unexpected . in an
       expression', m43_0059). The tuple-callee `(` arm and the
       legacy single-token operand-die arm unchanged.
       OPEN (the v7 probe, `@#$ x int`): the LEXER junk-char flow —
       theirs errs 'Unexpected token @'/'Unexpected token $' per
       invalid char (gathered out), `#` without a following space errs
       'Expect a following space' AND the `#` rides as the name node
       (the name error 2,4-2,5), then `x` = the type and `int` =
       the inline settings ('Inline column settings can only be pk
       or unique' 2,9-2,12); ours errs the operand storm per char.
       A SEPARATE cluster from (a)/(b) — the lexer's invalid-char +
       invalid-comment handling, unported.
     * the custom-setting check anchors the value node ?? the name (the
       round-17 finding: probe x1 PASS: `uniEnum e` spans 42..51, the
       `{` junk point excluded); m43_0230/0255 PASS. The custom-setting check anchors
       attr.value ?? attr.name: the VALUE node when it parsed, the name
       stream when it didn't (x1 still PASS). OPEN: m43_0316 — the
       value = a tuple-plus-block application (`defaultrecords (a) {: …`)
       our value() leaves empty (no tuple arm at the settings-value
       position); m43_0240/0254/0526 (the default cascade: the string
       token's 'EOF reached while parsing' does not surface);
     * the records comma anchor (m43_0242, probes r1/r2: their consume
       anchors the CURRENT token after the trivia — the next line's
       `ode` at 5,1-5,4, not the newline token) stays OPEN with the
       finding: the naive trivia skip regresses the mid-typing recovery
       (the records_partial_input test: `records (a, b\n}` — the later
       table must survive; the skip needs the mid-typing `}` and the
       `ode) {` continuation distinguished first). The second delta =
       their 'Records at top-level must have a name in the form of
       table(col1, col2, ...)' for the malformed header (the tuple
       broke: the anchor = the header node 4,9-5,5) — our records port
       binds the nameless header silently),
       m43_0308 (REFINED by the round-17 probes c1/c2: their attribute
       NAME is an identifier STREAM crossing the trivia newlines —
       `[che\ncode text\n}` names the attribute 'che code text' with
       the node spanning 2,27..3,12 (the union of the idents), then the
       `}` errs 'Expect an identifier' at 4,1-4,2; ours names 'che' and
       errs at 3,3 — the port needs the key scan extended across the
       trivia newlines AND the downstream occ/custom-check flow to
       follow (the naive scan extension shifted the swallow-tail
       element spans and dropped the custom check — battery e3/c2
       regressions), so it waits for the flow work)
     * the remaining ~340 spread across small clusters — keep
       clustering from the round dump (the compare tool:
       /tmp/cluster2.py pattern — ours in 6 batch files, theirs in
       theirs_mut.json, the signature = the per-message count delta;
       use Counter diffs — plain membership misses count mismatches)
2. **Quick-fix EDIT-range parity — DONE** (enforced by the corpus
   gate): our dump emits edits [{start, end, newText}] converted to
   UTF-16 offsets (their Monaco model offsets); their driver emits the
   raw edits; run.mjs's fixKey includes the ranges. Ported divergence:
   their addSettingEdit's fallback anchors declaration.end — the node's
   union end (our Column.full_end), not the consumption end; the
   bracket path inserts at the `]` (their settingsList.end - 1). The
   battery quick-fixes ride the corpus gate.
3. **Keep the batteries honest**: any new divergence class found anywhere
   (user reports included) becomes a battery case + a corpus case when
   user-shaped, permanently.

## The mutation-campaign state (round 44 entry point)

- Seed 77 (300 cases, /tmp/mut-cases.json, the dumps /tmp/mut-ours-*.json
  + /tmp/mut-theirs.json): **129/300 identical** after rounds 42-46 (from
  120 at the round-41 base). ROUND 46: the custom-element name-walk's dq
  partial ported (r2 `Tabl"e a { id int }`: the newline error at the
  partial token 1,5-1,20, the gather takes it, the operand walk
  re-attempts across the trivia — at EOF 'Unexpected EOF' + the
  EmptyNode name 2,1-2,1; battery q2). The TableGroup `Note: 'x` sq
  shape ALREADY matched (r3 = battery q3). The r1 shape (`a in"t int`)
  recorded as COMPOSITE with the open lexer cluster ('Expect a
  following space' at the type token + the inline-settings cascade). ROUND 45: the dq-quoted scan's newline/EOF
  error ported into ident() (consumeUntil hits a newline → 'Invalid
  newline encountered while parsing' at the PARTIAL token — spaces do
  not stop it, m77_0056's `"odd col` = 2,3-2,11; EOF → 'EOF reached
  while parsing'); at the field-NAME position the dq error REPLACES
  the name-form error and the field DIES (no type error after, the
  next line parses fresh — battery q1). RESOLVED: the name-position
  newline shapes (the 24-missing signature left the top-8). REMAINING
  newline positions: the TYPE-position `"` (m77_0029 `a in"t ...`),
  the arg-walk `"` (m77_0058 `Tabl"e a ...`), the Note-value sq at
  the TableGroup (m77_0058 `Note: 'x` + the note-content follow-up) —
  the next round's probes. The divergent set = /tmp/mut-div.json; the
  message-signature clusters (Counter diffs, per-message counts across
  cases):
  * close: 34 cases — 'Expect a closing brace' missing 38 / extra 12;
    * expect-ident: 31 cases — 'Expect an identifier' missing 31 / extra
    17. The INTERNAL-LEAK HALF RESOLVED in round 44 (f7be49e): the
    bare/truncated-keyword header flow — Table/Ref/Project name-failure
    at EOF errs the TRIO ('Unexpected EOF' + "Expect an opening brace
    '{' or a colon ':'" + the keyword's name-form message — Table =
    'A Table name must be of the form <table> or <schema>.<table>',
    Ref/Project = 'A X's name is optional or must be an identifier or a
    quoted IDENTIFER' (their typo preserved verbatim)), the nameless
    `Table {` with a body errs 'A Table must have a name' over the
    whole ELEMENT span (1,1-2,2, deferred until the body parses);
    k-probes 8/8 (k1 `Table ` EOF trio, k4 the nameless-with-body, k6
    the Ref trio, k7 the Project trio, k5 = the Enum trio already
    green); battery 96/96 (h1-h5 promoted). The leak signatures
    ('expected identifier' x22, 'expected column type' x17) VANISHED
    from the seed-77 deltas; seed-77 = 124/300. The REMAINING half =
    the structured 'Expect an identifier' stack in the composite
    shapes (31 missing / 17 extra) — deeper recovery flows, the
    next-round work;
  * newline: 24 cases — 'Invalid newline encountered while parsing'
    missing 24;
  * colon-op: 17 cases — 'Invalid start of operand ":"' missing at the
    `:` in `Ref r0:` NAMED-REF headers inside composite-damage files
    (m77_0021/0043/0057 — the colon at the ref-header position when
    upstream damage skews the parse);
  * records-comma: 17 cases — m43_0242's cluster ('Expect a comma'
    missing);
  * eof-parse: 14 cases — 'EOF reached while parsing' missing;
  * the index-field cluster: 'An index field must be an identifier...'
    extra 17 (the `(name) [unique, name: 'ix']` paren-name shape);
- The seed-43 600-case campaign state: **309/600** (re-measured at the
  round-54 engine, a863d77's parent — from 295 at the round-41 base;
  the 13 mechanisms transferred without targeting that batch; the
  divergent set = /tmp/m43-div.json, the dumps /tmp/m43-ours-*.json +
  /tmp/m43-theirs.json, the case list /tmp/m43-cases.json).

## The rounds 47-54 RESULT + the round-55 queue

- RESOLVED (round 47): the two recorded column-pair shapes. (1) The
  `'''` TRIPLE-QUOTE header gather — theirs = the consumeUntil scan
  erring 'EOF reached while parsing' at the PARTIAL token (multi-line
  to EOF: s1 1,14-3,1), the scan consumes it, the body check lands at
  the scan stop (3,1); ported into the table header's body-check block
  via note_value (battery r1). (2) The broken-bracket args — the ROOT
  = their gatherInvalid's field death takes the LINE REMAINDER (s6:
  `id integer lt: 1\ncode int` = the death at the `:` eats `1`, the
  next line parses clean; s2's `1, pk]` dies whole); ported into the
  legacy junk arm's Err (battery r2/r3). s-probes 5/5 (s4 `pk]`
  already matched). Seed-77 129→135/300; the pair-extras signatures
  dropped 44→30 / 37→30.
- ROUND 48 LANDED: the table-header junk-name arm (the m42_0014 shape —
  the operand error on the junk token, the name-form at the junk's END
  point, the sync to the body's `{` with NO body-check error; the
  composite m77_0006 fully PASSES) and the COMMA-STREAM name node (the
  call-form field `abc, def` streams the name node across the comma —
  the pair anchors the whole node 2,3-2,11, the type parses after;
  battery c26). Seed-77 135→136/300.
- ROUND 49 LANDED: the enum/ref/project header syncs — the enum's
  duplicate body-check at EOF suppressed after the discard already
  erred (p4 `Enum e]x` at EOF = ONE error); the ref's body-check at
  the offending token + the sync to the `:` with the INLINE REF STILL
  PARSING (p2 `Ref r]x: a.b > c.d` = the body check + the two
  missing-table binder errors); the project's body-check + the sync to
  the `{` (p3 `Project p]x { }`). p-probes 4/4 (p1 `Enum e]x { a }`
  already matched). Battery u1-u3 promoted (105 files, all green).
- ROUND 50 LANDED: the EOF close errors in the block flows — the
  TableGroup members loop errs the close at the EOF token on BOTH
  exits (the loop-top branch AND the post-binder `input.is_empty()`
  break, which had been skipping it; t2 `TableGroup g { orde` at EOF =
  the binder + the close 1,20) and the records-block EOF path errs the
  SECOND close at the EOF point instead of the internal 'unterminated
  records block' message (the blockExpression nesting: EVERY unclosed
  block's close errs; t3 = the binder + TWO closes at 3,8). The t1
  shape (`Ref { a.\nid > 'b.id` at EOF) = the SWALLOW-CHAIN divergence
  recorded below — theirs merges the lines into ONE field spanning
  1,7-2,11 (the field error + 'Unequal fields' + the sq EOF error all
  at that span, then the close 2,11 + 'Unexpected EOF' 2,11); ours
  parses the lines as two fields (the field-1 errors at 1,7-1,9, the
  line-2 field errors extra, 'Unexpected EOF' 2,1 extra, the close
  missing). Battery w1/w2 promoted. Seed-77 136→140/300 (the close
  cluster 37→29 missing).
- ROUND 51 LANDED: the ref-block swallow-chain fidelity — (1) the
  incomplete-infix pull: a line ending in a trailing `.` (the member
  access with no right operand) chains the next line into the SAME
  field (the merged application `a.id > 'b.id`); (2) the EOF operand
  READS the EOF token (pos = toks.len()) so the application's node
  union reaches the EOF point — the field errors anchor 1,7-2,11 not
  the `>`'s end; (3) the broken binary's binder still rides — theirs
  binds the left endpoint's variables even with the tuple side missing
  (Table 'a' does not exist 1,7-1,8) — only the variable-less bindees
  skip. t1 FULLY IDENTICAL (battery y1). Seed-77 140→141/300.
- ROUND 52 LANDED: the custom-element name's BROKEN-BRACKET
  extension — a LATER-LINE unclosed `[` = their lexer's junk token:
  'Expect a closing bracket' errs at the EOF point and the contents'
  idents ride the NAME node (z1 `a int\n [pk, not null`: the name node
  = 1,5-2,15, the element = 1,3-2,15, NO setting-list error); the
  SAME-LINE `[` stays the settings branch (e3/e10/e27 un-regressed)
  and the later-line PLAIN idents do NOT join the name (v3/v4: the
  name stays `int`, the body check errs at them). Battery aa1
  promoted. Seed-77 141→142/300.
- ROUND 53 LANDED (abe5e84): the QUOTED-TYPE RESTART — their
  elementDeclaration type = an IDENTIFIER only: a quoted token errs
  'Expect an identifier' at the token's full span and the element
  RESTARTS at the next token (b1 `\n  "odd col" int[]\n  x varchar(255)
  [note: 'n']\n}`: the ident error 2,3-2,12, the element = type `int`
  from 2,13, the `[]` = the setting list 2,16-2,18, the body check at
  `x` 3,3-3,4 — 4/6 identical). Seed-77 142/300 (the 'Expect an
  identifier' missing 31→29; the composites hold on the deeper spans).
- ROUND 54 LANDED: the INLINE-BODY STRING-TOKEN RULE — the colon
  body's inline value = the STRING TOKEN: the body node ends at the
  string's end (b1: 3,28 = the 'n's end, NOT the raw-slice line end
  3,29) and the trailing junk errs 'Expect an identifier' ONLY on the
  invalid-operand `]` (3,28-3,29) — the juxtaposed words are their
  application's args and stay silent, and the `}` is LEFT for the
  enclosing flow (the program's junk arm errs it at 4,1-4,2; the host
  table's close stays silent — the first cut's per-token walk and the
  `}`-inclusion both regressed and were caught by the seed-77
  signature before commit). b1 6/6 (battery bb1). Seed-77 142→143/300
  (the 'Expect an identifier' extras signature GONE from the top-6).
- ROUND 55 RESULT — THE FLAT SCAN REVERTED (the method's first lesson):
  the d1 trace's ListExpressionNode [4,36) was MISREAD as a flat bracket
  scan; the dual-seed re-measure caught it (seed-43 309→307: m43_0087's
  'Expect a comma' at the entry tail and m43_0308's quoted-type-in-list
  error both vanished under the flat scan) and the pass-set diff pinned
  the flips to exactly m43_0087 + m43_0308. The truth: their list = the
  ENTRY parse (settings_list_occs-class), the [4,36) node = the UNION
  rule through the last real arg when the close fails — the node span
  shows the WHAT, the mechanism still needs the behavioral probes.
  Reverted to the balanced entry parse: seed-43 = 309/600 restored,
  seed-77 = 143/300, battery 110/110 (ff1 = d1 removed as the open
  shape). The methodological rule added below.
- ROUND 57 LANDED (317fec9) — THE ']' CONSUMER FOUND: the
  handler-scan's `#`-handler. The dispatch scan treated `#1` (the
  COLOR literal inside d1's nested `[color: #1]`) as a LINE COMMENT
  and ate `#1]` — the scan lost the list's real close and the `}`
  became the handler (the closer=Some((23,125)) debug line pinned it;
  the `]` sat at the relative 21, the scan jumped to 23). THE FIX: the
  `#` immediately followed by a hex digit = the color literal (skip
  the hex run), else the line comment. d1 = 4/4 (battery ff1
  re-promoted), seed-77 = 143/300, seed-43 = 309/600 — NO
  regressions. The round-56 source map = the map that made the fix
  findable in one session.
- ROUND 56 — the settings-list SOURCE MAP (read, not yet ported; the
  session's context ran out mid-debug — the parser.rs edits were
  reverted to the committed state, the tree = clean at the round-55
  closure):
  * THEIR listExpression (core/parser/parser.ts:1326) = READ: consume
    `[`; the FIRST attribute() ONCE (the catch = push the partial +
    canHandle ? synchronizeList : rethrow); the WHILE loop =
    consume('Expect a comma') + attribute() per iteration (the same
    catch shape); consume('Expect a closing bracket') at the end —
    every catch = synchronizeList (skip to the `,`/`]`) and the loop
    CONTINUES — no entry error ever kills the list;
  * their attributeName (parser.ts:1495) = the ident-stream: the while
    !check(COLON,COMMA,RBRACKET) → consume('Expect an identifier') per
    ident — the JUNK token inside the stream logs 'Expect an
    identifier' at the junk token (d1's `{` @1,15-1,16 = code 1005 = a
    SyntaxToken = THIS consume) and throws the partial
    IdentifierStreamNode;
  * d1 under THEIR engine: the stream `e orders` + the `{`-log + the
    sync (skip to the `]` at 2,20) → the while-loop exits (the `]` =
    RBRACKET) → the close consume = the `]` ✓ → the list = [4,36)
    CLOSED — the element = [0,36), the body check at the `}` 3,1 —
    4/4;
  * OURS' gap = RESOLVED in round 57 (the handler-scan's `#`-handler —
    see above);
  * the queue: m77_0039's remaining stack, m77_0043's Note-sticky, the
    'Expect an identifier' remainder, s5's span puzzle, the colon-op
    cluster (17), the index-field paren-name (17), m43_0242
    records-comma.
- ROUND 58 LANDED — the nested NOTE DECLARATION in the table body:
  `Note <name> { triple-quote }` errs 'A Note shouldn't have a name'
  at the name span (m77_0043's Note-delta resolved); the NAME-LESS
  `Note {` form falls through to the dedicated legal arm (the first
  cut shadowed it — the battery's e22 caught the regression: the
  broken body value errs its lexer error AND 'A Note must have a
  content' over the note-element span 2,3-4,2). Probe nn1. The
  m77_0039 composite = improved by the round-57 fix but remains the
  multi-damage tail (the broken-ref-occ's union-end exclusion, the
  line-18 recovery).
- ROUND 59 — the h1 MAP (traced, the port = queued): the minimal
  `Tabl[e orders {` element isolates the remaining deltas — the
  element node [0,107) = ..4,28 (ours 4,54), the value-node = the
  FIRST entry's comma-expression (`#1E69FD, default` = 4,12-4,28), the
  invalid tokens (`:`/`1`/`,`/`]`) = the consume-logs in NO node, and
  the `ref: ?<> orders.code` = the ref-SETTING entry INSIDE the
  inline-body (the ref-binder ran: the binary/unequal/missing-table).
  THE MODEL: their colon-body's non-quoted value = the
  ATTRIBUTE-ENTRY parse = settings_list_occs's entry-loop refactored
  for the unbracketed bodies — the token-walk attempt over-fired (16
  vs 12) and was reverted the same session; the port = the
  entry-loop refactor (round 60).
- ROUND 60 LANDED — the inline-body's non-quoted value = the
  h1-mapped TWO-PHASE walk: the VALUE phase extends the node union
  over the operand tokens (the color = `#`+hex consumed as ONE token
  — consume_one_token emits the `#` alone and the hex tail re-logged
  as digit-lead junk; the idents; the quoted strings via note_value),
  the `,` = the silent comma-expression separator, the next entry's
  `:` logs 'Expect an identifier' and ends the value-node — the h1
  SPANS = theirs (the element = 1,1-4,28, the value-string =
  4,12-4,28; the 4,54 swallow-tail deltas dead). The POST-value phase
  = the silent skip: their walk = the ATTRIBUTE-ENTRY parse with the
  ref-binder running (the ref-diags + the 4 consume-logs = the
  remaining 7) — the settings_list_occs entry-loop extraction = the
  round-61 queue (hh1 = 5/12, NOT promoted per the open-shapes rule).
- ROUND 61 LANDED — the post-value ENTRY-walk merged into the
  inline-body loop: the alpha-lead name-attempts (the ident = the
  silent entry name with the `:`-value introducer — their
  consume(IDENTIFIER) REJECTS the digit-lead: h1's `1` logs; ours'
  ident() accepts digits so the walk checks the lead explicitly), the
  junk tokens log 'Expect an identifier' at their spans (d1's
  `:`/`1`/`,`/`]` = four logs ✓), the value-runs log the `]`/`}`
  terminators and stay silent on the operands. h1 = 9/12 — the
  missing 3 = the ref-binder trio (the ref-SETTING entries inside the
  inline body = the round-62 queue). hh1 = still not promoted (the
  open-shapes rule). Both seeds hold.
- ROUND 62 LANDED — the ref-SETTING entries in the inline-body
  entry-walk: the ref-value = the ref-FIELD parse (the
  cardinality-op tokens + the endpoint streams `a.b` with the TABLE =
  the FIRST segment — the first-cut captured the column and the
  missing-table anchored 'code' instead of 'orders'); the endpoints
  != 2 errs the binary + the unequal pair over the value-run (h1's
  4,38-4,53 x2). THE MISSING-TABLE = the DOC-dependent check — the
  direct push fired on the VALID tables (m77_0039's extras exploded)
  and was reverted: it belongs to the binder-chains path (the
  validate-layer renders it with the doc access). h1 = 11/12 (the
  missing-table = the last 1); hh1 = not promoted (the open-shapes
  rule). m77_0039 = 45 vs 25 — the composite = the queue.
- ROUND 63 LANDED — the missing-table via the binder-chains: the
  custom_element signature gains the chains param (the single call-site
  passes it through), the inline-body ref-entry pushes the
  endpoint-table chain (the table fragment = the single var) — the
  validate's resolve_prefix renders it with the doc access: the missing
  table errs at the fragment (h1's 4,42-4,48), the existing resolves
  silently. h1 = 12/12 (hh1 PROMOTED), battery 113/113, corpus 78/78,
  lib 84/84. Both seeds hold (143/300, 309/600) — the h1 family = the
  narrow slice, the wide clusters = the round-64 pivot (the user
  approved the re-ordering).
- ROUND 64 LANDED (the wide-cluster pivot = VALIDATED): the
  field-name dispatch's bracket-lead and triple-quote exclusions —
  the BRACKET-lead junk (`)code`) = their lexer's bracket token = the
  LEGACY operand-die (the operand error + the pair at the junk's end;
  j1) and the `'''` at the field position = the triple-quote scan
  (j3: 'EOF reached while parsing' at the partial token to EOF, the
  field dies) leave the punct+ident gather arm. SEED-77 = 145/300
  (+2), SEED-43 = 310/600 (+1) — the biggest single-round gain in
  weeks; probes kk1/kk2; battery 115/115, corpus 78/78, lib 84/84.
  The position-start tally drove the choice: the paren-lead (17) +
  the quote-lead (11) = the widest patterns in the column-pair extras.
- ROUND 65 LANDED — the tuple-callee field SURVIVES: the paren-arm =
  the name-error at the TUPLE node via the binder channel + the
  fall-through (the Err-die and the retry-storm extras dead); the
  ty-walk supplies the type-missing at the tuple (u2's pair, the bare
  tuple) or 'Invalid column type' at the settings-list (j2's
  2,10-2,30). SEED-43 = 312/600 (+2!), seed-77 = 145/300. Probe mm1;
  battery 116/116, corpus 78/78, lib 84/84.
- ROUND 66 — THE CLOSE-BRACE CLUSTER = THE COMPOSITE-TAIL (the
  strategic finding): the 24 missing-close cases probed via the two
  minimal shapes (the enum with the values, the table with the
  duplicate-default) = BOTH ALREADY GREEN — the 24 = the composites
  where the upstream damage shifts the block-flow before the EOF
  close. NOT a single-mechanism cluster — the per-case grind = the
  background queue (the round-50 audit-pattern applies per-flow).
- ROUND 67 LANDED — the single-quoted token at the field-name
  position = the sq-string scan: an unterminated string errs the
  newline error at the partial token (r5: 2,3-2,22 to the line end)
  and the field DIES — theirs builds no custom element from the
  broken-quote line (ours' gather-arm misrouted it into the
  custom-element conversion: the appear/inline/name trio). The
  terminated string = the name-value rides with the identifier-check.
  The position-tally = the method that found it (the `'`-lead = 11 of
  the appear-extras). Probe pp1; battery 117/117, corpus 78/78, lib
  84/84. Both seeds hold (145/300, 312/600).
- ROUND 68 LANDED — the table body-check's discardUntil CONSUMES the
  junk when no `{` exists: the unconsumed tail re-dispatched as a
  phantom custom element (`Table pare integer` = the body-check once +
  the phantom element trio; theirs = the body-check once). SEED-77 =
  146/300 (+1), SEED-43 = 314/600 (+2!). Probe ss2; battery 118/118,
  corpus 78/78, lib 84/84. (The s1 `T{able a {` shape = ALREADY green
  — the round-64+ mechanisms covered it.) The audit-note: the same
  None-arm shape exists in the enum/project/group body-checks — the
  same consume-fix applies when the probes show it.
- ROUND 69 LANDED — the None-arm audit: the ENUM already consumes
  (q3 green); the PROJECT's no-`{` arm = the consume-fix (q4
  `Project p foo bar`: the phantom element dead); the GROUP's fail =
  the body-check-style error at the offending token's full span + the
  discardUntil with the direct return (q5 `TableGroup g foo bar`: the
  wrong message + the unconsumed tail + the extra close-error dead).
  Probes pp2/pp3; battery 120/120, corpus 78/78, lib 84/84. Both seeds
  hold (146/300, 314/600).
- ROUND 70 — the Expect-an-identifier tally (14 cases) + the lead
  mapped: the WIDEST = the dq-name-across-the-newline shape
  (m77_0099: the bare `Table` + the NEXT line's `"quoted name" {` —
  their name-stream = the ident-only, the dq = the JUNK (the two
  consume-logs: the dq 7,2-7,15 + the `{` 7,16-7,17); ours'
  qualified_name = the trivia-crossing = the dq = the legal name-part
  — the delta = the qualified_name's dq-part = the SAME-LINE-only
  rule (the newline = the stream-stop, the dq = the junk-log). The
  `}`-class = the second pattern (4 cases — the consume-logs inside
  the block contexts). The port = the qualified_name's dq-line-rule =
  the round-71 queue (the m43_0034's ident-crossing = the
  still-legal!).
- ROUND 71 LANDED — the `{`-block at the column TYPE position errs
  'Invalid column type' over the BLOCK span (the c5/c6 list-family —
  the block = the list's sibling; the internal 'expected column type'
  leak dead) — the u1/u2 shape = the dq-named field with the block
  (the m77_0099 family's cleanest slice). Probes uu1/uu2; battery
  122/122, corpus 78/78, lib 84/84. Seed-43 = 315/600 (+1), seed-77 =
  146/300. (The m77_0099 = the 20-diag cascade — the dq-line-rule =
  ONE piece of it; the case = the queue.)
- ROUND 72 — THE ENTRY-WALK OVER-FIRE = THE WIDEST SIGNATURE (102
  extra 'Expect an identifier' = 3× the next): the round-61 walk's
  logs over-fire on the composites in three families — (1) 50 logs at
  positions PAST the line-end (the empty-segment logs), (2) 25 on the
  ref-op tokens (`?<>`/`?>` — theirs' ref-parse handles them, ours'
  walk logs them — the m77_0070 `Ref: parent.id <>? child.paren`
  shape reaches the walk via the upstream unclosed bodies), (3) the
  `}`-block-close logs where theirs = silent. THE FIX = the walk's
  CONTEXT-GUARD (the fresh-context design: the walk's reach must stop
  at the construct boundaries their walk respects — the ref-entries
  delegate to the ref-parse, the block-closes end the walk) = the
  round-73 spec. The h1/hh1 = the narrow shapes where the walk = the
  measured-correct — the guard = the generalization.
- ROUND 73 — THE KEYWORD-STOP GUARD = THE LOCAL-AMBIGUITY (the
  attempted, reverted): the walk's loop-top keyword-break regressed
  h1 by one — the peek_kw('Ref') matches the h1's ENTRY-KEY `ref:`
  (the lowercase, the icase!) AND the m77_0070's `Ref:`-statement —
  the two = locally indistinguishable at the walk's position. The
  distinguishing signal = the entry-context (the walk = inside the
  h1's body-value = the entries = the keys; the m77_0070's = the
  top-level-ish statement) — the guard = the CONTEXT-plumbing (the
  walk's caller-side distinction = the fresh-context design; the
  naive keyword-stop = wrong on both ends). The over-fire families
  (the 102) = the queue with this finding as the map.
- ROUND 74 — THE m77_0039 SCOPE-FINDING: the over-fire = FILE-WIDE
  (41 'Expect an identifier' logs across the lines 1-26 — every
  construct in the file, not just the entry-walk's inline-body): the
  composite cascade = MULTI-FLOW. The round-74 line-scope/caller-side
  guard = INSUFFICIENT alone — the 102 = the sum of the per-flow
  over-fires (the entry-walk + the field-parses + the block-loops),
  each flow = the individual guard-work. THE STRATEGIC STATE: the
  campaign = the deep-tail phase — the case-count metric = the
  all-or-nothing (the m77_0039 = the ~10-flow composite = the
  multi-round case); the diagnostic-level deltas = shrinking. The
  session's context = the wall — the next session = the per-flow
  guards (the entry-walk's caller-side = the one; the field-parses'
  junk-logs = the two; the block-loops' = the three), probe-first
  with the m77_0039's per-line map (the file = the best instrumented
  composite — 41 logs = the per-line ledger).
- ROUND 75 — THE m77_0039 PER-LINE LEDGER (the deepest finding): the
  lines 1-4 = the h1-slice = ours = theirs (the round-63's ✓); the
  lines 5-10 = the mixed over-fires (the line 5's fields + the
  records-block); BUT the lines 11-26 = THE VALID CONSTRUCTS (`Table
  ñT {`, `Table B {`, `Enum e_status {`, `TableGroup g {...}`) — ours
  logs THEIR LINES as entry-junk (the walk = the element's post-value
  = swallowing the REST OF THE FILE) while theirs parses them
  (the duplicate-column/comma/TableGroup-field diags = the missing).
  THE ROOT = the element's post-value walk = NOT bounded by the line
  — the walk = the file-swallow = the entry-logs on the following
  constructs' lines. THE FIX-DESIGN: the post-value walk = the
  LINE-SCOPED (the value-loop's line-end break = the wall — the
  walk's line-4 stop = the existing ✓ — the LOGS = the lines 5-26 =
  the WALK = ... the audit = the walk's actual reach vs the logs'
  positions = the round-76 first action with the fresh eyes).
- ROUND 76 LANDED — THE LINE-SCOPE WALL: the reach-audit = ONE
  instrumented run (67 walk-iterations on the m77_0039 = the
  file-wide walk) and the root = the ws() running BEFORE the
  newline-check — the whitespace consumed the line-break before the
  guard saw it. THE FIX = the raw pre-ws lookahead (the newline =
  the walk's wall). m77_0039: missing=9/extra=32 → missing=4/extra=5
  (the valid constructs parse — the ledger's lines 11-26 = clean);
  h1 = 12/12; battery 122/122, corpus 78/78, lib 84/84. Both seeds
  hold (146/300, 315/600). The over-fire ledger = the instrument that
  found a one-line root cause in one session.
- ROUND 77 — the m77_0039 remaining deltas = the LINE-18
  SETTINGS-SWALLOW (the fresh ledger): the field `code int[] [check:
  \`x > 0\` indexes {` — the settings-list's `[` with the unclosed
  `indexes {` inside = ours EOF-swallows (the trio at the EOF); theirs
  recovers with 'Expect a comma' at the `indexes` (18,30-37) + the
  parse continues (the L19-L26 = parsed). Plus the two single-diag
  spans (the L21's `}`-ident-log, the L26's TableGroup-field span).
  The fix = the settings-list's recovery on the nested-`{` junk =
  the round-78 queue (the settings_list_occs's `]`-terminator vs the
  nested-brace handling).
- ROUND 78 — THE W1 TRACE + THE COMMA-CHECK ATTEMPT: the trace =
  theirs' comma-log @ [33,40) = the SyntaxToken code 1005 = the
  list-loop's consume('Expect a comma', COMMA) = the round-56
  source-map CONFIRMED (the comma-before-each-subsequent-attribute).
  The ours-side comma-check = inserted at the entry-loop's top but
  UNREACHED — ours dies in the FIRST entry's value-parse (the
  backtick-string's tail swallows the `indexes {` into the
  EOF-reached = the sq-message path) before the loop's second
  iteration. THE ROOT = the first entry's value-parse = the
  upstream-die — the comma-check = the SECOND-iteration semantics;
  the fix = the value-parse's tail-handling FIRST (the probe-first
  = the fresh-context round-79). The parser.rs edits reverted (the
  tree clean).
- ROUND 79 — THE PARTIAL READ: the w1's EOF-reached = @2,20 = the
  BACKTICK position (the `\`x\``'s opener — the col 20) — ours'
  first-entry value-parse mis-scans the backtick string through the
  sq/note_value family (the 'EOF reached while parsing' = their
  message, the span 2,20-4,1 = the swallow to the EOF) — the value =
  the backtick-string consumed as a string, the tail (`indexes {`)
  swallowed, the field = the EOF-cascade. The ref-region's string-flow
  (the sq/swallow_gather = the read) = the ref-only path — the
  check:-value = the non-ref = the colon-branch's backtick handling =
  the un-read region. The session's context = the wall — the fix =
  the next session's first action (the backtick-value parse = the
  read + the fix, THEN the comma-check).
- ROUND 80 — THE VALUE-PARSE MAP (the read): settings_list_occs's
  colon-branch = the key-dispatch (`ref` = the endpoint/ref-binder
  flow; `default` = the verbatim-quoted flow; the OTHER keys = the
  generic-value tail = the UN-READ region — the w1's `check:` = the
  generic = the backtick mis-scan site). The EOF-reached @2,20 = the
  backtick's opener = the generic-value's sq/note mis-scan. The fix =
  the generic-value's backtick consume (the `\`…\`` = the verbatim
  token) = the round-81 first action, THEN the comma-check (the
  verified insert). The session's context = the wall — the read's
  scope = the recorded.
- ROUND-81 QUEUE:
  * the generic-value's backtick consume (the read the tail-region +
    the fix — the w1's EOF-reached @2,20 = the backtick mis-scan),
    THEN the comma-check (the verified insert);
  * the composite queue (the m77_0099-family cascade, s5's span
    puzzle, the colon-op cluster, the index-field paren-name,
    m43_0242 records-comma).
  * s5 (`, 1 int` at a field start): theirs = ONE name error at
    1,10-2,6 — the span starts at the TABLE'S OPEN-BRACE END and ends
    mid-line — the mechanism unexplained (the block-child node's
    union?); ours errs the operand storm + the pairs;
  * the colon-op cluster (17): 'Invalid start of operand ":"' missing at
  the `:` of `Ref r0:` NAMED-REF headers inside composite-damage files
  (m77_0021/0043/0057 — the colon at the ref-header position when the
  upstream damage skews the parse).
- The r1 shape (`a in"t int`) = COMPOSITE with the open lexer cluster
  ('Expect a following space' at the type token `in` + the dq newline
  + the inline-settings cascade 3,3-3,9) — the lexer's invalid-char +
  keyword-space handling, unported (the v7 probe `@#$ x int` = the
  same cluster).
- The index-field paren-name cluster (17): 'An index field must be an
  identifier, a quoted identifier, or a functional...' extra — the
  `(name) [unique, name: 'ix']` shape.
- m43_0242's records-comma cluster (17): 'Expect a comma' missing —
  the recorded consume-anchor + the mid-typing recovery finding.

## The white-box tracing method (round 55 onward, user-approved)

The vendored engine (parity/upstream/dbml/packages/dbml-parse/src — the
driver imports it DIRECTLY, no node_modules copy) is instrumented:
core/types/errors.ts's CompileError constructor logs every diagnostic to
stderr when DBML_TRACE=1 is set:

  CE "<message>" @ [<start>,<end>) node=<NodeTypeName> code=<code>

Usage:
  cd parity/driver && DBML_TRACE=1 npx tsx theirs-dump-many.ts cases.json 2>&1 >/dev/null
(the JSON goes to stdout, the trace to stderr — redirect separately;
the harness parity-report stays clean without the env var).

What it answers: every diagnostic's anchor NODE TYPE + span — the
validator-layer question "which node did it anchor" becomes direct
instead of inferred from span arithmetic. Evidence: z1's name error =
an ArrayNode [int..null) (the round-52 broken-bracket port confirmed in
one trace); d1's list = a ListExpressionNode [4,36) = the FLAT bracket
scan to the FIRST `]` (the balanced-scan assumption was wrong — one
trace exposed the whole m77_0039 span family).

Limits: the trace shows the validators' anchors, not the parse-time
control flow — and a node's SPAN is the union rule's OUTPUT, not the
scan rule's definition (round 55's flat-scan misread: the [4,36) node
was the partial-list union, not a flat scan — the dual-seed re-measure
caught the regression the probe missed) — the rules living IN the parse (the swallow-chain class,
round 51) still need behavioral probes; the probes become
confirmations, not searches. The per-cluster deep-dive = instrument the
specific node builder (core/types/nodes.ts's span setters or the
parser's node creation) the same env-gated way.

The instrumentation = the HARNESS FIXTURE only — never shipped, never
committed upstream; if lost, re-create the 6-line env-gated hook in
errors.ts's constructor.

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

## Round 81 addendum — the else-region read (the partial)

The generic-value tail = the entry-tail flow (the occ-push + the
comma-eat + the close-eat + the tail-error dispatch — the c25e/d2
machinery = read and understood). The w1's EOF-reached @2,20 = the
backtick = the value-PARSE (the colon eaten, the value = the sq/note
mis-scan — the eat = eq_ignore_ascii_case = the backtick U+0060 vs
the quote U+0027 = NOT matching — the mis-scan = the deeper path in
the ':'-value's string-dispatch region, settings_list_occs's
colon-branch tail). The session's context = the wall — the exact
mis-scan site = the next session's first read.

## Round 82 queue

- The ':'-value's string-dispatch (the colon-branch's tail = the
  backtick mis-scan site — the w1's EOF-reached @2,20 = the read +
  the backtick consume fix), THEN the comma-check (the verified
  insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 82 addendum — the ':'-value's dispatch region read (the partial)

The region confirmed: `if key == "ref"` = the endpoint/ref-binder flow
(the sq_string + swallow_gather machinery); the generic keys = the
else where ours pushes `(key, None)` — the value "never parsed" — YET
ours' w1 emits the EOF-reached (the sq/note-family message) at the
backtick position. The contradiction = the diagnostic's SOURCE is not
the settings-value parse — it is the caller-side (the field's
type-position block = the round-71's `{`-block flow REACHED from the
settings entry: ours' type-walk's `{`-branch = the round-71's
'Invalid column type' + the field survives — the EOF-reached =
the OTHER caller = the column()'s sq/note = the type_str-fail path).
The round-71's `{`-branch = the FIX = in place — the w1's =
the caller = the column() with the `{`-value = hmm — the audit =
the next session's first read: the EOF-reached's emission-site =
grep the message-family in the column()/type-walk paths with the
w1 input as the instrumented run.

## Round 83 queue

- The EOF-reached's emission-site audit (the grep the message-family
  + the instrumented run on the w1 = the exact caller), THEN the
  backtick consume fix (the w1's = the caller-side = the round-71's
  `{`-branch-adjacent), THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 83 addendum — the emission-site audit (the partial)

The DBGE instrumented the sq_string/note_value-triple EOF-returns:
NEITHER fired on the w1. The remaining emitters = the dq-scan (the
ident's quoted-partial path), the type_str's dq-branch (the type-
position's `"`-quoted = the Fail with the EOF-message), and the
backtick-EOF site (`probe b1: [default: \`now(\`` — the backtick
consumeUntil = the Fail + the token dropped, the scan crosses the
newlines). THE W1's = the backtick `\`x\`` CLOSED — but the
EOF-reached fires at the backtick position 2,20 — the leading
candidate = the type_str's dq-branch family (the type-position's
quote-scan: the w1's `check:` value reaching the type-walk through
the generic-key path) — the instrument of the type_str's dq/EOF
arm + the backtick arm = the next session's first action (the
verified-insert discipline: the grep the DBGE count after each
edit!).

## Round 84 queue

- The type_str/backtick emission instrumentation (the verified
  insert), the w1 run = the exact emitter, the fix at that caller;
- THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 84 addendum — the emitter found, the sibling fix landed, the second emitter remains

The DBGD instrumentation (the type_str's dq-arm + the backtick-EOF
site) ran the w1: **the backtick-EOF fired at abs 29 = the space right
after `check:`** — the type_str path reaches the backtick value and
treats it as an unterminated quote-scan. THE FIX = the type_str's
backtick-quoted branch (the dq-sibling): the verbatim consume to the
close backtick. LANDED (the safe sibling-add; the battery/corpus/lib
green).

BUT w1's cascade PERSISTS = a SECOND emitter (the same shape at the
same position — the type_str fix = not the w1's path). The round-85 =
the DBGD instrument restored (the type_str's dq/backtick arms + the
note_value's backtick path), the w1 run = the second emitter's
identity, the fix at that caller, THEN the comma-check (the verified
insert). The composite queue = the background.

## Round 85 queue

- The DBGD instrument restored (the verified insert), the w1 run = the
  second emitter, the fix at that caller;
- THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 85 addendum — the DBGD6 elimination

The backtick-EOF site (the note_value's backtick arm, the 544-region)
instrumented with the verified insert: **DBGD6 did NOT fire** on the
w1 — that site is eliminated too. The remaining candidates for the
w1's EOF-reached @2,20: the dq-scan (the ident's quoted-partial
path) and the type_str's dq-branch (the `"`-quoted Fail) — both
quote-family scans that could mis-fire on a backtick-lead token.
The round-86 = instrument THOSE two arms (the verified insert), run
the w1, identify the emitter, fix at that caller.

## Round 86 queue

- The dq-scan + the type_str's dq-branch instrumentation (the
  verified insert), the w1 run = the emitter's identity, the fix at
  that caller;
- THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 86 addendum — the double elimination

The dq-scan (the ident's quoted-partial EOF-return) and the
type_str's dq-lead instrumented — **NEITHER fired** on the w1. Both
candidates eliminated. The remaining un-instrumented emitters of
'EOF reached while parsing': the type_str's `"`-quoted Fail block
(the 420-region — the ok_or_else closure), and the two RParser/
ref_tokens-region sites (the 6202/6241). The round-87 = instrument
THOSE (the verified insert), run the w1 = the emitter's identity.

## Round 87 queue

- The type_str's `"`-quoted Fail-block + the RParser-region sites
  instrumented (the verified insert), the w1 run = the emitter;
- The fix at that caller; THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 87 addendum — the triple elimination

The type_str's dq-Fail-closure (the correctly-placed eprintln inside
the ok_or_else closure) instrumented: **DBGH1 did NOT fire** — that
site eliminated. The ref_tokens' dq-arm instrumented: the run's
output was inconclusive (the session's context hit the wall before
the verification completed) — the site = the round-88's first
re-check. The elimination table: the sq_string, the note_value
triple, the backtick-EOF, the dq-scan, the type_str's dq-Fail —
five eliminated. The remaining = the ref_tokens' dq-arm (the
re-verify) and the type_str's dq-arm (the re-verify with the
corrected placement).

## Round 88 queue

- The ref_tokens' dq-arm re-verified (the DBGH3 insert + the grep
  count + the build verify + the run) = the emitter's identity;
- The type_str's dq-arm re-verified (the correctly-placed eprintln);
- The fix at the identified caller; THEN the comma-check (the
  verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 88 addendum — the note-triple doubly eliminated

The note_value's triple-EOF re-instrumented (the DBGI1, the verified
count 1): **STILL did not fire** on the w1 — that site is doubly
eliminated. The remaining candidates for the w1's EOF-reached @2,20:
the sq_string's EOF-return (the re-verify = the round-89's first
action — the round-83's instrument was verified but the run's
absence was never explained), and the 4208-region's e.message
pass-through (the re-push of an upstream Fail — the upstream's
identity = the actual emitter).

## Round 89 queue

- The sq_string's EOF-return re-verified (the DBGE re-insert + the
  count + the build + the run);
- The 4208-region's upstream-Fail identity (the instrument the
  re-push site = the e.message's source);
- The fix at the identified emitter; THEN the comma-check (the
  verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).

## Round 89 addendum — the double elimination (the sq + the re-push)

The sq_string's EOF-return (the DBGE2) and the 4208-region's re-push
(the DBGE3) instrumented — **NEITHER fired** on the w1. Both
eliminated. The remaining un-instrumented emitters of 'EOF reached
while parsing': the RParser/ref_tokens-region sites (the 6202/6241 —
the string-token gathers inside the ref-body lexer). The round-90 =
instrument THOSE (the verified insert), run the w1 = the emitter's
identity.

## Round 90 queue

- The RParser/ref_tokens-region sites instrumented (the verified
  insert), the w1 run = the emitter's identity;
- The fix at that caller; THEN the comma-check (the verified insert);
- The composite queue (the m77_0099-family cascade, s5's span puzzle,
  the colon-op cluster, the index-field paren-name, m43_0242
  records-comma).
