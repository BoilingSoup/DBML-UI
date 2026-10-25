# PLAN.md — editor polish batch

Scope from user request: new vim motions/edits, neoscroll-style animated
scrolling, case-insensitive DBML keywords, and keyboard fixes (Shift,
CapsLock, arrows, PageUp/PageDown everywhere).

## M1 — Keyboard plumbing: Shift, CapsLock, arrows, PgUp/PgDn

**Root cause (investigated):** iced 0.13's `keyboard::on_key_press` hands us
the *logical* key — `Shift+a` arrives as `Char('a')`, so capitals and shifted
symbols are impossible, and CapsLock is invisible. The full event
(`keyboard::Event::KeyPressed`) carries `modified_key` ("all modifiers applied
except Ctrl") and `text` (exactly what typing produces — respects Shift and
CapsLock).

- Replace the `on_key_press` subscription with `event::listen_with` matching
  `Event::Keyboard(KeyPressed { .. })`; map character keys through
  `modified_key` (fallback `text`), everything else as today.
- Extend the editor key shim: `PageUp`, `PageDown`, `Delete` (Space exists).
- Motion coverage in **all** modes (normal, insert, visual):
  - Arrows = hjkl equivalents (normal/visual already have them via
    `apply_motion`; add to insert: Back/Forward/Up/Down cursor moves).
  - Home/End = Head/End in insert; already mapped in normal.
  - PageUp/PageDown = page up/down: cursor moves by page and the view
    scrolls (animated, see M3). Page size = editor pane lines
    (`(window_height - ~62px) / LINE_HEIGHT`).
  - Delete = `x` in normal, delete-next-char in insert.
- Tests: shim-level engine tests for arrows in insert, PageUp/Down page
  math, Delete in both modes, Shift-produced capitals in insert
  (`modified_key` path).

## M2 — Vim edits: `r`, visual `u`/`U`

- **`r` (normal)**: `r` enters a pending state (new `Pending::R`); next key:
  - `Char(c)` replaces the char under the cursor; with `[count]`, replaces
    `count` chars with the same char as one undo unit — aborts (no change,
    like vim) if the count runs past end-of-line.
  - `Enter` replaces the char with a newline (vim behavior); `Esc` cancels.
  - Buffer gains `replace_next(n, c) -> bool` (single history push).
- **Visual `u` / `U`**: lowercase/uppercase the selection in place, keep
  visual mode and re-select the (possibly re-sized) transformed range.
  - Buffer gains `apply_selection_case(upper: bool) -> bool` (single undo
    unit; handles length changes like `ß` → `SS` by recomputing the end).
- Tests: `r` basic/count/abort/Enter; `u`/`U` on char and multi-line
  selections; selection integrity after transform; undo is one step.

## M3 — Animated scrolling (neoscroll.nvim behavior)

- State: `editor_scroll` (current, already f32) + `scroll_anim:
  Option<{ from, to, started, duration }>`.
- Subscription: `iced::time::every(16ms) -> Tick` **only while an animation
  is active** (conditional subscription), driving eased interpolation.
- Easing: ease-in-out cubic over **250ms** (neoscroll default speed/feel);
  re-requesting mid-animation retargets: `to` extends by the new delta and
  `from` restarts from the *current* animated position — repeated `Ctrl-d`
  stacks smoothly, exactly like neoscroll.
- Animated: `Ctrl-d`/`Ctrl-u` (half page), PageUp/PageDown (full page),
  `G`/`gg`/`[count]G` when the target line is offscreen, click-to-jump.
- Instant (neoscroll defaults): wheel, `j`/`k` cursor-follow.
- `Ctrl-d`/`Ctrl-u` also move the cursor half a page (vim), in normal,
  visual, and insert modes.
- Scroll clamping uses the real page size from M1.
- Tests: easing math (monotonic, endpoints), retarget accumulation
  (two `Ctrl-d` = one page total), clamping at buffer top/bottom.

## M4 — Case-insensitive DBML keywords

dbdiagram.io accepts `table`, `TABLE`, `TaBlE`, `pk`, `NOT NULL`, etc. Our
parser matches keywords case-sensitively. Fix, scoped to *keywords only*:

- Top-level: `Project`, `Table`, `Ref`, `Enum`, `TableGroup` — ASCII
  case-insensitive, keeping the existing word-boundary rule
  (`table` still doesn't match `tableX`).
- Block keyword `indexes`, project/table keys (`Note`, `database_type`), and
  column settings (`pk`, `primary key`, `not null`, `null`, `unique`,
  `increment`, `autoincrement`, `auto_increment`, `default`, `note`, `ref`,
  `headercolor`) — same treatment.
- Identifiers are untouched: an unquoted column named `note` inside a table
  body still parses as a column (settings/table-body parsing never consults
  the keyword list); quoted names unaffected.
- `highlight.rs` stays in lockstep: keyword comparison becomes
  ASCII-case-insensitive there too (single shared helper if cheap).
- Tests: `table/TABLE/TaBlE`, `ref:` lowercase, `PK`/`NOT NULL` settings,
  mixed-case sample parses to the identical layout as the canonical sample.

## M5 — Verification & wrap-up

- Full `cargo test` + clippy + fmt per milestone; commit per milestone.
- Xvfb + XTEST scripted checks: type capitals via real Shift keystroke;
  `r` replacement on screen; visual `U` on a selection; `Ctrl-d` produces
  intermediate frames (screenshot diff catches the animated scroll).
- Update README key reference + `?` overlay with the new bindings.

## Open decisions (made, flagged for review)

- Wheel stays instant (neoscroll default); can be animated later if wanted.
- `r<Enter>` replaces with a newline (vim-accurate) rather than canceling.
- Keyword case-insensitivity includes settings and `indexes`, not just the
  five top-level words — matches dbdiagram's actual behavior.
