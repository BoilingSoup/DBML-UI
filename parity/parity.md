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
       route). The port (round 33): the
       custom-element body's child parse — their element = {type: c,
       name: id, body: the BlockExpression (3,5)-(4,19)} (the block =
       the `{` at 3,5 through the last real token), the inline-field
       error anchors the BODY block, and the ref block's field string
       errs the newline error — the structure: the body = the child
       loop (the ref block = a nested element parsed inside), the
       element span = the union ending at the last real token (4,19),
       the battery probe = nl1. IMPLEMENTATION SKETCH: the
       custom_element's body block-scan keeps the brace balance; the
       child loop parses the block's LINES as elements/applications
       (the `Ref {` line = a nested element via the existing
       dispatch), the unterminated string in the ref block's field
       errs via the existing note_value/sq_string path (the
       'Invalid newline' message + the partial-token anchor); the
       element span ends at the last real token before the swallow
       tail (the body's union end)
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
