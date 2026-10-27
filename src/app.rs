//! Application state and the iced update loop.

use crate::canvas::diagram::{hit_test, to_cell_f, CELL_ASPECT, MARGIN_CELLS};
use crate::editor::{self, Action, Editor, KeyCode, KeyEvent, KeyModifiers};
use dbml_ui::dbml::ast::Span;
use dbml_ui::layout::{self, Layout};
use dbml_ui::{dbml, model};
use iced::{Point, Vector};

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    CanvasPressed(Point),
    CanvasMoved(Point),
    CanvasReleased(Point),
    /// Wheel notches (normalized: lines, or pixels / 40).
    Wheel(f32),
    /// A keypress, mapped from iced's keyboard events.
    KeyPressed(KeyCode, KeyModifiers),
    /// Wheel motion over the editor pane, both axes in notches (winit
    /// lines, or pixels / 40) and app-space: positive `v` scrolls the view
    /// down, positive `h` scrolls it right.
    EditorScrolled { v: f32, h: f32 },
    /// Left-press in the editor pane (local px).
    EditorPressed(Point),
    /// The pane seam was pressed (on the invisible resize strip); drag to
    /// resize the split.
    DividerPressed,
    /// Window-relative mouse move (drives divider drags).
    MouseMovedWindow(f32, f32),
    /// Left button released anywhere.
    MouseReleased,
    /// Window resized.
    WindowResized(f32, f32),
    /// A completion popup row was clicked.
    CompletionPicked(usize),
    /// Mouse hover over the editor pane (local px, None when it left).
    EditorHover(Option<Point>),
    /// Animation frame for the editor scroll.
    Tick,
    /// Expand/collapse the editor pane (Space+e or the seam chevron).
    ToggleEditor,
    /// Start a new, unsaved diagram from the start screen.
    NewDiagram,
    /// Toggle vim / Regular editing (persisted).
    ToggleVim,
    /// A recent file was picked on the start screen.
    OpenRecent(String),
    /// Open the native file-picker dialog (async; the UI stays live).
    OpenFilePicker,
    /// The file dialog closed without a choice.
    PickerDismissed,
    /// No dialog tool exists; fall back to the cmdline.
    PickerUnavailable,
    /// Toolbar/button save-as request.
    SaveAsPrompt,
    /// The native save dialog produced a target path.
    SaveAs(String),
    /// Run the canonical formatter over the whole buffer.
    FormatNow,
    /// Flip the persisted format-on-save preference.
    ToggleFmtOnSave,
    /// The + button / Ctrl+T: open a fresh tab showing the start screen.
    NewTab,
    /// Close the tab at index (X button, middle-click, Ctrl+W, menu).
    CloseTab(usize),
    /// Press on a tab: activate it and start click & drag tracking.
    TabPressed(usize),
    /// Right-press on a tab: open its context menu near the pointer.
    TabMenu(usize),
    /// Context-menu item: close the tab the menu belongs to.
    TabMenuClose,
    /// Context-menu item: close every tab, leaving one start screen.
    TabMenuCloseAll,
    /// A click anywhere outside the context menu dismisses it.
    DismissTabMenu,
    /// Right-press in the editor pane: open its Copy/Cut/Paste/Format
    /// context menu (canvas-local px).
    EditorMenu(Point),
    /// Context-menu item: copy the selection to the register and the
    /// system clipboard (the selection stays).
    EditorMenuCopy,
    /// Context-menu item: cut the selection (register + system clipboard).
    EditorMenuCut,
    /// Context-menu item: paste the system clipboard at the cursor.
    EditorMenuPaste,
    /// Context-menu item: run the canonical formatter (Message::FormatNow).
    /// A click anywhere outside the editor context menu dismisses it.
    DismissEditorMenu,
    /// Open the record view for the table node at this schema index.
    OpenRecords(usize),
    /// Close the record view overlay.
    CloseRecords,
    /// Follow a navigable record cell: open the target table's record
    /// view with the matching row highlighted (best-effort match).
    RecordsFollow(usize, usize),
    /// The diagram pane's Auto-arrange button: ELK layered left-right
    /// positions for every table (see `layout::auto_arrange`), written
    /// into `table_pos` (the same map user drags fill) and re-laid out.
    AutoArrange,
    /// The diagram pane's Zoom to Fit button: scale + pan so the whole
    /// diagram fits the visible diagram canvas.
    ZoomToFit,
    /// The toolbar Undo button: ONE undo step on the active buffer — the
    /// same history stack vim's `u` walks (single step; `[count]u` stays
    /// vim-only).
    Undo,
    /// The toolbar Redo button: ONE redo step on the active buffer (the
    /// Ctrl+r stack).
    Redo,
}

/// The open record-view overlay: the table node whose records are shown
/// and, when the user followed a FK cell, the row to highlight.
#[derive(Debug, Clone)]
pub struct RecordView {
    /// Index into the schema's nodes (a table with records).
    pub node: usize,
    /// Followed row (a FK click landed on it), if any.
    pub follow: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Canvas,
}

/// Open completion popup: candidate items, the selected index, where the
/// completed word starts in the buffer, and the typed prefix.
/// An in-flight animated scroll, neoscroll.nvim-style: each tick applies
/// the absolute whole-line positions for the current eased fraction —
/// cursor and window travel one eased timeline, each clamped at its own
/// limit (the window freezes at stop_eof while the cursor glides on).
#[derive(Debug, Clone)]
pub struct ScrollAnim {
    pub scroll_from: f32,
    pub scroll_to: f32,
    pub started: std::time::Instant,
    /// Seconds; see `scroll_duration_override`.
    pub duration: f32,
    /// Cursor rows coupled to the scroll (neoscroll scrolls the cursor one
    /// line per tick alongside the window, gj/gk with ctrl-e/y). Each side
    /// clamps independently: the window freezes at its limit (stop_eof)
    /// while the cursor continues alone (cursor_scrolls_alone). None for
    /// view-only animations (G/gg/jump-to-definition).
    pub cursor: Option<(usize, usize)>,
}

/// Duration override in ms for slow-motion verification
/// (DBML_UI_SCROLL_MS); None in normal use.
fn scroll_duration_override() -> Option<f32> {
    std::env::var("DBML_UI_SCROLL_MS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|ms| ms / 1000.0)
}

/// neoscroll's canonical mapping durations.
pub const HALF_PAGE_MS: f32 = 250.0;
pub const FULL_PAGE_MS: f32 = 450.0;

/// Zoom range (px per diagram cell height) the wheel allows; Zoom to Fit
/// clamps to the same range so the two always agree.
pub const ZOOM_MIN: f32 = 5.0;
pub const ZOOM_MAX: f32 = 160.0;

/// Uniform screen margin (px) Zoom to Fit keeps around the content
/// (dbdiagram's focus margins RIGHT 20 / TOP 60 / BOTTOM 60, simplified
/// to one value).
const FIT_MARGIN_PX: f32 = 40.0;

/// Vim's 'scrolloff': the cursor is kept at least this many screen lines
/// away from the top and bottom of the window whenever the buffer has
/// room. j/k scroll the view to hold the margin; Ctrl-d/u first walk the
/// cursor out to the margin before the view scrolls under it.
const SCROLLOFF: f32 = 5.0;

/// How long after a ctrl+d/u scroll chord a bare d/u is treated as the
/// chord's roll-out rather than a vim key.
const SCROLL_CHORD_ROLLOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Distance-progress at time-fraction `y` under neoscroll's quadratic
/// easing. neoscroll applies easing to the TIME between line steps
/// (ef(x) = 1 - (1-x)^(1/2) maps distance to time); inverting gives the
/// distance covered by a given time — a quadratic ease-out in space:
/// fast start, long decelerating arrival.
fn scroll_progress(y: f32) -> f32 {
    1.0 - (1.0 - y.clamp(0.0, 1.0)).powi(2)
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub items: Vec<crate::editor::completion::Item>,
    pub selected: usize,
    /// Buffer position where the completed word starts.
    pub word: crate::editor::buffer::Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    None,
    /// Dragging empty space: pan the view, anchored at the press point.
    Pan {
        anchor: Point,
    },
    /// Dragging a table: live re-layout at the grabbed offset.
    Table {
        title: String,
        grab: (f32, f32),
        press: Point,
        moved: bool,
    },
    /// Dragging the pane divider.
    Divider,
    /// Regular-mode mouse text selection in the editor: press anchors at
    /// the char under the pointer, MouseMovedWindow extends it,
    /// MouseReleased finalizes (a click collapses it).
    EditorSelect,
}

/// A click (no drag) on the diagram: the editor jump target, resolved at
/// release time. A press on a column row carries that column's name span;
/// any other press on a box (header band, padding) the table's name span,
/// so the caret lands on the definition line either way.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingJump {
    /// Span of the name token to land the caret on.
    pub span: Span,
}

// ---------------------------------------------------------------- tabs --
//
// Chrome-like tab model: every open document is a `Tab`; `App` renders the
// active one. Split —
//
// per tab (`Tab`): everything that belongs to one buffer and its diagram —
//   source, editor (mode state + undo history), diagram layout + dragged
//   table positions + hover/highlight animation state, scrolls, parse
//   errors, highlights, file path and the dirty-flag reference. Diagram
//   edge indices (hover_edges/edge_anim) live with the diagram they index
//   so switching tabs can never leave them mismatched.
//
// global (`App`): window chrome and cross-tab UI — pane focus/collapse,
//   config, the in-flight mouse drags (pan/table/divider/tab-reorder),
//   completion popup, hover tooltip, pending diagram->editor jump, help
//   overlay, toast, split, window size. These either outlive a document
//   or reference the pointer, not the buffer; the ones tied to the
//   previous tab's content (completion, hover, jump, drag) reset on every
//   switch in `after_tab_switch`.

/// Width of one tab slot in the strip: the fixed tab button plus its
/// trailing gap. Drag-reorder slots are this wide.
pub const TAB_SLOT_W: f32 = 164.0;
/// The drawn tab width (slot minus the strip's tab gap); shared by the
/// drag-swap thresholds and the canvas strip.
pub const TAB_VISUAL_W: f32 = TAB_SLOT_W - 8.0;
/// Left inset of the tab strip — the origin strip-relative pointer x is
/// measured from (kept in sync with the strip container's padding).
pub const STRIP_LEFT: f32 = 6.0;
/// Horizontal pointer travel before a tab press becomes a reorder drag
/// (Chrome-like: a click is a click, a drag moves the tab).
const TAB_DRAG_THRESHOLD: f32 = 6.0;

/// Click & drag tab reorder in flight: the dragged tab's index, where the
/// press happened (strip-relative px), and whether the threshold is crossed.
#[derive(Debug, Clone)]
pub(crate) struct TabDrag {
    pub(crate) from: usize,
    /// Pointer offset within the pressed tab at press time (strip px):
    /// the dragged tab's left edge is always `x - grab_dx`, anchored to
    /// the POINTER so the position is continuous across swaps.
    pub(crate) grab_dx: f32,
    pub(crate) moved: bool,
    /// Current pointer x, strip-relative (drives the dragged tab's pose).
    pub(crate) x: f32,
}

/// Right-click context menu over a tab, anchored at the pointer.
#[derive(Debug, Clone)]
pub struct TabMenu {
    /// Index of the tab the menu acts on.
    pub tab: usize,
    /// Window-relative anchor (last known pointer position).
    pub at: Point,
}

/// Right-click context menu over the editor pane, anchored at the pointer.
/// Item availability is snapshotted at open time: Copy/Cut need a buffer
/// selection, Paste needs text on the system clipboard (queried lazily
/// here, on the UI thread; a failed read just disables the item).
#[derive(Debug, Clone)]
pub struct EditorMenu {
    /// Window-relative anchor (last known pointer position).
    pub at: Point,
    /// The buffer has a selection: Copy and Cut are enabled.
    pub has_selection: bool,
    /// The system clipboard had text when the menu opened: Paste enabled.
    pub clipboard_has_text: bool,
}

/// Chrome-style drop slot for strip-relative pointer `x` over `tabs`
/// fixed-width slots: the slot under the pointer, clamped into the strip
/// (dragging past either end pins to the first/last slot).
/// Remove the element at `from` and re-insert it at `to` — the live
/// reorder behind tab dragging.
fn move_element<T>(v: &mut Vec<T>, from: usize, to: usize) {
    let item = v.remove(from);
    v.insert(to, item);
}

/// One open document: all per-document state (see the split comment above).
pub struct Tab {
    /// Raw DBML source shown in the editor.
    pub source: String,
    /// Vim editor over the source (mode state + undo history).
    pub editor: Editor,
    /// Parsed + laid out diagram for the current source.
    pub diagram: Layout,
    /// Parse errors for `source`.
    pub parse_errors: Vec<dbml::parser::ParseError>,
    /// Syntax-highlight tokens for `source` (byte spans), rebuilt with it.
    pub highlights: Vec<dbml_ui::dbml::highlight::Token>,
    /// User-dragged table positions (diagram cells, by table name);
    /// survive re-parses while editing.
    pub table_pos: std::collections::HashMap<String, (i32, i32)>,
    /// Editor viewport scroll, in lines.
    pub editor_scroll: f32,
    /// Horizontal viewport scroll in pixels, kept just large enough that
    /// the cursor stays inside the pane when lines are wider than it.
    pub editor_hscroll: f32,
    /// Active animated scroll (neoscroll-style): interpolate editor_scroll
    /// from `from` to `to` over SCROLL_DURATION.
    pub scroll_anim: Option<ScrollAnim>,
    /// Accumulated target of the active animated scroll (repeated Ctrl-d
    /// extends it instead of restarting).
    pub scroll_target: Option<f32>,
    /// Diagram zoom: pixels per diagram cell height.
    pub zoom: f32,
    /// Viewport offset into the zoomed diagram, in pixels.
    pub pan: Vector,
    /// Diagram edges to highlight (hovered row's connections / hovered line).
    pub hover_edges: Vec<usize>,
    /// Per-edge highlight amount in 0..1, eased toward `hover_edges` on
    /// every tick so highlights fade in and out (parallel to
    /// `diagram.edges`).
    pub edge_anim: Vec<f32>,
    /// File backing the buffer, if any.
    pub file: Option<std::path::PathBuf>,
    /// Buffer contents at the last save/open — modified-flag reference.
    pub saved_text: String,
    /// A document is open (editor pane active). False only on a start
    /// screen tab, where the pane is not collapsible.
    pub doc_open: bool,
}

impl Tab {
    /// A document tab seeded with `source` (a diagram is open).
    pub fn with_source(source: String) -> Self {
        let mut tab = Self {
            source: String::new(),
            editor: Editor::default(),
            diagram: layout::layout(&model::build(&dbml::parser::parse("").doc)),
            parse_errors: Vec::new(),
            highlights: Vec::new(),
            table_pos: Default::default(),
            editor_scroll: 0.0,
            editor_hscroll: 0.0,
            scroll_anim: None,
            scroll_target: None,
            zoom: crate::view::BASE_ZOOM,
            pan: Vector::default(),
            hover_edges: Vec::new(),
            edge_anim: Vec::new(),
            file: None,
            saved_text: String::new(),
            doc_open: true,
        };
        tab.editor.set_text(&source);
        tab.set_source(&source);
        tab
    }

    /// The start-screen tab: no document; the side pane shows the Open
    /// file / New diagram buttons and is not collapsible.
    pub fn blank() -> Self {
        let mut tab = Self::with_source(String::new());
        tab.doc_open = false;
        tab
    }

    /// Re-parse + re-layout (keeps dragged table positions).
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        self.highlights = dbml_ui::dbml::highlight::highlight(src);
        let out = dbml::parser::parse(src);
        let schema = model::build(&out.doc);
        self.diagram = layout::layout_with_overrides(&schema, &self.table_pos);
        // keep highlight state aligned with the new edge list (indices are
        // stable for an unchanged schema; edits may shift them, which just
        // moves the highlight — the amounts ease back out)
        if self.edge_anim.len() < self.diagram.edges.len() {
            self.edge_anim.resize(self.diagram.edges.len(), 0.0);
        }
        self.parse_errors = out.errors;
    }

    /// Unsaved changes: the buffer differs from the last save/open.
    pub fn is_modified(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    /// Strip title: the file's base name, "Untitled" for a nameless
    /// document, "Start" for the start screen.
    pub fn title(&self) -> String {
        if !self.doc_open {
            return "Start".to_string();
        }
        match &self.file {
            Some(f) => f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.display().to_string()),
            None => "Untitled".to_string(),
        }
    }
}

pub struct App {
    /// One tab per open document; always at least one (the window stays
    /// alive like a browser's).
    pub tabs: Vec<Tab>,
    /// Index of the active (rendered) tab.
    pub active: usize,
    /// Per-tab visual x-offset in px (Chrome-style drag/slide animation):
    /// drawn at `slot_x + offset`, eased to 0 by the tick. The dragged tab
    /// is pointer-driven instead; a displaced neighbor starts at ±slot so
    /// it slides to the vacated position.
    pub tab_anim: Vec<f32>,
    /// Tab click & drag reorder in flight, if any.
    pub(crate) tab_drag: Option<TabDrag>,
    /// Open tab context menu, if any.
    pub tab_menu: Option<TabMenu>,
    /// Open editor context menu (right-click), if any.
    pub editor_menu: Option<EditorMenu>,
    /// System-clipboard seam (src/clipboard.rs): `None` until the first
    /// use lazily creates the arboard backend — a display may not exist,
    /// and creation is only attempted from user actions. Tests swap in a
    /// stub; a real backend that fails at use time surfaces as a toast.
    clipboard: Option<Box<dyn crate::clipboard::ClipboardBackend + Send>>,
    /// Last known window-relative pointer position (drives the tab menu
    /// anchor and the drag threshold math).
    window_cursor: Option<Point>,
    /// Which pane receives keys.
    pub focus: Focus,
    /// Editor pane collapsed: the diagram takes the full width.
    pub editor_collapsed: bool,
    /// Space was pressed awaiting the leader key (Space+e).
    space_leader: bool,
    /// Persisted preferences (vim/Regular) and the recent-files list.
    pub config: crate::config::Config,
    /// Animation clock in seconds; drives the hover glow's breathing.
    pub anim_clock: f32,
    /// Instant of the last Tick, for the real elapsed delta.
    last_tick: Option<std::time::Instant>,
    /// Mouse interaction in flight.
    pub drag: Drag,
    /// Last known cursor position over the diagram canvas (local px).
    pub canvas_cursor: Option<Point>,
    /// Table or column row that was clicked (no drag) — the editor jumps
    /// to its definition line (the column's own, else the table's name).
    pub pending_jump: Option<PendingJump>,
    /// Quit requested (window close Task follows).
    pub should_quit: bool,
    /// When the last ctrl+d/u scroll chord fired: a bare d/u right after
    /// it is the chord's roll-out (ctrl released a few ms early, or the
    /// autorepeat tail), not a vim key — see `editor_key`.
    last_scroll_chord: Option<std::time::Instant>,
    /// Status-bar toast.
    pub message: Option<String>,
    /// Open completion popup (insert mode) over the active buffer.
    pub completion: Option<Completion>,
    /// Editor-pane hover position (local px) for error tooltips.
    pub editor_hover: Option<Point>,
    /// Help overlay visibility.
    pub show_help: bool,
    /// Open record-view overlay (a centered modal above everything).
    pub record_view: Option<RecordView>,
    /// Pane split fraction (editor share), driven by divider drags.
    pub split: f32,
    /// Window size for split math.
    pub window_size: (f32, f32),
}

impl Default for App {
    fn default() -> Self {
        Self::with_source(dbml_ui::SAMPLE_DBML.to_string())
    }
}

impl App {
    /// A bare app around the given tabs (all global state at its defaults).
    fn shell(tabs: Vec<Tab>) -> Self {
        Self {
            tabs,
            active: 0,
            tab_drag: None,
            tab_anim: Vec::new(),
            tab_menu: None,
            editor_menu: None,
            clipboard: None,
            window_cursor: None,
            focus: Focus::Editor,
            editor_collapsed: false,
            space_leader: false,
            config: crate::config::Config::default(),
            anim_clock: 0.0,
            last_tick: None,
            drag: Drag::None,
            canvas_cursor: None,
            pending_jump: None,
            should_quit: false,
            last_scroll_chord: None,
            message: None,
            completion: None,
            editor_hover: None,
            show_help: false,
            record_view: None,
            split: 0.4,
            window_size: (1400.0, 900.0),
        }
    }

    pub fn with_source(source: String) -> Self {
        Self::shell(vec![Tab::with_source(source)])
    }

    // ------------------------- accessors -------------------------
    //
    // The active tab carries all per-document state; these shims keep the
    // call sites in app/view/canvas readable (`app.editor()`, ...).

    /// The active tab (all per-document state lives there).
    pub fn cur(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// The active tab, mutably.
    pub fn cur_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// The active tab's vim editor.
    pub fn editor(&self) -> &Editor {
        &self.cur().editor
    }

    /// The active tab's vim editor, mutably.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.cur_mut().editor
    }

    /// The active tab's source.
    pub fn source(&self) -> &str {
        &self.cur().source
    }

    /// The active tab's diagram layout.
    pub fn diagram(&self) -> &Layout {
        &self.cur().diagram
    }

    /// Whether the active tab has a document open (false on a start tab).
    pub fn doc_open(&self) -> bool {
        self.cur().doc_open
    }

    /// The active tab's backing file, if any.
    pub fn file(&self) -> Option<&std::path::PathBuf> {
        self.cur().file.as_ref()
    }

    /// Window title: the active tab's file when one is open.
    pub fn window_title(&self) -> String {
        match self.file() {
            Some(f) => format!("dbml-ui — {}", f.display()),
            None => "dbml-ui".to_string(),
        }
    }

    // ------------------------ tab actions ------------------------

    /// Chrome semantics for closing `idx`: the tab to the right of it
    /// becomes active, else the last one; closing a background tab leaves
    /// the active choice alone.
    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            // the window stays alive: the last tab becomes a fresh start
            self.tabs[0] = Tab::blank();
            self.active = 0;
            self.after_tab_switch();
            return;
        }
        self.tabs.remove(idx);
        match idx.cmp(&self.active) {
            std::cmp::Ordering::Less => self.active -= 1,
            std::cmp::Ordering::Equal => self.active = self.active.min(self.tabs.len() - 1),
            std::cmp::Ordering::Greater => {}
        }
        self.after_tab_switch();
    }

    /// Close every tab; one fresh start-screen tab remains.
    pub fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.tabs.push(Tab::blank());
        self.active = 0;
        self.after_tab_switch();
    }

    /// A fresh start-screen tab (the + button / Ctrl+T).
    pub fn new_tab(&mut self) {
        self.tabs.push(Tab::blank());
        self.activate_tab(self.tabs.len() - 1);
    }

    /// Make `idx` the active tab (swaps all per-tab state in).
    pub fn activate_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx == self.active {
            return;
        }
        self.active = idx;
        self.after_tab_switch();
    }

    /// Globals that belonged to the previously active tab: reset them so
    /// stale buffer/diagram references never leak across a switch. (Per-tab
    /// state swaps itself; hover animation lives on the Tab with the edge
    /// indices it indexes.)
    fn after_tab_switch(&mut self) {
        self.completion = None;
        self.editor_hover = None;
        self.pending_jump = None;
        self.drag = Drag::None;
        self.tab_menu = None;
        self.editor_menu = None;
        self.tab_drag = None;
        self.space_leader = false;
        // the record view indexes the previous tab's schema nodes
        self.record_view = None;
    }

    /// A tab was pressed: activate it and begin click & drag tracking.
    fn tab_pressed(&mut self, idx: usize) {
        self.tab_menu = None;
        self.activate_tab(idx);
        let press_x = self
            .window_cursor
            .map(|p| p.x - STRIP_LEFT)
            .unwrap_or(idx as f32 * TAB_SLOT_W);
        self.tab_drag = Some(TabDrag {
            from: idx,
            grab_dx: press_x - idx as f32 * TAB_SLOT_W,
            moved: false,
            x: press_x,
        });
    }

    /// The pointer moved while a tab drag is in flight. Past the threshold
    /// the dragged tab follows the pointer (drawn via its `tab_anim`
    /// offset); when its edge crosses a neighboring slot's midpoint the
    /// two swap, and the displaced neighbor starts a slot away so it
    /// slides animatedly to the vacated position. `x` is strip-relative.
    fn tab_drag_moved(&mut self, x: f32) {
        let Some(drag) = self.tab_drag.as_mut() else {
            return;
        };
        drag.x = x;
        if !drag.moved {
            // pointer travel from the press point (grab_dx + the pressed
            // slot's origin reconstructs the press x)
            if (x - (drag.grab_dx + drag.from as f32 * TAB_SLOT_W)).abs()
                < TAB_DRAG_THRESHOLD
            {
                return;
            }
            drag.moved = true;
        }
        if self.tab_anim.len() < self.tabs.len() {
            self.tab_anim.resize(self.tabs.len(), 0.0);
        }
        let n = self.tabs.len();
        // the dragged tab's left edge, anchored to the POINTER (not the
        // slot): continuous across swaps, so the same pointer position can
        // never re-trigger the reverse swap (that oscillation made tabs
        // overlap and appear to vanish)
        let vx = (x - drag.grab_dx).clamp(0.0, (n - 1) as f32 * TAB_SLOT_W);
        // Chrome's swap rule: the dragged tab's leading edge crossing the
        // neighbor slot's midpoint switches the two. The two thresholds
        // differ by SLOT - TAB_W (the gap), giving natural hysteresis.
        if drag.from + 1 < n
            && vx + TAB_VISUAL_W >= (drag.from as f32 + 1.5) * TAB_SLOT_W
        {
            let j = drag.from + 1;
            move_element(&mut self.tabs, drag.from, j);
            self.active = j; // the dragged tab (was active) follows
            self.tab_anim[j] = 0.0;
            self.tab_anim[drag.from] = TAB_SLOT_W; // displaced: slides left
            drag.from = j;
        } else if drag.from > 0 && vx <= (drag.from as f32 - 0.5) * TAB_SLOT_W {
            let j = drag.from - 1;
            move_element(&mut self.tabs, drag.from, j);
            self.active = j;
            self.tab_anim[j] = 0.0;
            self.tab_anim[drag.from] = -TAB_SLOT_W; // displaced: slides right
            drag.from = j;
        }
        self.tab_anim[drag.from] = vx - drag.from as f32 * TAB_SLOT_W;
    }

    /// Ease every non-dragged tab's visual offset back to its slot.
    fn tick_tab_anim(&mut self) {
        let dragging = self.tab_drag.as_ref().map(|d| d.from);
        for (i, off) in self.tab_anim.iter_mut().enumerate() {
            if Some(i) == dragging {
                continue; // the dragged tab is pointer-driven
            }
            *off *= 0.72;
            if off.abs() < 0.5 {
                *off = 0.0;
            }
        }
    }

    /// Right-press on a tab: open its context menu anchored at the pointer.
    fn tab_menu_open(&mut self, idx: usize) {
        let at = self.window_cursor.unwrap_or(Point::new(
            STRIP_LEFT + idx as f32 * TAB_SLOT_W,
            0.0,
        ));
        self.tab_menu = Some(TabMenu { tab: idx, at });
    }

    // ------------------------ clipboard ------------------------
    //
    // Everything here is failure-tolerant: arboard needs a display, and
    // headless sessions (CI, SSH) must degrade to toasts, not panics.

    /// The system clipboard, creating the real backend on first use. None
    /// when no system clipboard is reachable; callers degrade.
    fn clipboard(&mut self) -> Option<&mut Box<dyn crate::clipboard::ClipboardBackend + Send>> {
        if self.clipboard.is_none() {
            self.clipboard = crate::clipboard::system_backend()
                .ok()
                .map(|b| Box::new(b) as Box<dyn crate::clipboard::ClipboardBackend + Send>);
        }
        self.clipboard.as_mut()
    }

    /// Current system-clipboard text: None when the clipboard is empty,
    /// unreachable or the read failed. (Menu Paste's enable flag and the
    /// Ctrl+V paths both funnel through here.)
    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard()?.get_text().ok().filter(|t| !t.is_empty())
    }

    /// Push `text` to the system clipboard; Err carries the reason (the
    /// caller turns it into a status-bar toast).
    fn copy_to_system(&mut self, text: &str) -> Result<(), String> {
        match self.clipboard() {
            Some(cb) => cb.set_text(text.to_string()),
            None => Err("no system clipboard".to_string()),
        }
    }

    /// Ctrl+C / menu Copy: the selection into the register and the system
    /// clipboard; the selection itself STAYS (a follow-up paste replaces
    /// it). A no-op without a selection.
    fn copy_selection(&mut self) {
        let Some(t) = self.editor_mut().buf.selection_text() else {
            return;
        };
        self.editor_mut()
            .set_register(t.split('\n').map(str::to_string).collect());
        self.message = Some(match self.copy_to_system(&t) {
            Ok(()) => format!("copied {} chars", t.chars().count()),
            Err(e) => format!("clipboard: {e}"),
        });
    }

    /// Ctrl+X / menu Cut: the selection into the register and the system
    /// clipboard, then deleted from the buffer.
    fn cut_selection(&mut self) {
        let Some(t) = self.editor_mut().buf.cut_selection() else {
            return;
        };
        self.editor_mut()
            .set_register(t.split('\n').map(str::to_string).collect());
        self.message = Some(match self.copy_to_system(&t) {
            Ok(()) => format!("cut {} chars", t.chars().count()),
            Err(e) => format!("clipboard: {e}"),
        });
        self.after_edit(editor::Action::Changed);
        self.follow_cursor_horizontal();
        self.keep_cursor_visible();
    }

    /// Ctrl+V / menu Paste: the system clipboard at the cursor (an active
    /// selection is replaced by the first inserted line). Falls back to
    /// the internal register when no system clipboard is reachable.
    fn paste_from_system(&mut self) {
        let text = match self.clipboard_text() {
            Some(t) => t,
            None => {
                let reg = self.editor().register_lines();
                if reg.is_empty() {
                    self.message = Some("nothing to paste".to_string());
                    return;
                }
                reg.join("\n")
            }
        };
        self.paste_text(&text);
        self.after_edit(editor::Action::Changed);
        self.follow_cursor_horizontal();
        self.keep_cursor_visible();
    }

    /// Insert `text` at the cursor as a multi-line paste: '\n' becomes
    /// real line breaks ('\r\n' endings are normalized), and the first
    /// inserted op replaces an active selection (Buffer edits clear it).
    fn paste_text(&mut self, text: &str) {
        for (i, line) in text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .enumerate()
        {
            if i > 0 {
                self.editor_mut().buf.insert_newline();
            }
            if !line.is_empty() {
                self.editor_mut().buf.insert_str(line);
            }
        }
    }

    /// Toolbar Undo / Ctrl+Z: ONE undo step on the active buffer — the
    /// same history stack vim's `u` walks, so buttons and shortcuts agree
    /// in every mode (single step; `[count]u` stays a vim normal-mode
    /// feature). Works from any focus: the button path skips key dispatch.
    fn undo_step(&mut self) {
        self.undo_redo_common();
        if self.editor_mut().buf.undo() {
            self.after_edit(editor::Action::Changed);
            self.follow_cursor_horizontal();
            self.keep_cursor_visible();
        }
    }

    /// Toolbar Redo / Ctrl+Y / Ctrl+Shift+Z: the redo twin of `undo_step`.
    fn redo_step(&mut self) {
        self.undo_redo_common();
        if self.editor_mut().buf.redo() {
            self.after_edit(editor::Action::Changed);
            self.follow_cursor_horizontal();
            self.keep_cursor_visible();
        }
    }

    /// Bookkeeping every undo/redo entry does before stepping: the popup
    /// and hover tooltip dismiss like on any other edit (an undo leaves
    /// the popup's prefix stale; the tooltip may sit on erased text).
    fn undo_redo_common(&mut self) {
        self.completion = None;
        self.editor_hover = None;
    }

    /// Right-press in the editor pane: open its context menu anchored at
    /// the pointer, snapshotting item availability (a failed clipboard
    /// read disables Paste — never a panic).
    fn editor_menu_open(&mut self, local: Point) {
        let at = self.window_cursor.unwrap_or(Point::new(
            local.x,
            local.y + crate::view::EDITOR_CANVAS_TOP,
        ));
        let menu = EditorMenu {
            at,
            has_selection: self.editor().buf.selection_anchor().is_some(),
            clipboard_has_text: self.clipboard_text().is_some(),
        };
        self.editor_menu = Some(menu);
    }

    // ---------------- regular-mode mouse selection ----------------

    /// A press in the editor pane while in Regular mode: anchor the
    /// selection at the char under the pointer and start the drag
    /// (MouseMovedWindow extends, MouseReleased finalizes).
    fn editor_select_pressed(&mut self, local: Point) {
        let pos = crate::canvas::editor::px_to_pos(self, local);
        self.editor_mut().buf.cancel_selection();
        self.editor_mut()
            .buf
            .move_cursor(editor::buffer::Move::Jump(pos.row, pos.col));
        self.editor_mut().buf.start_selection();
        self.drag = Drag::EditorSelect;
    }

    /// Selection-drag extension: move the cursor to the buffer position
    /// under the (canvas-local) pointer. Points outside the pane (the
    /// drag left it) clamp to the buffer edges via px_to_pos.
    fn editor_select_moved(&mut self, local: Point) {
        let pos = crate::canvas::editor::px_to_pos(self, local);
        self.editor_mut()
            .buf
            .move_cursor(editor::buffer::Move::Jump(pos.row, pos.col));
        self.follow_cursor_horizontal();
    }

    /// Selection-drag end: finalize. A click that never extended the
    /// selection (anchor still on the cursor) collapses it — a plain
    /// click places the caret and selects nothing.
    fn editor_select_released(&mut self) {
        self.drag = Drag::None;
        let ed = self.editor_mut();
        if ed.buf.selection_anchor().is_some_and(|a| a == ed.buf.pos()) {
            ed.buf.cancel_selection();
        }
        self.keep_cursor_visible();
    }

    fn canvas_pressed(&mut self, p: Point) {
        use crate::canvas::diagram as dia;
        self.canvas_cursor = Some(p);
        // The records affordance wins over the generic table press: it
        // opens the record view instead of starting a table drag (which
        // would end in a jump-to-editor click on release).
        if let Some(b) = dia::records_icon_hit(&self.cur().diagram, self.cur().zoom, self.cur().pan, p)
        {
            let node = b.node;
            self.drag = Drag::None;
            self.open_records(node);
            return;
        }
        match hit_test(&self.cur().diagram, self.cur().zoom, self.cur().pan, p) {
            Some(b) => {
                let (cx, cy) = to_cell_f(p, self.cur().zoom, self.cur().pan);
                self.drag = Drag::Table {
                    title: b.title.clone(),
                    grab: (cx - b.x as f32, cy - b.y as f32),
                    press: p,
                    moved: false,
                };
            }
            None => self.drag = Drag::Pan { anchor: p },
        }
    }

    /// Open the record view for a table node, when that node is a table
    /// with records (indices can arrive stale after an edit).
    fn open_records(&mut self, node: usize) {
        if self
            .cur()
            .diagram
            .boxes
            .iter()
            .any(|b| b.node == node && b.records.is_some())
        {
            self.record_view = Some(RecordView { node, follow: None });
        }
    }

    /// Recompute the highlighted edges for a pointer position: the line
    /// under the pointer plus the hovered row's connections (dbdiagram
    /// lights up every ref touching the hovered column).
    fn update_hover(&mut self, p: Point) {
        use crate::canvas::diagram as dia;
        let mut active: Vec<usize> = Vec::new();
        if let Some(ei) = dia::edge_hit(&self.cur().diagram, self.cur().zoom, self.cur().pan, p, 7.0) {
            active.push(ei);
        }
        if let Some((b, row)) = dia::row_at(&self.cur().diagram, self.cur().zoom, self.cur().pan, p) {
            for ei in self.cur().diagram.edges_touching(b.node, row) {
                if !active.contains(&ei) {
                    active.push(ei);
                }
            }
        }
        self.cur_mut().hover_edges = active;
    }

    fn canvas_moved(&mut self, p: Point) {
        self.canvas_cursor = Some(p);
        // Frozen during a drag (pan/table): re-highlighting while the
        // canvas moves under the pointer reads as flicker.
        if matches!(self.drag, Drag::None) {
            self.update_hover(p);
        }
        match self.drag.clone() {
            Drag::Pan { anchor } => {
                self.cur_mut().pan = self.cur().pan + Vector::new(p.x - anchor.x, p.y - anchor.y);
                self.drag = Drag::Pan { anchor: p };
            }
            Drag::Table {
                title,
                grab,
                press,
                moved,
            } => {
                let moved = moved || p.distance(press) > 3.0;
                let (cx, cy) = to_cell_f(p, self.cur().zoom, self.cur().pan);
                // free positioning: boxes may go above/left of the origin
                let pos = (
                    ((cx - grab.0).round() as i32),
                    ((cy - grab.1).round() as i32),
                );
                let src = self.source().to_string();
                self.cur_mut().table_pos.insert(title.clone(), pos);
                self.cur_mut().set_source(&src);
                self.drag = Drag::Table {
                    title,
                    grab,
                    press,
                    moved,
                };
            }
            Drag::Divider | Drag::EditorSelect | Drag::None => {}
        }
    }

    fn canvas_released(&mut self, p: Point) {
        use crate::canvas::diagram as dia;
        self.canvas_cursor = Some(p);
        if let Drag::Table {
            title,
            press,
            moved,
            ..
        } = &self.drag
        {
            if !*moved && p.distance(*press) <= 3.0 {
                // A click, not a drag. The layout did not move (only a real
                // drag re-lays-out), so the press point still resolves: a
                // column row jumps to THAT column's definition line, the
                // header band or any other part of the box to the table's
                // name line.
                let d = &self.cur().diagram;
                let span = match dia::row_at(d, self.cur().zoom, self.cur().pan, *press) {
                    Some((b, row)) => Some(b.rows[row].name_span.clone()),
                    None => d
                        .boxes
                        .iter()
                        .find(|b| b.title == *title)
                        .and_then(|b| b.name_span.clone()),
                };
                if let Some(span) = span {
                    self.pending_jump = Some(PendingJump { span });
                }
            }
        }
        // an in-flight Regular-mode selection drag is finalized by
        // MouseReleased (which knows whether the click selected anything)
        if !matches!(self.drag, Drag::EditorSelect) {
            self.drag = Drag::None;
        }
        // the highlight froze while dragging; catch up to the pointer
        self.update_hover(p);
    }

    /// Zoom continuously, keeping the point under the cursor stationary.
    fn wheel(&mut self, notches: f32) {
        let old = self.cur().zoom;
        let new = (old * (notches * 0.22f32).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        if let Some(p) = self.canvas_cursor {
            let s = new / old;
            self.cur_mut().pan = Vector::new(p.x - (p.x - self.cur().pan.x) * s, p.y - (p.y - self.cur().pan.y) * s);
        }
        self.cur_mut().zoom = new;
    }

    // ----------------------- diagram actions -----------------------

    /// Auto-arrange (dbdiagram's Left-right): ELK-layered positions for
    /// every table (see `layout::auto_arrange`), written into `table_pos`
    /// — the same map user drags fill — and re-laid out via the usual
    /// set_source re-parse. The routes move out from under the pointer,
    /// so the hover highlight resets with them.
    fn auto_arrange(&mut self) {
        let src = self.source().to_string();
        let schema = model::build(&dbml::parser::parse(&src).doc);
        let pos = layout::auto_arrange(&self.cur().diagram.boxes, &schema);
        self.cur_mut().table_pos = pos;
        self.cur_mut().set_source(&src);
        self.cur_mut().edge_anim = vec![0.0; self.cur().diagram.edges.len()];
        self.cur_mut().hover_edges.clear();
        self.message = Some("auto-arranged".to_string());
    }

    /// Zoom to fit: scale + pan so the whole diagram (every box plus a
    /// uniform margin) sits inside the visible diagram canvas. The scale
    /// clamps to the wheel's range; the pan puts the content's top-left
    /// corner at the margin — the same composition `to_px` and the
    /// wheel's cursor-anchored zoom use (screen = (cell + margin) * size
    /// + pan).
    fn zoom_to_fit(&mut self) {
        let boxes = &self.cur().diagram.boxes;
        let Some(first) = boxes.first() else {
            return; // nothing to fit (start screen, empty schema)
        };
        let (mut min_x, mut min_y, mut max_r, mut max_b) =
            (first.x, first.y, first.right(), first.bottom());
        for b in boxes.iter().skip(1) {
            min_x = min_x.min(b.x);
            min_y = min_y.min(b.y);
            max_r = max_r.max(b.right());
            max_b = max_b.max(b.bottom());
        }
        let (cw, ch) = self.diagram_canvas_size();
        let avail_w = (cw - 2.0 * FIT_MARGIN_PX).max(1.0);
        let avail_h = (ch - 2.0 * FIT_MARGIN_PX).max(1.0);
        // content size in px at zoom 1 (cell height 1px, width CELL_ASPECT)
        let content_w = ((max_r - min_x) as f32 * CELL_ASPECT).max(1.0);
        let content_h = ((max_b - min_y) as f32).max(1.0);
        let scale = (avail_w / content_w)
            .min(avail_h / content_h)
            .clamp(ZOOM_MIN, ZOOM_MAX);
        let pan = Vector::new(
            FIT_MARGIN_PX - (min_x as f32 + MARGIN_CELLS) * CELL_ASPECT * scale,
            FIT_MARGIN_PX - (min_y as f32 + MARGIN_CELLS) * scale,
        );
        self.cur_mut().zoom = scale;
        self.cur_mut().pan = pan;
    }

    /// Diagram pane width in px: the window minus the editor (or start)
    /// pane's share of the flush pane row, or the whole width when the
    /// editor pane is collapsed. Mirrors `editor_pane_width`'s integer
    /// FillPortion math.
    fn diagram_pane_width(&self) -> f32 {
        if self.editor_collapsed {
            return self.window_size.0.max(60.0);
        }
        (self.window_size.0.max(60.0) - self.editor_pane_width()).max(60.0)
    }

    /// Visible diagram canvas size in px: the pane minus its 1px padding
    /// on each side and the chrome above/below it (view::DIAGRAM_CHROME).
    fn diagram_canvas_size(&self) -> (f32, f32) {
        (
            (self.diagram_pane_width() - 2.0).max(1.0),
            (self.window_size.1 - crate::view::DIAGRAM_CHROME).max(1.0),
        )
    }
}

pub fn subscription(_app: &App) -> iced::Subscription<Message> {
    let mouse = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::MouseMovedWindow(position.x, position.y))
        }
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(Message::MouseReleased)
        }
        _ => None,
    });
    // Full keyboard events, not on_key_press: the logical key it hands out
    // is unmodified (Shift+a arrives as 'a'), which made capitals and
    // shifted symbols impossible. KeyPressed.modified_key has shift and
    // CapsLock applied (everything but Ctrl), and `text` is what typing
    // actually produced.
    let keys = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key,
            modified_key,
            modifiers,
            ..
        }) => {
            let code = match &modified_key {
                iced::keyboard::Key::Character(c) => c.chars().next().map(KeyCode::Char),
                iced::keyboard::Key::Named(n) => named_code(n),
                _ => None,
            }?;
            // a Ctrl chord must keep the UNMODIFIED character (Ctrl+r, not Ctrl+R)
            let code = if modifiers.control() {
                match &key {
                    iced::keyboard::Key::Character(c) => {
                        c.chars().next().map(KeyCode::Char).unwrap_or(code)
                    }
                    _ => code,
                }
            } else {
                code
            };
            let m = KeyModifiers::new(modifiers.control(), modifiers.shift(), modifiers.alt());
            Some(Message::KeyPressed(code, m))
        }
        _ => None,
    });
    // Ticks drive the scroll animation and the edge-highlight easing +
    // breathing glow: hot while scrolling, while something is hovered
    // (the glow keeps breathing), until every fade-out has finished, and
    // while the vim block cursor is blinking (canvas::editor::BlinkState).
    let tick = if _app.cur().scroll_anim.is_some()
        || !_app.cur().hover_edges.is_empty()
        || _app.cur().edge_anim.iter().any(|&t| t > 0.0)
        || _app.tab_drag.is_some()
        || _app.tab_anim.iter().any(|o| o.abs() > 0.5)
        || crate::canvas::editor::block_cursor_blinking(_app)
    {
        iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick)
    } else {
        iced::Subscription::none()
    };
    iced::Subscription::batch([mouse, keys, tick, window_resize()])
}

fn named_code(n: &iced::keyboard::key::Named) -> Option<KeyCode> {
    Some(match n {
        iced::keyboard::key::Named::Escape => KeyCode::Esc,
        iced::keyboard::key::Named::Enter => KeyCode::Enter,
        iced::keyboard::key::Named::Space => KeyCode::Space,
        iced::keyboard::key::Named::Backspace => KeyCode::Backspace,
        iced::keyboard::key::Named::Tab => KeyCode::Tab,
        iced::keyboard::key::Named::ArrowLeft => KeyCode::Left,
        iced::keyboard::key::Named::ArrowRight => KeyCode::Right,
        iced::keyboard::key::Named::ArrowUp => KeyCode::Up,
        iced::keyboard::key::Named::ArrowDown => KeyCode::Down,
        iced::keyboard::key::Named::Home => KeyCode::Home,
        iced::keyboard::key::Named::End => KeyCode::End,
        iced::keyboard::key::Named::PageUp => KeyCode::PageUp,
        iced::keyboard::key::Named::PageDown => KeyCode::PageDown,
        iced::keyboard::key::Named::Delete => KeyCode::Delete,
        _ => return None,
    })
}

fn window_resize() -> iced::Subscription<Message> {
    iced::window::resize_events().map(|(id, size)| {
        let _ = id;
        Message::WindowResized(size.width, size.height)
    })
}

/// Result of the native file-dialog subprocess.
enum PickOutcome {
    Chosen(String),
    Cancelled,
    /// no dialog tool on the system
    NoDialog,
}

/// Blocking zenity/kdialog save dialog. Must run off the UI thread.
fn pick_save_path_blocking() -> PickOutcome {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let start = format!("{home}/diagram.dbml");
    for (tool, args) in [
        (
            "zenity",
            vec![
                "--save",
                "--confirm-overwrite",
                "--filename",
                &start,
                "--title=Save DBML file",
            ],
        ),
        ("kdialog", vec!["--getsavefilename", &start, "*.dbml"]),
        (
            "osascript",
            vec![
                "-e",
                "POSIX path of (choose file name with prompt \"Save DBML file\")",
            ],
        ),
        (
            "powershell",
            vec![
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; \
                 $f = New-Object System.Windows.Forms.SaveFileDialog; \
                 $f.Filter = 'DBML (*.dbml)|*.dbml'; \
                 if ($f.ShowDialog() -eq 'OK') { $f.FileName }",
            ],
        ),
    ] {
        let Ok(out) = std::process::Command::new(tool)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return PickOutcome::Chosen(p);
            }
        }
        return PickOutcome::Cancelled;
    }
    PickOutcome::NoDialog
}

/// Blocking zenity/kdialog file picker. Must run off the UI thread.
fn pick_file_blocking() -> PickOutcome {
    let run = |cmd: &str, args: &[&str]| -> Option<std::process::Output> {
        std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
    };
    for (tool, args) in [
        ("zenity", vec!["--file-selection", "--title=Open DBML file"]),
        ("kdialog", vec!["--getopenfilename", ".", "*.dbml"]),
    ] {
        if let Some(out) = run(tool, &args) {
            if out.status.success() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !p.is_empty() {
                    return PickOutcome::Chosen(p);
                }
            }
            // the tool ran: an empty/cancelled dialog is a cancel
            return PickOutcome::Cancelled;
        }
    }
    PickOutcome::NoDialog
}

pub fn update(app: &mut App, message: Message) -> iced::Task<Message> {
    match message {
        // native file dialog (zenity/kdialog subprocess off the render
        // loop; the GTK rfd crate broke iced's event loop entirely)
        Message::OpenFilePicker => {
            let (tx, rx) = smol::channel::bounded(1);
            std::thread::spawn(move || {
                let outcome = pick_file_blocking();
                let _ = smol::block_on(tx.send(outcome));
            });
            return iced::Task::future(async move {
                match rx.recv().await {
                    Ok(PickOutcome::Chosen(p)) => Message::OpenRecent(p),
                    Ok(PickOutcome::NoDialog) => Message::PickerUnavailable,
                    _ => Message::PickerDismissed,
                }
            });
        }
        Message::PickerDismissed => {}
        // Ctrl+O opens the picker from either editing mode
        Message::KeyPressed(code, mods)
            if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('o') =>
        {
            return update(app, Message::OpenFilePicker);
        }
        // Ctrl+N (and the toolbar button) opens the save dialog
        Message::KeyPressed(code, mods)
            if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('n') =>
        {
            return update(app, Message::SaveAsPrompt);
        }
        Message::SaveAsPrompt => {
            let (tx, rx) = smol::channel::bounded(1);
            std::thread::spawn(move || {
                let outcome = pick_save_path_blocking();
                let _ = smol::block_on(tx.send(outcome));
            });
            return iced::Task::future(async move {
                match rx.recv().await {
                    Ok(PickOutcome::Chosen(p)) => Message::SaveAs(p),
                    Ok(PickOutcome::NoDialog) => Message::PickerUnavailable,
                    _ => Message::PickerDismissed,
                }
            });
        }
        Message::SaveAs(path) => {
            app.cur_mut().file = Some(std::path::PathBuf::from(&path));
            if let Err(e) = app.save_file(None) {
                app.message = Some(format!("save failed: {e}"));
            }
        }
        Message::FormatNow => app.format_now(),
        Message::ToggleFmtOnSave => {
            app.config.fmt_on_save = !app.config.fmt_on_save;
            app.config.save();
            app.message = Some(if app.config.fmt_on_save {
                "format on save: on".to_string()
            } else {
                "format on save: off".to_string()
            });
        }
        m => handle_message(app, m),
    }
    handle_message_finish(app)
}

fn handle_message_finish(app: &mut App) -> iced::Task<Message> {
    if app.should_quit {
        return iced::exit();
    }
    iced::Task::none()
}

fn handle_message(app: &mut App, message: Message) {
    match message {
        Message::CanvasPressed(p) => app.canvas_pressed(p),
        Message::CanvasMoved(p) => app.canvas_moved(p),
        Message::CanvasReleased(p) => {
            app.canvas_released(p);
            app.consume_pending_jump();
        }
        Message::Wheel(n) => app.wheel(n),
        Message::AutoArrange => app.auto_arrange(),
        Message::ZoomToFit => app.zoom_to_fit(),
        Message::KeyPressed(code, mods) => app.key_pressed(code, mods),
        Message::EditorScrolled { v, h } => {
            // instant (neoscroll default) and takes over any animation —
            // either axis counts as a user scroll
            app.cur_mut().scroll_anim = None;
            app.cur_mut().scroll_target = None;
            if v != 0.0 {
                app.cur_mut().editor_scroll =
                    (app.cur().editor_scroll + v).clamp(0.0, app.max_scroll());
            }
            if h != 0.0 {
                // h is in notches; the horizontal offset lives in px, one
                // notch advancing one character cell
                let h_px = h * crate::canvas::editor::char_width();
                app.cur_mut().editor_hscroll =
                    (app.cur().editor_hscroll + h_px).clamp(0.0, app.max_hscroll());
            }
        }
        Message::DividerPressed => app.drag = Drag::Divider,
        Message::MouseMovedWindow(x, y) => {
            app.window_cursor = Some(Point::new(x, y));
            // a tab drag follows the pointer window-wide (Chrome keeps
            // reordering while you drag, then drops on any release)
            if app.tab_drag.is_some() {
                app.tab_drag_moved(x - STRIP_LEFT);
            }
            // a Regular-mode selection drag also follows the pointer
            // window-wide: dragging below the pane or into the diagram
            // clamps to the buffer edges instead of stopping (the canvas
            // top offset converts window px to the canvas-local coords
            // px_to_pos expects — see EDITOR_CANVAS_TOP)
            if app.drag == Drag::EditorSelect {
                app.editor_select_moved(Point::new(x, y - crate::view::EDITOR_CANVAS_TOP));
            }
            if x < app.editor_pane_width() + 30.0 && app.drag == Drag::None {
                app.cur_mut().hover_edges.clear();
            }
            if app.drag == Drag::Divider {
                const PAD: f32 = 0.0; // window content is flush, no outer padding
                let usable = (app.window_size.0 - 2.0 * PAD).max(1.0);
                app.split = ((x - PAD) / usable).clamp(0.15, 0.85);
            }
        }
        Message::MouseReleased => {
            if app.drag == Drag::Divider {
                app.drag = Drag::None;
            }
            // a Regular-mode selection drag finalizes: a real drag keeps
            // its selection, a plain click collapses it
            if app.drag == Drag::EditorSelect {
                app.editor_select_released();
            }
            // drop: the dragged tab glides from its released pose to its
            // slot via the same easing as the displaced neighbors
            if let Some(drag) = app.tab_drag.take() {
                if app.tab_anim.len() < app.tabs.len() {
                    app.tab_anim.resize(app.tabs.len(), 0.0);
                }
                let vx = (drag.x - drag.grab_dx)
                    .clamp(0.0, (app.tabs.len() - 1) as f32 * TAB_SLOT_W);
                app.tab_anim[drag.from] = vx - drag.from as f32 * TAB_SLOT_W;
            }
        }
        Message::WindowResized(w, h) => app.window_size = (w, h),
        Message::CompletionPicked(idx) => app.completion_accept(idx),
        Message::EditorHover(p) => app.editor_hover = p,
        Message::ToggleEditor => app.toggle_editor(),
        Message::ToggleVim => app.toggle_vim(),
        Message::OpenRecent(path) => {
            if let Err(e) = app.open_file(&path) {
                app.message = Some(format!("cannot open {path}: {e}"));
            }
        }
        Message::OpenFilePicker | Message::PickerDismissed => {} // handled in update
        Message::SaveAsPrompt | Message::SaveAs(_) => {}         // handled in update
        Message::FormatNow => {}                                 // handled in update
        Message::ToggleFmtOnSave => {}                           // handled in update
        Message::PickerUnavailable => {
            // no zenity/kdialog: fall back to typing the path in the cmdline
            app.editor_mut().cmdline = Some(editor::CmdLine {
                kind: ':',
                input: "o ".to_string(),
            });
            app.focus = Focus::Editor;
        }
        Message::NewDiagram => app.new_diagram(),
        Message::NewTab => app.new_tab(),
        Message::CloseTab(idx) => app.close_tab(idx),
        Message::TabPressed(idx) => app.tab_pressed(idx),
        Message::TabMenu(idx) => app.tab_menu_open(idx),
        Message::TabMenuClose => {
            let tab = app.tab_menu.as_ref().map(|m| m.tab).unwrap_or(app.active);
            app.close_tab(tab);
        }
        Message::TabMenuCloseAll => app.close_all_tabs(),
        Message::DismissTabMenu => app.tab_menu = None,
        Message::EditorMenu(p) => app.editor_menu_open(p),
        Message::EditorMenuCopy => {
            app.editor_menu = None;
            app.copy_selection();
        }
        Message::EditorMenuCut => {
            app.editor_menu = None;
            app.cut_selection();
        }
        Message::EditorMenuPaste => {
            app.editor_menu = None;
            app.paste_from_system();
        }
        Message::DismissEditorMenu => app.editor_menu = None,
        Message::Undo => app.undo_step(),
        Message::Redo => app.redo_step(),
        Message::OpenRecords(node) => app.open_records(node),
        Message::CloseRecords => app.record_view = None,
        Message::RecordsFollow(node, row) => {
            app.record_view = Some(RecordView {
                node,
                follow: Some(row),
            });
        }
        Message::Tick => {
            // ease every edge's highlight toward its hover target with the
            // real elapsed delta (fade-in ~0.22s, fade-out ~0.30s)
            let now = std::time::Instant::now();
            let dt = app
                .last_tick
                .map_or(0.016, |t| (now - t).as_secs_f32().min(0.1));
            app.last_tick = Some(now);
            app.anim_clock += dt;
            app.tick_tab_anim();
            let hovered: Vec<usize> = app.cur().hover_edges.clone();
            for (i, t) in app.cur_mut().edge_anim.iter_mut().enumerate() {
                let target = if hovered.contains(&i) { 1.0 } else { 0.0 };
                if *t < target {
                    *t = (*t + dt / 0.22).min(target);
                } else {
                    *t = (*t - dt / 0.30).max(target);
                }
            }
            let Some(anim) = app.cur().scroll_anim.clone() else {
                return;
            };
            let y = (anim.started.elapsed().as_secs_f32() / anim.duration).min(1.0);
            let frac = scroll_progress(y);
            let rows = app.editor_mut().buf.line_count() as i32;
            if let Some((from_row, to_row)) = anim.cursor {
                let row = ((from_row as f32 + (to_row as f32 - from_row as f32) * frac).round()
                    as i32)
                    .clamp(0, rows - 1) as usize;
                let col = app.editor_mut().buf.pos().col;
                app.editor_mut()
                    .buf
                    .move_cursor(crate::editor::buffer::Move::Jump(row, col));
            }
            let scroll = (anim.scroll_from + (anim.scroll_to - anim.scroll_from) * frac).round();
            app.cur_mut().editor_scroll = scroll;
            if std::env::var("DBML_UI_DEBUG").is_ok() {
                eprintln!(
                    "DBG tick y={y:.3} scroll={scroll:.0}/{:.0} cursor={}",
                    anim.scroll_to,
                    app.editor_mut().buf.pos().row
                );
            }
            if y >= 1.0 {
                app.cur_mut().editor_scroll = anim.scroll_to;
                if let Some((_, to_row)) = anim.cursor {
                    let col = app.editor_mut().buf.pos().col;
                    app.editor_mut()
                        .buf
                        .move_cursor(crate::editor::buffer::Move::Jump(to_row, col));
                }
                app.cur_mut().scroll_anim = None;
                app.cur_mut().scroll_target = None;
            }
        }
        Message::EditorPressed(p) => {
            // mouse press-drag text selection in EVERY mode (regular,
            // vim normal/visual/insert): a click collapses to a caret at
            // the pointer, a drag extends a selection like gvim's mouse
            app.editor_select_pressed(p);
            app.focus = Focus::Editor;
            app.follow_cursor_horizontal();
        }
    }
}

impl App {
    pub fn is_modified(&self) -> bool {
        self.cur().is_modified()
    }

    /// Fresh app with no document: the side pane shows the start screen
    /// (Open file / New diagram) and is not collapsible.
    pub fn blank_start() -> Self {
        Self::shell(vec![Tab::blank()])
    }

    /// New, unsaved diagram: seed a minimal Project and open the editor.
    pub fn new_diagram(&mut self) {
        let tpl = "Project untitled {\n  database_type: 'PostgreSQL'\n}\n";
        self.cur_mut().table_pos.clear();
        self.cur_mut().file = None;
        self.cur_mut().saved_text = tpl.to_string();
        self.editor_mut().set_text(tpl);
        self.cur_mut().set_source(tpl);
        self.cur_mut().editor_scroll = 0.0;
        self.cur_mut().editor_hscroll = 0.0;
        self.cur_mut().doc_open = true;
        self.editor_collapsed = false;
        self.focus = Focus::Editor;
        self.message = Some("new diagram".to_string());
    }

    /// True when editing is notepad-style (no modal operators).
    pub fn regular_mode(&self) -> bool {
        !self.config.vim
    }

    /// Switch vim <-> Regular editing; persisted. Any vim state in flight
    /// (visual mode, pending operators) resets so nothing leaks across.
    pub fn toggle_vim(&mut self) {
        self.config.vim = !self.config.vim;
        self.config.save();
        self.editor_mut().mode = editor::Mode::Normal;
        self.editor_mut().visual_linewise = false;
        self.editor_mut().buf.cancel_selection();
        self.completion = None;
        self.space_leader = false;
        self.message = Some(if self.config.vim {
            "vim mode".to_string()
        } else {
            "regular mode".to_string()
        });
    }

    /// Regular-mode editor keys: notepad behavior — typing inserts (a
    /// selection is replaced), arrows move, Shift+arrows extend the
    /// selection, no modal operators.
    fn regular_key(&mut self, key: KeyEvent) {
        let m = key.modifiers;
        let sel = self.editor_mut().buf.selection_anchor().is_some();
        let replace_sel = |app: &mut Self| {
            if app.editor_mut().buf.selection_anchor().is_some() {
                let _ = app.editor_mut().buf.cut_selection();
            }
        };
        let mv = |app: &mut Self, k: KeyCode, shift: bool, motion: editor::buffer::Move| {
            let _ = k;
            if shift {
                if !sel {
                    app.editor_mut().buf.start_selection();
                }
                app.editor_mut().buf.move_cursor(motion);
            } else {
                app.editor_mut().buf.cancel_selection();
                app.editor_mut().buf.move_cursor(motion);
            }
        };
        match key.code {
            // Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z: undo/redo on the same stack
            // the toolbar buttons drive (before the typing arm, which only
            // takes unmodified chars, so bare z/y still type)
            KeyCode::Char('z') if m.contains(KeyModifiers::CONTROL) => {
                if m.contains(KeyModifiers::SHIFT) {
                    self.redo_step();
                } else {
                    self.undo_step();
                }
            }
            KeyCode::Char('y') if m.contains(KeyModifiers::CONTROL) => self.redo_step(),
            KeyCode::Char(c)
                if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
            {
                replace_sel(self);
                self.editor_mut().buf.insert_char(c);
                self.after_edit(editor::Action::Changed);
            }
            KeyCode::Space => {
                replace_sel(self);
                self.editor_mut().buf.insert_char(' ');
                self.after_edit(editor::Action::Changed);
            }
            KeyCode::Enter => {
                replace_sel(self);
                self.editor_mut().buf.insert_newline();
                self.after_edit(editor::Action::Changed);
            }
            KeyCode::Backspace => {
                if sel {
                    let _ = self.editor_mut().buf.cut_selection();
                } else {
                    self.editor_mut().buf.delete_char();
                }
                self.after_edit(editor::Action::Changed);
            }
            KeyCode::Delete => {
                if sel {
                    let _ = self.editor_mut().buf.cut_selection();
                } else {
                    self.editor_mut().buf.move_cursor(editor::buffer::Move::Forward);
                    self.editor_mut().buf.delete_char();
                }
                self.after_edit(editor::Action::Changed);
            }
            KeyCode::Left => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::Back,
            ),
            KeyCode::Right => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::Forward,
            ),
            KeyCode::Up => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::Up,
            ),
            KeyCode::Down => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::Down,
            ),
            KeyCode::Home => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::Head,
            ),
            KeyCode::End => mv(
                self,
                key.code,
                m.contains(KeyModifiers::SHIFT),
                editor::buffer::Move::End,
            ),
            KeyCode::PageUp => self.half_page(-1.0),
            KeyCode::PageDown => self.half_page(1.0),
            KeyCode::Esc => self.editor_mut().buf.cancel_selection(),
            KeyCode::Char('a') => {
                // Ctrl+A: select all
                self.editor_mut().buf.cancel_selection();
                self.editor_mut()
                    .buf
                    .move_cursor(editor::buffer::Move::Jump(0, 0));
                self.editor_mut().buf.start_selection();
                let last = self.editor_mut().buf.line_count().saturating_sub(1);
                self.editor_mut()
                    .buf
                    .move_cursor(editor::buffer::Move::Jump(last, usize::MAX));
                self.editor_mut().buf.move_cursor(editor::buffer::Move::End);
            }
            KeyCode::Char('c') => {
                // Ctrl+C: copy the selection (register + system clipboard),
                // keeping it (a follow-up paste replaces it)
                self.copy_selection();
            }
            KeyCode::Char('x') => {
                // Ctrl+X: cut the selection (register + system clipboard)
                self.cut_selection();
            }
            KeyCode::Char('v') => {
                // Ctrl+V: paste the system clipboard at the cursor
                // (register fallback when no system clipboard exists)
                self.paste_from_system();
            }
            _ => {}
        }
        self.follow_cursor_horizontal();
        self.keep_cursor_visible();
    }

    /// Collapse/expand the editor pane (Space+e or the seam chevron).
    /// No-op on the start screen, where the pane holds the Open/New buttons.
    pub fn toggle_editor(&mut self) {
        if !self.doc_open() {
            return;
        }
        self.editor_collapsed = !self.editor_collapsed;
        if self.editor_collapsed && self.focus == Focus::Editor {
            self.focus = Focus::Canvas;
        }
    }

    pub fn open_file(&mut self, path: &str) -> std::io::Result<()> {
        let mut content = std::fs::read_to_string(path)?;
        if path.to_ascii_lowercase().ends_with(".sql") {
            // SQL dump -> DBML via the importer (postgres unless the file
            // name mentions a dialect)
            let lower = path.to_ascii_lowercase();
            let dialect = if lower.contains("mysql") {
                dbml::sql::Dialect::MySQL
            } else if lower.contains("mssql") || lower.contains("sqlserver") {
                dbml::sql::Dialect::MSSQL
            } else if lower.contains("oracle") {
                dbml::sql::Dialect::Oracle
            } else if lower.contains("sqlite") || lower.contains("sqlite3") {
                dbml::sql::Dialect::SQLite
            } else {
                dbml::sql::Dialect::Postgres
            };
            content = dbml::sqldump::import(&content, dialect).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("import: {e}"))
            })?;
        }
        self.cur_mut().table_pos.clear();
        self.cur_mut().file = Some(std::path::PathBuf::from(path));
        self.cur_mut().saved_text = content.clone();
        self.editor_mut().set_text(&content);
        self.cur_mut().set_source(&content);
        self.cur_mut().editor_scroll = 0.0;
        self.cur_mut().editor_hscroll = 0.0;
        self.message = Some(format!(
            "\"{path}\" {}L, {}B",
            content.lines().count(),
            content.len()
        ));
        self.cur_mut().doc_open = true;
        self.config.push_recent(path);
        self.config.save();
        Ok(())
    }

    /// Format the whole buffer canonically (comments are preserved by the
    /// formatter). Skips sources with parse errors; reports why.
    pub fn format_now(&mut self) {
        match dbml::fmt::format_source(self.source()) {
            Some(f) if f != self.source() => {
                let f = if self.source().ends_with('\n') && !f.ends_with('\n') {
                    f + "\n"
                } else {
                    f
                };
                self.editor_mut().buf.replace_all(&f);
                self.cur_mut().set_source(&f);
                self.cur_mut().editor_scroll = 0.0;
                self.cur_mut().editor_hscroll = 0.0;
                self.message = Some("formatted".to_string());
            }
            Some(_) => self.message = Some("already formatted".to_string()),
            None => self.message = Some("format skipped: parse errors".to_string()),
        }
    }

    pub fn save_file(&mut self, path: Option<&str>) -> std::io::Result<()> {
        let target = match path {
            Some(p) => {
                self.cur_mut().file = Some(std::path::PathBuf::from(p));
                std::path::PathBuf::from(p)
            }
            None => match self.cur().file.clone() {
                Some(f) => f,
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "no file name (use :w <file>)",
                    ))
                }
            },
        };
        // format-on-save (default on): skips sources with parse errors —
        // reformatting those would lose data (comments are preserved)
        if self.config.fmt_on_save
            && dbml::fmt::format_source(&self.editor_mut().text()).is_some()
        {
            self.format_now();
        }
        let text = self.editor_mut().text();
        std::fs::write(&target, &text)?;
        self.cur_mut().saved_text = text;
        self.config.push_recent(&target.to_string_lossy());
        self.config.save();
        self.message = Some(format!(
            "\"{}\" written, {}B",
            target.display(),
            self.cur().saved_text.len()
        ));
        Ok(())
    }

    /// Export the diagram to `.svg` or `.png` (2x scale).
    pub fn export_diagram(&self, path: &str) -> anyhow::Result<String> {
        use dbml_ui::render::{raster, svg};
        if path.ends_with(".png") {
            let doc = svg::build_svg(&self.cur().diagram);
            let (sw, sh) = svg::svg_size(&self.cur().diagram);
            let scale = 2.0;
            let img =
                raster::rasterize(&doc, (sw * scale) as u32, (sh * scale) as u32, scale, scale)?;
            img.save(path)?;
        } else {
            std::fs::write(path, svg::build_svg(&self.cur().diagram))?;
        }
        Ok(format!("exported {path}"))
    }

    /// Execute an ex command (already stripped of ':').
    pub fn handle_command(&mut self, input: &str) {
        use dbml_ui::commands::ExCommand;
        match dbml_ui::commands::parse_ex(input) {
            ExCommand::Write(path) => {
                if let Err(e) = self.save_file(path.as_deref()) {
                    self.message = Some(format!(":w failed: {e}"));
                }
            }
            ExCommand::Quit { force } => {
                if self.is_modified() && !force {
                    self.message = Some("No write since last change (use :w or :q!)".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::WriteQuit => {
                if self.cur().file.is_none() {
                    self.message = Some("no file name (use :w <file>)".to_string());
                } else if let Err(e) = self.save_file(None) {
                    self.message = Some(format!(":wq failed: {e}"));
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::Open { path, force } => {
                if self.is_modified() && !force {
                    self.message =
                        Some("No write since last change (use :e! or :w first)".to_string());
                } else if let Err(e) = self.open_file(&path) {
                    self.message = Some(format!("cannot open \"{path}\": {e}"));
                }
            }
            ExCommand::Export(arg) => {
                // :export sql [dialect] writes DDL; anything else is the
                // diagram exporter (svg/png)
                let (first, rest) = match arg.split_once(' ') {
                    Some((f, r)) => (f, r.trim()),
                    None => (arg.as_str(), ""),
                };
                if first == "sql" {
                    let dialect =
                        dbml::sql::Dialect::from_name(rest).unwrap_or(dbml::sql::Dialect::Postgres);
                    match dbml::sql::export(&self.editor_mut().text(), dialect) {
                        Ok(sql) => {
                            let path = format!(
                                "{}.{}.sql",
                                self.cur().file
                                    .as_ref()
                                    .and_then(|p| p.file_stem())
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "diagram".into()),
                                match dialect {
                                    dbml::sql::Dialect::Postgres => "postgres",
                                    dbml::sql::Dialect::MySQL => "mysql",
                                    dbml::sql::Dialect::MSSQL => "mssql",
                                    dbml::sql::Dialect::Oracle => "oracle",
                                    dbml::sql::Dialect::SQLite => "sqlite",
                                }
                            );
                            match std::fs::write(&path, sql) {
                                Ok(()) => {
                                    self.message = Some(format!(
                                        "\"{path}\" written ({}B)",
                                        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                                    ))
                                }
                                Err(e) => self.message = Some(format!("export failed: {e}")),
                            }
                        }
                        Err(e) => self.message = Some(format!("export failed: {e}")),
                    }
                    return;
                }
                let path = if arg.is_empty() {
                    "diagram.svg".to_string()
                } else {
                    arg
                };
                match self.export_diagram(&path) {
                    Ok(msg) => self.message = Some(msg),
                    Err(e) => self.message = Some(format!("export failed: {e}")),
                }
            }
            ExCommand::Unknown(_) => self.message = Some(format!("not a command: {input}")),
        }
    }

    fn key_pressed(&mut self, code: KeyCode, mods: KeyModifiers) {
        let key = KeyEvent::new(code, mods);
        // the context menus are modal: any key just dismisses them
        if self.tab_menu.is_some() || self.editor_menu.is_some() {
            self.tab_menu = None;
            self.editor_menu = None;
            return;
        }
        // the record view is modal: Escape closes it, other keys are
        // swallowed so editing shortcuts cannot act underneath
        if self.record_view.is_some() {
            if key.code == KeyCode::Esc {
                self.record_view = None;
            }
            return;
        }
        // global shortcuts
        if mods.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => {
                    if let Err(e) = self.save_file(None) {
                        self.message = Some(format!("save failed: {e}"));
                    }
                    return;
                }
                KeyCode::Char('t') => {
                    self.new_tab();
                    return;
                }
                KeyCode::Char('w') => {
                    let active = self.active;
                    self.close_tab(active);
                    return;
                }
                KeyCode::Char('c') if self.config.vim => {
                    self.should_quit = true;
                    return;
                }
                _ => {}
            }
        }
        // '?' toggles the help overlay (unless typing text)
        let typing = self.focus == Focus::Editor
            && (self.editor_mut().mode == editor::Mode::Insert || self.editor_mut().cmdline.is_some());
        if key.code == KeyCode::Char('?') && !typing {
            self.show_help = !self.show_help;
            return;
        }
        if self.show_help {
            // any other key closes the overlay first
            self.show_help = false;
            return;
        }
        // Tab always toggles pane focus (and dismisses the popup)
        if key.code == KeyCode::Tab {
            self.completion = None;
            self.focus = match self.focus {
                Focus::Editor => Focus::Canvas,
                Focus::Canvas => Focus::Editor,
            };
            return;
        }
        // Space+e toggles the editor pane (leader sequence, not in insert
        // mode where Space types, nor while the cmdline is open)
        if self.space_leader {
            self.space_leader = false;
            if key.code == KeyCode::Char('e') {
                self.toggle_editor();
                return;
            }
            // any other key processes normally after the leader
        } else if key.code == KeyCode::Space
            && self.config.vim
            && self.editor_mut().mode != editor::Mode::Insert
            && self.editor_mut().cmdline.is_none()
            && !self.show_help
        {
            self.space_leader = true;
            return;
        }
        // the cmdline is reachable from either pane
        if self.editor_mut().cmdline.is_some() {
            let action = self.editor_mut().handle_key(key);
            self.after_edit(action);
            return;
        }
        if self.focus == Focus::Editor {
            if self.regular_mode() {
                self.regular_key(key);
            } else {
                self.editor_key(key);
            }
            return;
        }
        // canvas focus: hjkl/arrows pan, ':'/'/' open the cmdline
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.cur_mut().pan.x += 60.0,
            KeyCode::Char('l') | KeyCode::Right => self.cur_mut().pan.x -= 60.0,
            KeyCode::Char('k') | KeyCode::Up => self.cur_mut().pan.y += 60.0,
            KeyCode::Char('j') | KeyCode::Down => self.cur_mut().pan.y -= 60.0,
            KeyCode::Char('0') => self.cur_mut().pan = Vector::new(40.0, 40.0),
            KeyCode::Char(':') | KeyCode::Char('/') => {
                self.focus = Focus::Editor;
                self.editor_key(key);
            }
            _ => {}
        }
    }

    /// Keys routed to the editor pane, with the completion popup
    /// intercepting navigation/filter/accept keys first.
    fn editor_key(&mut self, key: KeyEvent) {
        // K: hover at the caret (vim keywordprg muscle memory) — shows the
        // error message when on a squiggle, else the definition docs for
        // the word under the cursor. Not in insert mode, where K types.
        if key.code == KeyCode::Char('K')
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && self.editor_mut().mode != editor::Mode::Insert
        {
            let caret = crate::canvas::editor::pos_to_px(
                self.editor_mut().buf.pos(),
                self.cur().editor_scroll,
                self.cur().editor_hscroll,
            );
            self.editor_hover = Some(iced::Point::new(
                caret.x + crate::canvas::editor::char_width() * 0.5,
                caret.y + crate::canvas::editor::LINE_HEIGHT * 0.5,
            ));
            return;
        }
        // any other editor key dismisses the hover tooltip
        self.editor_hover = None;
        // page scrolling works in every mode, before anything else
        match key.code {
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.last_scroll_chord = Some(std::time::Instant::now());
                self.half_page(1.0);
                return;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.last_scroll_chord = Some(std::time::Instant::now());
                self.half_page(-1.0);
                return;
            }
            KeyCode::PageDown => {
                self.page_scroll(1.0);
                return;
            }
            KeyCode::PageUp => {
                self.page_scroll(-1.0);
                return;
            }
            _ => {}
        }
        // chord roll-out: releasing ctrl a few ms before the d/u (or the
        // chord's autorepeat tail) makes the letter arrive bare, arming
        // vim's `d` operator / firing `u` — `gg` then completed `dgg` and
        // deleted to the top. The same letter right after its scroll chord
        // is residue: swallow it, and each swallow refreshes the window so
        // a held-key repeat tail is absorbed until release.
        if let Some(t) = self.last_scroll_chord {
            if t.elapsed() < SCROLL_CHORD_ROLLOUT {
                if matches!(key.code, KeyCode::Char('d') | KeyCode::Char('u'))
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.last_scroll_chord = Some(std::time::Instant::now());
                    return;
                }
            } else {
                self.last_scroll_chord = None;
            }
        }
        // Ctrl+V pastes the system clipboard in insert mode (regular mode
        // handles it in regular_key; vim's p/P stay register-based, and
        // there is no block-visual for vim's Ctrl+V binding to serve)
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('v')
            && self.editor_mut().mode == editor::Mode::Insert
        {
            self.paste_from_system();
            return;
        }
        // Ctrl+Z / Ctrl+Y (and Ctrl+Shift+Z) undo/redo in insert mode, on
        // the same history stack vim's u / Ctrl+r walk (regular mode
        // handles the chords in regular_key; vim normal mode keeps u /
        // Ctrl+r, so Ctrl+Z is deliberately not mapped there). Arms sit
        // before the completion popup's typing interception so an undo
        // lands even with the popup open (it is dismissed then).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('y'))
            && self.editor_mut().mode == editor::Mode::Insert
        {
            if key.code == KeyCode::Char('y') || key.modifiers.contains(KeyModifiers::SHIFT) {
                self.redo_step();
            } else {
                self.undo_step();
            }
            return;
        }
        // Ctrl+Space toggles the popup (VSCode muscle memory)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Space {
            if self.editor_mut().mode == editor::Mode::Insert {
                self.toggle_completion();
            }
            return;
        }
        if self.completion.is_some() {
            match key.code {
                KeyCode::Up => {
                    let c = self.completion.as_mut().unwrap();
                    c.selected = c.selected.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    let c = self.completion.as_mut().unwrap();
                    c.selected = (c.selected + 1).min(c.items.len().saturating_sub(1));
                    return;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    let idx = self.completion.as_ref().unwrap().selected;
                    self.completion_accept(idx);
                    return;
                }
                KeyCode::Esc => {
                    self.completion = None;
                    return;
                }
                KeyCode::Char(c) if c.is_alphanumeric() || c == '_' => {
                    let action = self.editor_mut().handle_key(key);
                    self.after_edit(action);
                    self.refilter_completion();
                    self.follow_cursor_horizontal();
                    return;
                }
                KeyCode::Backspace => {
                    let action = self.editor_mut().handle_key(key);
                    self.after_edit(action);
                    self.refilter_completion();
                    self.follow_cursor_horizontal();
                    return;
                }
                _ => self.completion = None, // any other key dismisses, then proceeds
            }
        }
        let was_insert = self.editor_mut().mode == editor::Mode::Insert;
        let action = self.editor_mut().handle_key(key);
        // auto-triggers fire only for keys typed while ALREADY in insert:
        // the i/a/o/s keys that enter insert mode must not pop completions
        // (the popup then swallowed the first Esc, stranding the session in
        // insert mode after a single Esc)
        if self.editor_mut().mode == editor::Mode::Insert && was_insert {
            match key.code {
                KeyCode::Char(c) if c.is_alphanumeric() || c == '_' => {
                    self.maybe_autocomplete(false);
                }
                KeyCode::Char('.') => self.maybe_autocomplete(true),
                _ => {}
            }
        }
        self.after_edit(action);
        // keep the cursor on screen after every key, motions included
        // (Action::None never reached the follow inside after_edit)
        self.follow_cursor_horizontal();
        self.keep_cursor_visible();
    }

    /// The word being typed before the cursor: its start position and text.
    fn word_before_cursor(&self) -> Option<(crate::editor::buffer::Pos, String)> {
        let (row, col) = self.editor().buf.cursor();
        let line = self.editor().buf.line(row);
        let start = line[..line.chars().count().min(col)]
            .char_indices()
            .collect::<Vec<_>>();
        let mut s = col;
        while s > 0 {
            let ch = line.chars().nth(s - 1)?;
            if ch.is_alphanumeric() || ch == '_' {
                s -= 1;
            } else {
                break;
            }
        }
        let prefix: String = line.chars().skip(s).take(col - s).collect();
        let _ = start;
        (s < col).then_some((crate::editor::buffer::Pos { row, col: s }, prefix))
    }

    /// The byte offset of the cursor in `source` (for highlight lookups).
    fn cursor_byte(&self) -> usize {
        let (row, col) = self.editor().buf.cursor();
        let mut b = 0usize;
        for l in self.editor().buf.lines().iter().take(row) {
            b += l.len() + 1;
        }
        b += self
            .editor()
            .buf
            .line(row)
            .chars()
            .take(col)
            .map(char::len_utf8)
            .sum::<usize>();
        b
    }

    /// Inside a string or comment: completions would be noise there.
    fn in_prose(&self) -> bool {
        let b = self.cursor_byte();
        if b == 0 {
            return false;
        }
        let idx = self.cur().highlights.partition_point(|t| t.end < b);
        self.cur().highlights
            .get(idx)
            .map(|t| {
                t.start < b
                    && matches!(
                        t.kind,
                        dbml_ui::dbml::highlight::Kind::String
                            | dbml_ui::dbml::highlight::Kind::Comment
                    )
            })
            .unwrap_or(false)
    }

    fn items_for(
        &self,
        word: crate::editor::buffer::Pos,
        prefix: &str,
    ) -> Vec<crate::editor::completion::Item> {
        let line = self.editor().buf.line(word.row);
        let before: String = line.chars().take(word.col).collect();
        crate::editor::completion::complete(prefix, &before, &self.cur().diagram)
    }

    /// Open the popup if the context warrants it. `after_dot` allows an
    /// empty prefix (member completion right after `table.`).
    fn maybe_autocomplete(&mut self, after_dot: bool) {
        if self.in_prose() {
            self.completion = None;
            return;
        }
        let word = match self.word_before_cursor() {
            Some(w) => w,
            None => {
                if after_dot {
                    // empty prefix right after the dot
                    let (row, col) = self.editor_mut().buf.cursor();
                    (crate::editor::buffer::Pos { row, col }, String::new())
                } else {
                    return;
                }
            }
        };
        // non-dot auto-open needs a real prefix; `.` needs the char before
        // the cursor to actually be the dot
        let after_dot_ctx = after_dot
            && word.1.is_empty()
            && word
                .0
                .col
                .checked_sub(1)
                .and_then(|c| self.editor_mut().buf.line(word.0.row).chars().nth(c))
                == Some('.');
        if !after_dot && word.1.chars().count() < 2 {
            return;
        }
        if after_dot && !after_dot_ctx && word.1.chars().count() < 2 {
            return;
        }
        let items = self.items_for(word.0, &word.1);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(Completion {
            items,
            selected: 0,
            word: word.0,
        });
    }

    fn toggle_completion(&mut self) {
        if self.completion.is_some() {
            self.completion = None;
        } else {
            let word = self.word_before_cursor().unwrap_or_else(|| {
                let (row, col) = self.editor_mut().buf.cursor();
                (crate::editor::buffer::Pos { row, col }, String::new())
            });
            let items = self.items_for(word.0, &word.1);
            if !items.is_empty() {
                self.completion = Some(Completion {
                    items,
                    selected: 0,
                    word: word.0,
                });
            }
        }
    }

    /// Recompute the popup for the extended/shrunk prefix; close when the
    /// prefix is gone or nothing matches.
    fn refilter_completion(&mut self) {
        let Some(c) = &self.completion else { return };
        let (row, col) = self.editor().buf.cursor();
        let line = self.editor().buf.line(row);
        if row != c.word.row || col < c.word.col {
            self.completion = None;
            return;
        }
        let prefix: String = line
            .chars()
            .skip(c.word.col)
            .take(col - c.word.col)
            .collect();
        if prefix.is_empty() {
            self.completion = None;
            return;
        }
        let items = self.items_for(c.word, &prefix);
        self.completion = if items.is_empty() {
            None
        } else {
            Some(Completion {
                selected: c.selected.min(items.len() - 1),
                items,
                word: c.word,
            })
        };
    }

    /// Replace the typed prefix with the chosen item and keep typing.
    fn completion_accept(&mut self, idx: usize) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(item) = c.items.get(idx) else { return };
        let (row, col) = self.editor_mut().buf.cursor();
        if row != c.word.row || col < c.word.col {
            return;
        }
        self.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(c.word.row, c.word.col));
        self.editor_mut().buf.delete_str(col - c.word.col);
        self.editor_mut().buf.insert_str(&item.label);
        if item.kind == crate::editor::completion::ItemKind::Keyword {
            self.editor_mut().buf.insert_str(" ");
        }
        self.after_edit(Action::Changed);
        self.follow_cursor_horizontal();
    }

    fn after_edit(&mut self, action: Action) {
        // a vim yank (y/yy/yiw/…) rides along to the system clipboard:
        // the Editor queues the text, the app pushes it (failures become
        // a toast, never a panic)
        if let Some(text) = self.editor_mut().take_system_yank() {
            if let Err(e) = self.copy_to_system(&text) {
                self.message = Some(format!("clipboard: {e}"));
            }
        }
        match action {
            Action::None => {}
            Action::Changed => {
                let text = self.editor_mut().text();
                self.cur_mut().set_source(&text);
                self.keep_cursor_visible();
            }
            Action::Command(cmd) => self.handle_command(&cmd),
        }
    }

    /// Visible editor lines from the window size: the editor canvas gets
    /// the window minus the root layout's chrome (toolbars, status bar,
    /// paddings — see `view::EDITOR_CHROME` for the term-by-term total).
    fn page_lines(&self) -> f32 {
        ((self.window_size.1 - crate::view::EDITOR_CHROME)
            / crate::canvas::editor::LINE_HEIGHT)
            .max(5.0)
    }

    fn max_scroll(&self) -> f32 {
        (self.editor().buf.line_count() as f32 - self.page_lines()).max(0.0)
    }

    /// Widest the editor view can scroll right, in px: the longest line's
    /// width past the visible text area (pane width minus the gutter and a
    /// small right margin), never negative. The wheel's horizontal axis and
    /// `follow_cursor_horizontal` share this clamp.
    fn max_hscroll(&self) -> f32 {
        let max_cols = self
            .editor()
            .buf
            .lines()
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0) as f32;
        let text_w = self.editor_pane_width()
            - crate::canvas::editor::GUTTER
            - crate::canvas::editor::H_RIGHT_MARGIN;
        (max_cols * crate::canvas::editor::char_width() - text_w).max(0.0)
    }

    /// Start/re-target a line-stepped animated scroll. `duration_ms` is the
    /// neoscroll duration for the WHOLE remaining distance; repeated
    /// requests extend the accumulated target (neoscroll behavior) and the
    /// animation restarts from the current stepped position.
    fn animate_scroll(&mut self, to: f32, duration_ms: f32, cursor: Option<(usize, usize)>) {
        // round to the line step INSIDE the clamp: clamping a fractional
        // max first and rounding after would land a line past the buffer
        let to = to.round().clamp(0.0, self.max_scroll());
        let from = self.cur().editor_scroll.round();
        let cursor_moves = cursor.is_some_and(|(a, b)| a != b);
        if (to - from).abs() < 0.5 && !cursor_moves {
            // neither the window nor the cursor can move
            self.cur_mut().editor_scroll = to;
            self.cur_mut().scroll_anim = None;
            self.cur_mut().scroll_target = None;
            return;
        }
        if (to - from).abs() >= 0.5 {
            self.cur_mut().scroll_target = Some(to);
        }
        self.cur_mut().scroll_anim = Some(ScrollAnim {
            scroll_from: from,
            scroll_to: to,
            started: std::time::Instant::now(),
            duration: scroll_duration_override().unwrap_or(duration_ms / 1000.0),
            cursor,
        });
    }

    /// neoscroll's distance-proportional duration (z-command formula):
    /// a half page takes HALF_PAGE_MS, scaling linearly with distance.
    /// neoscroll semantics: the cursor always animates (line by line) even
    /// when the window cannot scroll (fitting document, or at an edge);
    /// the window follows in lockstep until ITS clamp — stop_eof — freezes
    /// it, after which the cursor continues alone.
    ///
    /// 'scrolloff' couples the two: scrolling down from within SCROLLOFF
    /// of the window top walks the cursor out to the margin while the
    /// content scrolls (and symmetrically for up from the bottom), so the
    /// cursor never gets scrolled off its edge of the window.
    fn scroll_by_lines(&mut self, lines: f32, duration_ms: f32) {
        self.completion = None;
        let pos = self.editor_mut().buf.pos();
        let rows = self.editor_mut().buf.line_count() as i32;
        let base = self.cur().scroll_target.unwrap_or(self.cur().editor_scroll.round());
        let to = (base + lines).round().clamp(0.0, self.max_scroll());
        // static scroll: the cursor keeps its screen line
        let mut cursor_to = pos.row as f32 + lines;
        // margin correction: from inside 'scrolloff', the cursor first
        // travels to the far margin, then rides the view from there
        let scroll = self.cur().editor_scroll;
        let screen = pos.row as f32 - scroll;
        if lines > 0.0 && screen < SCROLLOFF {
            cursor_to = cursor_to.max(to + SCROLLOFF);
        } else if lines < 0.0 && screen > self.page_lines() - 1.0 - SCROLLOFF {
            cursor_to = cursor_to.min(to + self.page_lines() - 1.0 - SCROLLOFF);
        }
        let cursor_to = (cursor_to.round() as i32).clamp(0, rows - 1) as usize;
        self.focus = Focus::Editor;
        self.animate_scroll(to, duration_ms, Some((pos.row, cursor_to)));
    }

    /// Half-page scroll with the cursor (vim Ctrl-d / Ctrl-u).
    fn half_page(&mut self, dir: f32) {
        self.scroll_by_lines((self.page_lines() / 2.0) * dir, HALF_PAGE_MS);
    }

    /// Full-page scroll with the cursor (PageDown / PageUp).
    fn page_scroll(&mut self, dir: f32) {
        self.scroll_by_lines(self.page_lines() * dir, FULL_PAGE_MS);
    }

    /// Editor pane width in px from the window size and split share: the
    /// pane row is `window - 16 column padding` and FillPortion counts
    /// (split * 1000, truncated) share it — mirror that integer math so
    /// the hover/cursor estimates land on the real layout.
    fn editor_pane_width(&self) -> f32 {
        let e = (self.split * 1000.0) as u16;
        let d = ((1.0 - self.split) * 1000.0) as u16;
        // the window content is flush (no outer horizontal padding)
        (self.window_size.0.max(60.0) * e as f32 / (e + d).max(1) as f32).max(60.0)
    }

    /// Window-x of the pane seam: the editor pane's width — the flush
    /// editor/diagram borders the edge chevron and resize strip straddle
    /// (the window content is flush, no outer left padding). At the
    /// collapsed edge it is 0: the diagram's left border is the window's
    /// left edge.
    pub fn seam_x(&self) -> f32 {
        if self.editor_collapsed {
            0.0
        } else {
            self.editor_pane_width()
        }
    }

    /// Keep the cursor inside the pane horizontally when the line is
    /// wider than the text area: scroll right just far enough to bring a
    /// cursor that passed the right edge back inside (with a few chars of
    /// lookahead, never past the line's end), scroll back left when the
    /// cursor moves before the view, and relax to 0 once the line fits.
    fn follow_cursor_horizontal(&mut self) {
        let char_w = crate::canvas::editor::char_width();
        let text_w = self.editor_pane_width() - crate::canvas::editor::GUTTER;
        if text_w <= 4.0 * char_w {
            // degenerate pane: the lookahead math would misbehave, but the
            // buffer-wide clamp still applies (wheel scrolling shares this
            // offset)
            let hs = self.cur().editor_hscroll;
            self.cur_mut().editor_hscroll = hs.clamp(0.0, self.max_hscroll());
            return;
        }
        let pos = self.editor_mut().buf.pos();
        let line_len = self.editor_mut().buf.line(pos.row).chars().count();
        let col = pos.col;
        let cx = col as f32 * char_w;
        let line_end = line_len as f32 * char_w;
        let mut hs = self.cur().editor_hscroll;
        if cx + char_w > hs + text_w {
            // cursor's cell would cross the right edge
            hs = cx + char_w - text_w + 4.0 * char_w;
        } else if cx < hs {
            hs = (cx - 4.0 * char_w).max(0.0);
        }
        // never scroll past what the current line needs (plus one char),
        // nor past the buffer-wide limit shared with wheel scrolling
        let max = (line_end + char_w - text_w).max(0.0).min(self.max_hscroll());
        self.cur_mut().editor_hscroll = hs.clamp(0.0, max).round();
    }

    /// Scroll the editor viewport so the cursor obeys 'scrolloff': at
    /// least SCROLLOFF screen lines from the top and bottom of the window
    /// whenever the buffer has room (vim j/k behavior — the view scrolls
    /// under a pinned cursor once it reaches the margin). Vim motions
    /// jump instantly — gg, G, search hits and click-to-jump land in
    /// place; the only animated scrolls are the explicit neoscroll ones
    /// (Ctrl-d / Ctrl-u / Ctrl-f / Ctrl-b and the page keys).
    fn keep_cursor_visible(&mut self) {
        let row = self.editor_mut().buf.pos().row as f32;
        let page = self.page_lines();
        let scroll = self.cur().editor_scroll;
        let target = if row < scroll + SCROLLOFF {
            row - SCROLLOFF
        } else if row > scroll + page - 1.0 - SCROLLOFF {
            row - (page - 1.0 - SCROLLOFF)
        } else {
            return;
        };
        // an in-flight neoscroll animation would fight the jump — drop it
        self.cur_mut().scroll_anim = None;
        self.cur_mut().scroll_target = None;
        self.cur_mut().editor_scroll = target.clamp(0.0, self.max_scroll());
    }

    /// A click on a diagram table — or one of its column rows — jumps the
    /// editor to the definition: the clicked column's own name token, or
    /// the table's name line for a header press.
    fn consume_pending_jump(&mut self) {
        if let Some(jump) = self.pending_jump.take() {
            let (row, col) = dbml::parser::line_col_at(self.source(), jump.span.start);
            self.editor_mut().jump(row, col);
            self.focus = Focus::Editor;
            self.keep_cursor_visible();
            self.follow_cursor_horizontal();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::with_source(dbml_ui::SAMPLE_DBML.to_string())
    }

    /// Screen point of a table box corner plus an offset.
    fn over_box(app: &App, title: &str, dx: f32, dy: f32) -> Point {
        let b = app.diagram().boxes.iter().find(|b| b.title == title).unwrap();
        crate::canvas::diagram::to_px(b.x, b.y, app.cur().zoom, app.cur().pan) + Vector::new(dx, dy)
    }

    #[test]
    fn drag_pan_moves_viewport() {
        let mut app = app();
        // empty space, far from any box
        let p = Point::new(2000.0, 2000.0);
        let _ = update(&mut app, Message::CanvasPressed(p));
        let _ = update(&mut app, Message::CanvasMoved(Point::new(1990.0, 2004.0)));
        assert_eq!(app.cur().pan, Vector::new(-10.0, 4.0));
        let _ = update(
            &mut app,
            Message::CanvasReleased(Point::new(1990.0, 2004.0)),
        );
        assert_eq!(app.drag, Drag::None);
    }

    #[test]
    fn wheel_zoom_is_cursor_anchored() {
        let mut app = app();
        let anchor = over_box(&app, "users", 7.0, 9.0);
        let _ = update(&mut app, Message::CanvasMoved(anchor));
        let before = crate::canvas::diagram::to_cell_f(anchor, app.cur().zoom, app.cur().pan);
        let _ = update(&mut app, Message::Wheel(1.0));
        let after = crate::canvas::diagram::to_cell_f(anchor, app.cur().zoom, app.cur().pan);
        assert!(app.cur().zoom > 20.0, "zoomed in: {}", app.cur().zoom);
        assert!(
            (before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01,
            "anchor drifted: {before:?} -> {after:?}"
        );

        let _ = update(&mut app, Message::Wheel(-1.0));
        assert!(
            (app.cur().zoom - 20.0).abs() < 1e-3,
            "round trip zoom, got {}",
            app.cur().zoom
        );
    }

    #[test]
    fn auto_arrange_fills_table_pos_relayouts_and_toasts() {
        let mut app = app();
        // a stale dragged position and stale highlight state get replaced
        app.cur_mut().table_pos.insert("users".to_string(), (500, 500));
        app.cur_mut().edge_anim = vec![0.5; app.diagram().edges.len()];
        app.cur_mut().hover_edges.push(0);
        let _ = update(&mut app, Message::AutoArrange);
        assert_eq!(app.message.as_deref(), Some("auto-arranged"));
        // every table gets an override, and the re-layout applied it
        assert_eq!(app.cur().table_pos.len(), app.diagram().boxes.len());
        for b in &app.diagram().boxes {
            assert_eq!(
                app.cur().table_pos.get(&b.title),
                Some(&(b.x, b.y)),
                "{}",
                b.title
            );
        }
        assert_ne!(
            app.cur().table_pos.get("users"),
            Some(&(500, 500)),
            "the stale drag is replaced"
        );
        // left-to-right: users (referenced) one 12-cell gap left of posts
        let users = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        let posts = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "posts")
            .unwrap();
        assert_eq!(posts.x, users.right() + layout::ARRANGE_LAYER_GAP_X);
        // highlight state reset
        assert!(app.cur().hover_edges.is_empty());
        assert!(app.cur().edge_anim.iter().all(|&t| t == 0.0));
    }

    #[test]
    fn zoom_to_fit_scales_and_pans_content_into_view() {
        let mut app = app();
        app.window_size = (1400.0, 900.0);
        // a table dragged out past the origin: the fit must cover it
        let src = app.source().to_string();
        app.cur_mut()
            .table_pos
            .insert("users".to_string(), (-50, -30));
        app.cur_mut().set_source(&src);
        let (mut min_x, mut min_y, mut max_r, mut max_b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for b in &app.diagram().boxes {
            min_x = min_x.min(b.x);
            min_y = min_y.min(b.y);
            max_r = max_r.max(b.right());
            max_b = max_b.max(b.bottom());
        }
        let _ = update(&mut app, Message::ZoomToFit);
        let (zoom, pan) = (app.cur().zoom, app.cur().pan);
        let (cw, ch) = app.diagram_canvas_size();
        // expected scale: the content into (canvas - 2 * 40px margin),
        // clamped to the wheel's range
        let exp = (((cw - 2.0 * FIT_MARGIN_PX) / ((max_r - min_x) as f32 * CELL_ASPECT))
            .min((ch - 2.0 * FIT_MARGIN_PX) / (max_b - min_y) as f32))
        .clamp(ZOOM_MIN, ZOOM_MAX);
        assert!((zoom - exp).abs() < 1e-4, "zoom {zoom} vs {exp}");
        // the content's top-left corner sits at the margin
        let tl = crate::canvas::diagram::to_px(min_x, min_y, zoom, pan);
        assert!(
            (tl.x - FIT_MARGIN_PX).abs() < 1e-3 && (tl.y - FIT_MARGIN_PX).abs() < 1e-3,
            "top-left {tl:?}"
        );
        // and the whole content is inside the canvas
        let s = crate::canvas::diagram::cell_size(zoom);
        let right = tl.x + (max_r - min_x) as f32 * s.width;
        let bottom = tl.y + (max_b - min_y) as f32 * s.height;
        assert!(
            right <= cw + 0.01 && bottom <= ch + 0.01,
            "content {right}x{bottom} overflows canvas {cw}x{ch}"
        );
    }

    #[test]
    fn zoom_to_fit_clamps_to_the_wheel_range() {
        let mut app = App::with_source("Table tiny { id integer [pk] }\n".into());
        // the diagram pane takes the whole (huge) window: the fit scale
        // would far exceed what the wheel allows
        app.editor_collapsed = true;
        app.window_size = (4000.0, 3000.0);
        let _ = update(&mut app, Message::ZoomToFit);
        assert_eq!(app.cur().zoom, ZOOM_MAX);
    }

    #[test]
    fn zoom_to_fit_and_auto_arrange_tolerate_an_empty_diagram() {
        let mut app = App::blank_start();
        let before = (app.cur().zoom, app.cur().pan);
        let _ = update(&mut app, Message::ZoomToFit);
        assert_eq!((app.cur().zoom, app.cur().pan), before, "nothing to fit");
        let _ = update(&mut app, Message::AutoArrange);
        assert!(app.cur().table_pos.is_empty());
        assert_eq!(app.message.as_deref(), Some("auto-arranged"));
    }

    #[test]
    fn table_drag_relayouts_live() {
        let mut app = app();
        let press = over_box(&app, "posts", 3.0, 3.0);
        let before = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "posts")
            .unwrap()
            .clone();
        let _ = update(&mut app, Message::CanvasPressed(press));
        assert!(matches!(app.drag, Drag::Table { .. }));
        // move 8 cells right, 2 down
        let s = crate::canvas::diagram::cell_size(app.cur().zoom);
        let _ = update(
            &mut app,
            Message::CanvasMoved(Point::new(
                press.x + 8.0 * s.width,
                press.y + 2.0 * s.height,
            )),
        );
        let after = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "posts")
            .unwrap();
        assert_eq!((after.x, after.y), (before.x + 8, before.y + 2));
        assert_eq!(
            app.cur().table_pos.get("posts"),
            Some(&(before.x + 8, before.y + 2))
        );

        let _ = update(
            &mut app,
            Message::CanvasReleased(Point::new(press.x + 80.0, press.y + 20.0)),
        );
        assert_eq!(app.pending_jump, None, "a real drag is not a click");
    }

    #[test]
    fn table_drag_moves_above_and_left_of_origin() {
        let mut app = app();
        // users sits at (0,0): press it and drag up-left past the origin
        let press = over_box(&app, "users", 3.0, 3.0);
        let _ = update(&mut app, Message::CanvasPressed(press));
        let s = crate::canvas::diagram::cell_size(app.cur().zoom);
        let _ = update(
            &mut app,
            Message::CanvasMoved(Point::new(
                press.x - 6.0 * s.width,
                press.y - 4.0 * s.height,
            )),
        );
        let b = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        assert!(b.x < 0 && b.y < 0, "moved past origin: {:?}", (b.x, b.y));
        assert_eq!(app.cur().table_pos.get("users"), Some(&(b.x, b.y)));
        // and it survives a re-parse (edit)
        let src = app.source().to_string();
        app.cur_mut().set_source(&src);
        let b = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        assert!(
            b.x < 0 && b.y < 0,
            "negative override survives: {:?}",
            (b.x, b.y)
        );
    }

    #[test]
    fn click_without_drag_jumps_editor_to_definition() {
        let mut app = app();
        let before = app.editor_mut().buf.cursor();
        // the header band is not a column row: the jump goes to the table
        let p = over_box(&app, "posts", 4.0, 4.0);
        let _ = update(&mut app, Message::CanvasPressed(p));
        let _ = update(&mut app, Message::CanvasReleased(p));
        assert_eq!(app.focus, Focus::Editor);
        let after = app.editor_mut().buf.cursor();
        assert_ne!(before, after, "cursor moved to the definition");
        let line = app.editor_mut().buf.line(after.0);
        assert!(line.contains("posts"), "on the posts line: {line:?}");
        // the caret lands on the name token, not just the line
        assert_eq!(after.1, line.find("posts").unwrap());
    }

    #[test]
    fn click_on_a_column_row_jumps_to_that_column() {
        let mut app = app();
        // the "username" row of the users box: the header band is one cell
        // tall, so row i's band spans (i + 1)..(i + 2) cells from the top
        let row = app
            .diagram()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap()
            .rows
            .iter()
            .position(|r| r.name == "username")
            .unwrap();
        let s = crate::canvas::diagram::cell_size(app.cur().zoom);
        let p = over_box(&app, "users", 4.0, (row as f32 + 1.5) * s.height);
        let _ = update(&mut app, Message::CanvasPressed(p));
        let _ = update(&mut app, Message::CanvasReleased(p));
        assert_eq!(app.focus, Focus::Editor);
        let after = app.editor_mut().buf.cursor();
        let line = app.editor_mut().buf.line(after.0);
        assert!(
            line.contains("username"),
            "on the username line, not the table's: {line:?}"
        );
        // the caret sits at the column's name token on that line
        assert_eq!(after.1, line.find("username").unwrap());
    }

}

#[cfg(test)]
mod record_view_tests {
    use super::*;

    fn records_app() -> App {
        App::with_source(
            r#"Table users {
  id integer [pk]
  name varchar
  records (id, name) {
    1, 'Alice'
    2, 'Bob'
  }
}

Table posts {
  id integer [pk]
  user_id integer
  records (id, user_id) {
    10, 1
    11, 9
  }
}

Ref: posts.user_id > users.id
"#
            .to_string(),
        )
    }

    fn node_of(app: &App, title: &str) -> usize {
        app.diagram()
            .boxes
            .iter()
            .find(|b| b.title == title)
            .expect("box")
            .node
    }

    /// The records affordance point: the rows glyph at the right end of a
    /// table's header band.
    fn records_icon_point(app: &App, title: &str) -> Point {
        let b = app.diagram().boxes.iter().find(|b| b.title == title).unwrap();
        let s = crate::canvas::diagram::cell_size(app.cur().zoom);
        let tl = crate::canvas::diagram::to_px(b.x, b.y, app.cur().zoom, app.cur().pan);
        Point::new(
            tl.x + b.w as f32 * s.width - 1.0 * s.width,
            tl.y + s.height * 0.5,
        )
    }

    #[test]
    fn records_icon_press_opens_the_view_not_a_drag() {
        let mut app = records_app();
        let p = records_icon_point(&app, "users");
        let _ = update(&mut app, Message::CanvasPressed(p));
        let users = node_of(&app, "users");
        assert_eq!(
            app.record_view.as_ref().map(|rv| rv.node),
            Some(users),
            "the record view opened for users"
        );
        assert_eq!(app.drag, Drag::None, "no table drag from the affordance");
        // releasing must not land the jump-to-editor click either
        let _ = update(&mut app, Message::CanvasReleased(p));
        assert_eq!(app.pending_jump, None);
        // and the generic table press still drags as before
        let b = app.diagram().boxes.iter().find(|b| b.title == "users").unwrap();
        let s = crate::canvas::diagram::cell_size(app.cur().zoom);
        let tl = crate::canvas::diagram::to_px(b.x, b.y, app.cur().zoom, app.cur().pan);
        let q = Point::new(tl.x + 3.0 * s.width, tl.y + 3.0 * s.height);
        let _ = update(&mut app, Message::CanvasPressed(q));
        assert!(matches!(app.drag, Drag::Table { .. }));
    }

    #[test]
    fn record_view_messages_open_close_follow() {
        let mut app = records_app();
        let users = node_of(&app, "users");
        let posts = node_of(&app, "posts");
        let _ = update(&mut app, Message::OpenRecords(users));
        assert_eq!(app.record_view.as_ref().unwrap().node, users);
        assert_eq!(app.record_view.as_ref().unwrap().follow, None);
        let _ = update(&mut app, Message::RecordsFollow(posts, 0));
        assert_eq!(app.record_view.as_ref().unwrap().node, posts);
        assert_eq!(app.record_view.as_ref().unwrap().follow, Some(0));
        let _ = update(&mut app, Message::CloseRecords);
        assert!(app.record_view.is_none());
        // a stale index (after an edit re-parses) opens nothing
        let _ = update(&mut app, Message::OpenRecords(999));
        assert!(app.record_view.is_none());
        // a node without records opens nothing
        let _ = update(&mut app, Message::OpenRecords(usize::MAX));
        assert!(app.record_view.is_none());
    }

    #[test]
    fn record_view_is_modal_and_escape_closes_it() {
        let mut app = records_app();
        let users = node_of(&app, "users");
        let _ = update(&mut app, Message::OpenRecords(users));
        // Escape closes
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.record_view.is_none());
        // while open, other keys are swallowed ('?' must not open help)
        let _ = update(&mut app, Message::OpenRecords(users));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert!(app.record_view.is_some(), "still open");
        assert!(!app.show_help, "help did not open underneath");
        // switching tabs drops the overlay (it indexes the old schema)
        app.new_tab();
        assert!(app.record_view.is_none());
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn file_round_trip_is_byte_identical() {
        let dir = std::env::temp_dir().join(format!("dbml-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.dbml");
        std::fs::write(&path, "Table t {\n  id integer [pk]\n}\n").unwrap();

        let mut app = App::default();
        app.open_file(path.to_str().unwrap()).unwrap();
        assert!(!app.is_modified());

        // edit: dd the second line
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(1, 0));
        let action = app
            .editor_mut()
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.editor_mut()
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(action, Action::None);
        assert!(app.is_modified());

        app.save_file(None).unwrap();
        assert!(!app.is_modified());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), app.editor_mut().text());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ex_commands_save_and_export() {
        let dir = std::env::temp_dir().join(format!("dbml-ui-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("e.svg");
        let mut app = App::default();
        app.handle_command(&format!("export {}", out.display()));
        assert!(out.exists(), "svg exported");
        let png = dir.join("e.png");
        app.handle_command(&format!("export {}", png.display()));
        assert!(png.exists(), "png exported");

        let dbml = dir.join("x.dbml");
        app.handle_command(&format!("w {}", dbml.display()));
        assert!(dbml.exists(), "written via :w");
        assert!(!app.is_modified());

        // :q refuses when modified, :q! works
        app.editor_mut()
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(app.is_modified());
        app.handle_command("q");
        assert!(!app.should_quit, "refuses with unsaved changes");
        app.handle_command("q!");
        assert!(app.should_quit);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use crate::editor::completion::ItemKind;

    fn insert_app() -> App {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        app
    }

    fn ctrl_space() -> Message {
        Message::KeyPressed(KeyCode::Space, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_space_opens_and_toggles() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        let c = app.completion.as_ref().expect("popup open");
        assert!(c.items.iter().any(|i| i.label == "Table"));
        assert!(c.items.iter().any(|i| i.label == "users"));
        let _ = update(&mut app, ctrl_space());
        assert!(app.completion.is_none(), "second Ctrl-Space closes");
    }

    #[test]
    fn typing_two_chars_autocompletes_and_filters() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        let c = app.completion.as_ref().expect("auto-opened");
        assert!(c.items.iter().any(|i| i.label == "Table"), "{c:?}");
        assert!(!c.items.iter().any(|i| i.label == "users"), "{c:?}");
    }

    #[test]
    fn enter_accepts_keyword_with_trailing_space() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none());
        assert!(
            app.source().starts_with("Table "),
            "got: {:?}",
            &app.source()[..20]
        );
        let (_, col) = app.editor_mut().buf.cursor();
        assert_eq!(col, "Table ".len());
    }

    #[test]
    fn dot_completes_columns_and_accept() {
        let mut app = App::with_source("Table users { id integer [pk] }\nRef: users\n".into());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(1, 0));
        app.editor_mut().buf.move_cursor(crate::editor::buffer::Move::End);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('.'), KeyModifiers::NONE),
        );
        let c = app.completion.as_ref().expect("dot opens columns");
        assert!(
            c.items
                .iter()
                .any(|i| i.label == "id" && i.kind == ItemKind::Column),
            "{c:?}"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.source().ends_with("Ref: users.id\n"), "{:?}", app.source());
    }

    #[test]
    fn esc_dismisses_but_keeps_insert_mode() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        assert!(app.completion.is_some());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none());
        assert_eq!(app.editor_mut().mode, crate::editor::Mode::Insert);
    }

    #[test]
    fn regular_mode_types_and_selects() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // keep test writes out of the real config
        std::env::set_var(
            "DBML_UI_CONFIG",
            std::env::temp_dir().join("dbml-ui-cfg-regular-test"),
        );
        let mut app = App {
            config: crate::config::Config {
                vim: false,
                ..Default::default()
            },
            ..App::default()
        };
        app.editor_mut().set_text("");
        app.cur_mut().set_source("");
        let k = |c: char| Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE);
        for ch in "hello".chars() {
            let _ = update(&mut app, k(ch));
        }
        assert!(app.source().contains("hello"), "{}", app.source());
        // vim keys are inert: x types instead of deleting
        let _ = update(&mut app, k('x'));
        assert!(app.source().contains("x"));
        // backspace deletes the typed x
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(!app.source().contains('x') || app.source().contains("xx"));
        assert!(app.source().ends_with("hello"));
        // Shift+Left selects, then typing replaces the selection
        for _ in 0..2 {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Left, KeyModifiers::SHIFT),
            );
        }
        assert!(app.editor_mut().buf.selection_anchor().is_some(), "shift selects");
        let _ = update(&mut app, k('!'));
        assert!(app.source().ends_with("hel!"), "{}", app.source());
        // Esc clears the selection; Ctrl+C copies without quitting
        for _ in 0..2 {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Left, KeyModifiers::SHIFT),
            );
        }
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(!app.should_quit, "regular Ctrl+C copies, not quit");
        assert!(!app.editor_mut().register_lines().is_empty());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(app.editor_mut().buf.selection_anchor().is_none());
        std::env::remove_var("DBML_UI_CONFIG");
    }

    #[test]
    fn toggle_vim_switches_and_persists() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-tgl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("DBML_UI_CONFIG", dir.join("config"));
        let mut app = App::default();
        assert!(!app.regular_mode(), "vim by default");
        let _ = update(&mut app, Message::ToggleVim);
        assert!(app.regular_mode());
        assert_eq!(app.editor_mut().mode, editor::Mode::Normal, "vim state reset");
        // persisted
        assert!(!crate::config::Config::load().vim);
        // vim motions work again after toggling back
        let _ = update(&mut app, Message::ToggleVim);
        assert!(!app.regular_mode());
        let before = app.editor_mut().buf.pos();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert_ne!(app.editor_mut().buf.pos(), before, "vim l moves");
        // vim Ctrl+C still quits
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.should_quit);
        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_file_records_recent() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DBML_UI_CONFIG", dir.join("config"));
        let path = dir.join("x.dbml");
        std::fs::write(&path, "Table t { id integer }").unwrap();
        let mut app = App::blank_start();
        let _ = update(
            &mut app,
            Message::OpenRecent(path.to_string_lossy().into_owned()),
        );
        assert!(app.doc_open(), "opened from the recent list");
        assert_eq!(
            app.config.recent.first().map(String::as_str),
            Some(path.to_string_lossy().as_ref())
        );
        let _ = update(
            &mut app,
            Message::OpenRecent(path.to_string_lossy().into_owned()),
        );
        assert_eq!(app.config.recent.len(), 1, "no duplicate entries");
        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_now_cleans_the_buffer() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("DBML_UI_CONFIG", dir.join("config"));
        let mut app = App::default();
        let messy = "Table   users {\nid integer [pk,increment]\n}\n";
        app.editor_mut().set_text(messy);
        app.cur_mut().set_source(messy);
        let _ = update(&mut app, Message::FormatNow);
        eprintln!("AFTER FORMAT: {:?}", app.source());
        assert!(
            app.source()
                .contains("Table users {\n  id integer [pk, increment]\n}"),
            "{}",
            app.source()
        );
        // one undo step restores the original
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        assert_eq!(app.source(), messy, "single undo unit");
        // second format is a no-op ("already formatted")
        let _ = update(&mut app, Message::FormatNow);
        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_keeps_comments_and_skips_parse_errors() {
        let mut app = App::default();
        let noted = "// hello\nTable users {   id  integer }\n";
        app.editor_mut().set_text(noted);
        app.cur_mut().set_source(noted);
        let _ = update(&mut app, Message::FormatNow);
        assert_eq!(
            app.source(),
            "// hello\nTable users {\n  id integer\n}\n",
            "formatted with the comment preserved"
        );
        let broken = "Table users { id integer";
        app.editor_mut().set_text(broken);
        app.cur_mut().set_source(broken);
        let _ = update(&mut app, Message::FormatNow);
        assert_eq!(app.source(), broken, "broken source untouched");
    }

    #[test]
    fn fmt_on_save_is_default_and_toggles() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-fos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("DBML_UI_CONFIG", dir.join("config"));
        let mut app = App::default();
        assert!(app.config.fmt_on_save, "on by default");
        let _ = update(&mut app, Message::ToggleFmtOnSave);
        assert!(!app.config.fmt_on_save);
        assert!(!crate::config::Config::load().fmt_on_save, "persisted");
        let _ = update(&mut app, Message::ToggleFmtOnSave);
        assert!(app.config.fmt_on_save);
        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_formats_the_file() {
        let _g = crate::config::CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("dbml-ui-fsv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DBML_UI_CONFIG", dir.join("config"));
        let mut app = App::default();
        let messy = "Table   users {\nid integer [pk,increment]\n}\n";
        app.editor_mut().set_text(messy);
        app.cur_mut().set_source(messy);
        let path = dir.join("out.dbml");
        app.save_file(Some(path.to_str().unwrap())).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("Table users {\n  id integer [pk, increment]\n}"),
            "{written}"
        );
        // off-switch: disable, save, raw bytes land
        let _ = update(&mut app, Message::ToggleFmtOnSave);
        let messy2 = "Table   users {\nemail varchar(80) [unique,not null]\n}\n";
        app.editor_mut().set_text(messy2);
        app.cur_mut().set_source(messy2);
        app.save_file(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), messy2);
        std::env::remove_var("DBML_UI_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_screen_is_not_collapsible() {
        let mut app = App::blank_start();
        assert!(!app.doc_open(), "no document on the start screen");
        // Space+e and the toggle message are both no-ops
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert!(!app.editor_collapsed);
        let _ = update(&mut app, Message::ToggleEditor);
        assert!(!app.editor_collapsed);
        // New diagram opens the editor pane
        let _ = update(&mut app, Message::NewDiagram);
        assert!(app.doc_open());
        assert!(!app.editor_collapsed);
        assert!(app.source().contains("Project"));
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn space_e_toggles_the_editor_pane() {
        let mut app = App {
            focus: Focus::Editor,
            ..App::default()
        };
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert!(app.editor_collapsed, "Space+e collapses");
        assert_eq!(app.focus, Focus::Canvas, "keys leave the hidden editor");
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert!(!app.editor_collapsed, "Space+e expands again");
        // the button message does the same
        let _ = update(&mut app, Message::ToggleEditor);
        assert!(app.editor_collapsed);
        // a non-e key after Space processes normally: the leader is consumed
        let mut app = App::default();
        let before = app.editor_mut().buf.pos();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert!(!app.editor_collapsed);
        assert_ne!(app.editor_mut().buf.pos(), before, "the l moved the cursor");
    }

    #[test]
    fn space_types_in_insert_mode() {
        let mut app = insert_app();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert!(!app.editor_collapsed, "no leader in insert mode");
        assert!(app.source().contains(" e"), "space and e were typed");
    }

    #[test]
    fn k_hovers_the_word_at_the_caret() {
        let mut app = App::default();
        // caret on "users" in its "Table users {" row, col 6
        let row = app
            .editor()
            .buf
            .lines()
            .iter()
            .position(|l| l.contains("Table users"))
            .expect("sample has the users table");
        app.editor_mut().jump(row, 6);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('K'), KeyModifiers::NONE),
        );
        let hover = app.editor_hover.expect("K sets the hover point");
        // the point sits in the caret cell and resolves to the same word
        let pos = crate::canvas::editor::px_to_pos(&app, hover);
        assert_eq!((pos.row, pos.col), (row, 6), "on \"users\"");
        let doc = crate::canvas::editor::doc_hover_at(&app, hover)
            .expect("doc tooltip for the table name");
        assert_eq!(doc.title, "Table users");
        // any other editor key dismisses it
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert!(app.editor_hover.is_none());
        // K in insert mode just types (no hover)
        let mut app = insert_app();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('K'), KeyModifiers::NONE),
        );
        assert!(app.editor_hover.is_none(), "insert K types");
        assert!(app.source().contains('K'), "the K landed in the buffer");
    }

    #[test]
    fn horizontal_follow_when_pane_too_narrow() {
        let mut app = App {
            window_size: (500.0, 900.0), // pane ~188px: ~15 chars of text
            ..App::default()
        };
        let cw = crate::canvas::editor::char_width();
        // cursor at the end of the longest SAMPLE_DBML line
        let (row, len) = app
            .editor()
            .buf
            .lines()
            .iter()
            .enumerate()
            .map(|(i, l)| (i, l.chars().count()))
            .max_by_key(|&(_, l)| l)
            .unwrap();
        assert!(
            len as f32 * cw > app.editor_pane_width() - crate::canvas::editor::GUTTER,
            "fixture needs a line wider than the pane"
        );
        app.editor_mut().jump(row, len);
        app.follow_cursor_horizontal();
        assert!(app.cur().editor_hscroll > 0.0, "scrolled right");
        // the cursor cell is back inside the pane
        let cursor_right = crate::canvas::editor::GUTTER + len as f32 * cw - app.cur().editor_hscroll;
        assert!(
            cursor_right <= app.editor_pane_width() + 1.0,
            "{cursor_right} vs pane {}",
            app.editor_pane_width()
        );

        // moving to col 0 scrolls back to the line start
        app.editor_mut().jump(row, 0);
        app.follow_cursor_horizontal();
        assert_eq!(app.cur().editor_hscroll, 0.0);

        // through real keys: $ then 0
        app.editor_mut().jump(row, 0);
        app.cur_mut().editor_hscroll = 0.0;
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('$'), KeyModifiers::NONE),
        );
        assert!(app.cur().editor_hscroll > 0.0, "$ scrolls to line end");
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('0'), KeyModifiers::NONE),
        );
        assert_eq!(app.cur().editor_hscroll, 0.0, "0 scrolls back");
    }

    #[test]
    fn horizontal_follow_never_scrolls_past_line_end() {
        let mut app = App {
            window_size: (500.0, 900.0),
            ..App::default()
        };
        let cw = crate::canvas::editor::char_width();
        let text_w = app.editor_pane_width() - crate::canvas::editor::GUTTER;
        let (row, len) = app
            .editor()
            .buf
            .lines()
            .iter()
            .enumerate()
            .map(|(i, l)| (i, l.chars().count()))
            .max_by_key(|&(_, l)| l)
            .unwrap();
        app.editor_mut().jump(row, len);
        app.follow_cursor_horizontal();
        let max = (len as f32 * cw + cw - text_w).max(0.0);
        assert!(
            app.cur().editor_hscroll <= max + 0.5,
            "{} vs {max}",
            app.cur().editor_hscroll
        );
        // a short line relaxes the scroll entirely
        let short = app
            .editor()
            .buf
            .lines()
            .iter()
            .enumerate()
            .find(|(_, l)| (l.chars().count() as f32) * cw < text_w)
            .map(|(i, _)| i)
            .unwrap();
        app.editor_mut().jump(short, 0);
        app.cur_mut().editor_hscroll = 500.0; // stale scroll from a wider line
        app.follow_cursor_horizontal();
        assert_eq!(app.cur().editor_hscroll, 0.0, "clamped for a fitting line");
    }

    #[test]
    fn horizontal_wheel_scrolls_and_clamps() {
        // one long trailing comment line gives the pane horizontal overflow
        let mut body = String::new();
        for i in 0..60 {
            body.push_str(&format!("  col{i} integer [not null]\n"));
        }
        let mut app =
            App::with_source(format!("Table users {{\n{body}}}\n-- {}\n", "x".repeat(200)));
        app.window_size = (500.0, 900.0); // narrow pane under a 203-col line
        let max = app.max_hscroll();
        assert!(max > 0.0, "fixture needs horizontal overflow");
        let scroll_before = app.cur().editor_scroll;

        // a big two-finger pan right (app-space h > 0) runs to the clamp
        let _ = update(&mut app, Message::EditorScrolled { v: 0.0, h: 400.0 });
        assert!(
            (app.cur().editor_hscroll - max).abs() < 0.01,
            "{} vs max {max}",
            app.cur().editor_hscroll
        );
        // a pure-horizontal wheel never touches the vertical scroll
        assert_eq!(app.cur().editor_scroll, scroll_before);

        // and panning back left clamps at the line start
        let _ = update(&mut app, Message::EditorScrolled { v: 0.0, h: -10_000.0 });
        assert_eq!(app.cur().editor_hscroll, 0.0);

        // pure-vertical wheel leaves the horizontal offset alone
        let _ = update(&mut app, Message::EditorScrolled { v: 2.0, h: 0.0 });
        assert_eq!(app.cur().editor_hscroll, 0.0, "vertical wheel is vertical-only");
        assert_eq!(app.cur().editor_scroll, scroll_before + 2.0, "v still scrolls");
    }

    #[test]
    fn entering_insert_does_not_open_completions() {
        // regression: the i/a/o keys that enter insert are alphanumeric, so
        // the auto-trigger fired for the entering keypress itself — the
        // popup then ate the first Esc, needing two Escapes to leave insert
        for enter in ['i', 'a', 'o'] {
            let mut app = App::default();
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(enter), KeyModifiers::NONE),
            );
            assert_eq!(app.editor_mut().mode, crate::editor::Mode::Insert, "{enter}");
            assert!(app.completion.is_none(), "{enter} opened a popup");
            // and a single Esc leaves insert mode
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE),
            );
            assert_eq!(app.editor_mut().mode, crate::editor::Mode::Normal, "{enter}");
        }
    }

    #[test]
    fn navigation_moves_selection() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        let start = app.completion.as_ref().unwrap().selected;
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.completion.as_ref().unwrap().selected, start + 1);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Up, KeyModifiers::NONE),
        );
        assert_eq!(app.completion.as_ref().unwrap().selected, start);
    }

    #[test]
    fn no_popup_inside_strings_or_comments() {
        let mut app = insert_app();
        // cursor inside the project Note string
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(2, 10));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        assert!(app.completion.is_none(), "in prose: {:?}", app.completion);
    }

    #[test]
    fn backspace_refilters_and_closes_on_empty() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        assert!(app.completion.is_some());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(
            app.completion.is_some(),
            "still a 1-char... closes below 2? prefix 'T' refilter keeps >=1 matches"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none(), "empty prefix closes");
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use std::time::Duration;

    fn tall_app(lines: usize) -> App {
        let mut src = String::new();
        for i in 0..lines {
            src.push_str(&format!("Table t{i} {{\n  id integer [pk]\n}}\n"));
        }
        App::with_source(src)
    }

    fn force_finish(app: &mut App) {
        if let Some(a) = app.cur_mut().scroll_anim.as_mut() {
            a.started -= Duration::from_secs(1);
        }
        let _ = update(app, Message::Tick);
    }

    #[test]
    fn ctrl_d_couples_cursor_and_uses_half_page_duration() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let page = app.page_lines();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("animation running");
        assert!((anim.duration - 0.25).abs() < 1e-3, "250ms: {anim:?}");
        assert!(
            (anim.scroll_to - page / 2.0).abs() < 1.0,
            "half page: {anim:?}"
        );
        // neoscroll moves the cursor WITH the view, not ahead of it...
        assert_eq!(app.editor_mut().buf.pos().row, 0, "cursor not jumped");
        force_finish(&mut app);
        assert!((app.cur().editor_scroll - page / 2.0).abs() < 1.0);
        // ...but 'scrolloff' pulls a cursor inside the top margin out to
        // the 5-line boundary while the content scrolls (vim ctrl-d from
        // the top of the window)
        assert_eq!(
            app.editor_mut().buf.pos().row,
            (page / 2.0 + SCROLLOFF).round() as usize,
            "cursor arrives at the scrolloff line of the new view"
        );
    }

    #[test]
    fn page_scroll_uses_the_full_page_duration() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::PageDown, KeyModifiers::NONE),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("anim");
        assert!((anim.duration - 0.45).abs() < 1e-3, "450ms: {anim:?}");
        assert!((anim.scroll_to - app.page_lines()).abs() < 1.0);
    }

    #[test]
    fn steps_are_whole_lines_and_ease_out() {
        // quadratic ease-out in space: at t=0.5, 75% of the distance is due
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.cur_mut().scroll_anim.as_mut().unwrap();
        let half = Duration::from_secs_f32(anim.duration / 2.0);
        anim.started -= half;
        let range = (anim.scroll_to - anim.scroll_from).abs();
        let _ = update(&mut app, Message::Tick);
        let expected = (scroll_progress(0.5) * range).round();
        assert!(
            (app.cur().editor_scroll - expected).abs() < 0.01,
            "half time -> 75% distance: got {} want {expected}",
            app.cur().editor_scroll
        );
        assert_eq!(app.cur().editor_scroll, app.cur().editor_scroll.round(), "whole lines");
    }

    #[test]
    fn repeated_ctrl_d_stacks_to_a_full_page() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        let after_one = app.cur().editor_scroll;

        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().unwrap();
        assert!(
            (anim.scroll_to - after_one - app.page_lines() / 2.0).abs() < 1.0,
            "second ctrl-d extends by another half page: {anim:?}"
        );
    }

    #[test]
    fn tick_completes_the_animation_at_the_target() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let to = app.cur().scroll_anim.as_ref().unwrap().scroll_to;
        force_finish(&mut app);
        assert!(app.cur().scroll_anim.is_none());
        assert!((app.cur().editor_scroll - to).abs() < 0.001, "lands exactly");
        assert!(app.cur().editor_scroll >= 0.0);
    }

    #[test]
    fn scroll_clamps_at_buffer_bottom() {
        let mut app = tall_app(40);
        app.window_size = (1400.0, 900.0);
        for _ in 0..12 {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
            );
            force_finish(&mut app);
        }
        assert!(
            app.cur().editor_scroll <= app.max_scroll() + 0.001,
            "clamped: {} vs {}",
            app.cur().editor_scroll,
            app.max_scroll()
        );
    }

    #[test]
    fn scroll_progress_curve_shape() {
        // neoscroll quadratic easing, inverted: distance at time y
        assert_eq!(scroll_progress(0.0), 0.0);
        assert_eq!(scroll_progress(1.0), 1.0);
        assert!((scroll_progress(0.5) - 0.75).abs() < 1e-6, "ease-out");
        let mut prev = -0.001;
        for i in 0..=100 {
            let v = scroll_progress(i as f32 / 100.0);
            assert!(v > prev, "monotonic at {i}");
            assert!((0.0..=1.0).contains(&v));
            prev = v;
        }
    }

    #[test]
    fn cursor_at_buffer_bottom_does_not_yank_backward() {
        // regression: cursor_from was reconstructed from the scroll delta,
        // so a clamped cursor jumped UP before animating back down
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let last = app.editor_mut().buf.line_count() - 1;
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(last, 0));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            app.editor_mut().buf.pos().row,
            last,
            "cursor never leaves the bottom"
        );
        // force the animation to its end: cursor still at the bottom
        if let Some(a) = app.cur_mut().scroll_anim.as_mut() {
            a.started -= std::time::Duration::from_secs(1);
        }
        let _ = update(&mut app, Message::Tick);
        assert_eq!(app.editor_mut().buf.pos().row, last);
        assert!(app.cur().editor_scroll >= 0.0);
    }

    #[test]
    fn fitting_document_cursor_glides_without_scrolling() {
        // neoscroll: with a document that fits the page the window never
        // scrolls but the cursor still glides line by line — never an
        // instant jump (regression: this case jumped instantly)
        // tall_app(8): 24 lines, comfortably inside the ~37.8-line page at
        // 900px. (The 40-line default sample does NOT fit the true page —
        // it only looked like it did while page_lines underestimated the
        // chrome and hid its tail under the status bar.)
        let mut app = tall_app(8);
        app.window_size = (1400.0, 900.0);
        assert!(
            app.max_scroll() == 0.0,
            "precondition: document fits the page"
        );
        let page = app.page_lines();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("cursor animation runs");
        assert_eq!(app.cur().editor_scroll, 0.0, "window has nothing to scroll");
        assert_eq!(
            anim.cursor,
            Some((0, (page / 2.0).round() as usize)),
            "cursor glides from row 0"
        );
        // mid-animation: cursor advanced, window static
        if let Some(a) = app.cur_mut().scroll_anim.as_mut() {
            a.started -= std::time::Duration::from_secs_f32(a.duration / 2.0);
        }
        let _ = update(&mut app, Message::Tick);
        assert_eq!(app.cur().editor_scroll, 0.0, "window still static");
        assert!(
            app.editor_mut().buf.pos().row > 0,
            "cursor mid-glide at {}",
            app.editor_mut().buf.pos().row
        );
        force_finish(&mut app);
        assert_eq!(app.editor_mut().buf.pos().row, (page / 2.0).round() as usize);
        // Ctrl-u walks it back up the same way
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        assert_eq!(app.editor_mut().buf.pos().row, 0);
    }

    #[test]
    fn near_eof_window_freezes_and_cursor_continues_alone() {
        // the user-observed behavior: with less than half a screen left,
        // neoscroll's stop_eof freezes the window at its clamp while the
        // cursor continues alone to its target
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let last = app.editor_mut().buf.line_count() - 1;
        let max = app.max_scroll();
        app.cur_mut().editor_scroll = max - 5.0;
        let cursor_at = last - 10;
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(cursor_at, 0));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("animates");
        assert!(
            (anim.scroll_to - max).abs() < 0.5,
            "window stops at its clamp (stop_eof): {anim:?}"
        );
        assert_eq!(
            anim.cursor,
            Some((cursor_at, last)),
            "cursor continues to the last line"
        );
        force_finish(&mut app);
        assert_eq!(app.editor_mut().buf.pos().row, last);
        assert!((app.cur().editor_scroll - max).abs() < 0.5);
    }

    #[test]
    fn ctrl_d_then_gg_does_not_arm_the_delete_operator() {
        // regression: ctrl+d to the bottom then gg deleted the whole
        // document — the vim D operator must never see the ctrl+d key
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let rows = app.editor().buf.line_count();
        for _ in 0..12 {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
            );
            force_finish(&mut app);
        }
        let after_scroll = app.editor().buf.line_count();
        assert_eq!(after_scroll, rows, "scrolling deletes nothing");
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        assert_eq!(
            app.editor().buf.line_count(),
            rows,
            "gg after ctrl+d must not delete"
        );
        assert_eq!(app.editor().buf.pos().row, 0, "gg jumped to the top");
        // and the same for ctrl+u + u: no undo may fire
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        assert_eq!(
            app.editor().buf.line_count(),
            rows,
            "u after ctrl+u must not undo"
        );
    }

    #[test]
    fn a_bare_chord_letter_right_after_the_scroll_is_rollout() {
        // releasing ctrl a few ms before d (or the chord's autorepeat
        // tail) delivers a bare `d` right after the scroll: that is chord
        // residue, not a vim operator — `gg` must not turn it into `dgg`
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let rows = app.editor().buf.line_count();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        // the rolled-off bare d arrives within the rollout window
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            app.editor().buf.line_count(),
            rows,
            "residue d deletes nothing"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        assert_eq!(app.editor().buf.line_count(), rows, "gg deletes nothing");
        // ctrl+u's rollout letter is `u` — a bare u right after must not
        // fire an undo
        let src = app.editor().buf.lines().to_vec();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        let after = app.editor().buf.lines().to_vec();
        assert_eq!(after, src, "residue u undoes nothing");
    }

    #[test]
    fn a_genuine_d_after_the_rollout_window_still_arms_the_operator() {
        // the residue window must not eat real vim keys: `d` pressed well
        // after a scroll chord still arms the operator (then Esc cancels)
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        app.last_scroll_chord =
            Some(std::time::Instant::now() - SCROLL_CHORD_ROLLOUT - std::time::Duration::from_millis(1));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            app.editor().showcmd(),
            Some("d".to_string()),
            "a real d still arms the operator"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(app.editor().showcmd(), None, "esc cancels");
    }

    #[test]
    fn ctrl_modified_keys_never_drive_vim_directly() {
        // even if a ctrl-modified letter reaches the editor, it must not
        // arm operators, undo, or type — vim's keymap is unmodified-only
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let src = app.editor().buf.lines().to_vec();
        let rows = app.editor().buf.line_count();
        // a ctrl'd d delivered straight to the editor (bypassing the app
        // scroll): arms nothing
        let _ = app.editor_mut().handle_key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(app.editor().showcmd(), None, "ctrl+d arms no operator");
        // ctrl+u straight to the editor: no undo
        let _ = app.editor_mut().handle_key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(app.editor().buf.lines(), src, "ctrl+u does not undo");
        // in insert mode a ctrl'd letter does not type
        let _ = app.editor_mut().handle_key(KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::NONE,
        ));
        let _ = app.editor_mut().handle_key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL,
        ));
        let _ = app.editor_mut().handle_key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        ));
        assert_eq!(
            app.editor().buf.lines(),
            src,
            "insert-mode ctrl chord types nothing"
        );
        let _ = rows;
    }

    #[test]
    fn wheel_cancels_the_animation() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert!(app.cur().scroll_anim.is_some());
        let _ = update(&mut app, Message::EditorScrolled { v: -2.0, h: 0.0 });
        assert!(app.cur().scroll_anim.is_none(), "wheel takes over");
    }

    #[test]
    fn ctrl_u_from_the_bottom_margin_walks_the_cursor_to_the_margin_first() {
        // vim 'scrolloff': scrolling up with the cursor within 5 lines of
        // the window bottom first moves the cursor up to the 5-line
        // boundary, THEN the view scrolls with the cursor pinned there
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let page = app.page_lines();
        let max = app.max_scroll();
        // cursor 3 lines above the lowest visible line (inside the margin)
        let scroll = max - 2.0;
        app.cur_mut().editor_scroll = scroll;
        let cursor_at = (scroll + page - 4.0).round() as usize;
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(cursor_at, 0));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("animates");
        // the view scrolls a half page up...
        assert!(
            (anim.scroll_to - (scroll - page / 2.0)).abs() < 1.0,
            "view up half a page: {anim:?}"
        );
        // ...while the cursor lands 5 lines above the new view's bottom
        let margin_row = anim.scroll_to + page - 1.0 - SCROLLOFF;
        assert_eq!(
            anim.cursor,
            Some((cursor_at, margin_row.round() as usize)),
            "cursor walked out to the scrolloff boundary"
        );
        force_finish(&mut app);
        assert_eq!(app.editor_mut().buf.pos().row, margin_row.round() as usize);
    }

    #[test]
    fn ctrl_u_outside_the_margin_keeps_the_cursor_screen_line() {
        // from well above the bottom margin the scroll is static: the
        // cursor keeps its screen position while the content moves
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let page = app.page_lines();
        app.cur_mut().editor_scroll = 40.0;
        let cursor_at = 44.0; // 6+ lines above the view's bottom edge
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(cursor_at as usize, 0));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        let anim = app.cur().scroll_anim.as_ref().expect("animates");
        assert_eq!(
            anim.cursor,
            Some((cursor_at as usize, (cursor_at - page / 2.0).round() as usize)),
            "static scroll: cursor moves exactly half a page"
        );
    }

    #[test]
    fn j_and_k_scroll_the_view_to_honor_scrolloff() {
        // stepping into the bottom margin scrolls the view under the
        // cursor instead of letting it inside the 5-line zone
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let page = app.page_lines();
        // cursor at the last line that satisfies the bottom margin
        let scroll = 10.0;
        app.cur_mut().editor_scroll = scroll;
        let cursor_at = (scroll + page - 6.0).round() as usize;
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(cursor_at, 0));
        // one more j must scroll the view by one, cursor pinned at the margin
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(app.editor_mut().buf.pos().row, cursor_at + 1);
        assert!(
            (app.cur().editor_scroll - (scroll + 1.0)).abs() < 0.5,
            "view followed by one line: {}",
            app.cur().editor_scroll
        );
        // and k back up does NOT un-scroll: vim only follows the cursor
        // out of a margin, never back into one — the view stays until k
        // approaches the top margin
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        assert_eq!(app.editor_mut().buf.pos().row, cursor_at);
        assert!(
            (app.cur().editor_scroll - (scroll + 1.0)).abs() < 0.5,
            "view stayed: {}",
            app.cur().editor_scroll
        );
    }

    #[test]
    fn vim_jumps_land_instantly_and_cancel_neoscroll() {
        // vim semantics: G / gg / click-to-jump snap the viewport; only the
        // explicit neoscroll scrolls (Ctrl-d / Ctrl-u, page keys) animate
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        // a running half-page animation...
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert!(app.cur().scroll_anim.is_some());
        // ...is cancelled by G, which lands instantly at the bottom
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('G'), KeyModifiers::NONE),
        );
        assert!(app.cur().scroll_anim.is_none(), "jump cancels the animation");
        assert_eq!(
            app.editor_mut().buf.pos().row,
            app.editor_mut().buf.line_count() - 1,
            "cursor at the last line"
        );
        assert!(
            (app.cur().editor_scroll - app.max_scroll()).abs() < 0.5,
            "view at the bottom: {} vs max {}",
            app.cur().editor_scroll,
            app.max_scroll()
        );
        // gg is instant too
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        assert!(app.cur().scroll_anim.is_none());
        assert_eq!(app.cur().editor_scroll, 0.0, "view back at the top");
    }

    /// Ground truth measured with a real iced 0.13.1 layout pass over
    /// `view::root` (flex/column/container/button metrics, text line
    /// height 1.3x size): at a 900px window the editor canvas measures
    /// 780.5px tall. If the root layout chrome ever changes, re-measure
    /// this and `view::EDITOR_CHROME` together.
    const CANVAS_H_AT_900: f32 = 780.5;

    #[test]
    fn page_lines_covers_the_real_chrome() {
        // regression: page_lines subtracted a stale 62px chrome (written
        // before the two toolbars existed), so max_scroll stopped ~2 lines
        // short of the buffer end and the last lines rendered beneath the
        // status bar, clipped out of view
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let canvas_h = app.page_lines() * crate::canvas::editor::LINE_HEIGHT;
        assert!(
            (canvas_h - CANVAS_H_AT_900).abs() < 0.6,
            "page_lines shows {canvas_h:.1}px of editor canvas, \
             the real layout gives {CANVAS_H_AT_900}px"
        );
    }

    #[test]
    fn last_line_sits_on_screen_at_max_scroll() {
        // the user-visible invariant: scrolled all the way down, the last
        // line's drawn text (centered in its line box) is inside the true
        // canvas — nothing hides beneath the status bar. Scroll clamps
        // then round, so allow the half-line rounding overshoot.
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let last = app.editor_mut().buf.line_count() - 1;
        app.cur_mut().editor_scroll = app.max_scroll();
        let y = crate::canvas::editor::pos_to_px(
            crate::editor::buffer::Pos { row: last, col: 0 },
            app.cur().editor_scroll,
            0.0,
        )
        .y;
        let text_center = y + crate::canvas::editor::LINE_HEIGHT * 0.5;
        assert!(
            text_center <= CANVAS_H_AT_900 + 0.6,
            "last line's text center at {text_center} is below the \
             {CANVAS_H_AT_900}px canvas: the line hides under the status bar"
        );
        assert!(y >= 0.0, "overscrolled past the buffer start: {y}");
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;
    use dbml_ui::SAMPLE_DBML;

    fn key(mods: KeyModifiers, c: char) -> Message {
        Message::KeyPressed(KeyCode::Char(c), mods)
    }

    #[test]
    fn tab_drag_midpoint_swaps_and_slides_the_neighbor() {
        // Chrome rule: the dragged tab's edge crossing a neighbor slot's
        // midpoint swaps the two; the displaced neighbor starts a slot
        // away and eases home (its slide), the dragged tab is
        // pointer-driven
        let mut app = two_tab_app();
        app.window_cursor = Some(Point::new(STRIP_LEFT, 12.0));
        let _ = update(&mut app, Message::TabPressed(0));
        // the swap fires when the dragged tab's leading edge crosses the
        // right neighbor slot's midpoint: vx >= 1.5*SLOT - TAB_VISUAL_W
        let x = STRIP_LEFT + 1.5 * TAB_SLOT_W - TAB_VISUAL_W + STRIP_LEFT + 20.0;
        let vx = x - STRIP_LEFT - 0.0; // grab_dx = STRIP_LEFT (pressed at slot 0 left edge)
        let _ = update(&mut app, Message::MouseMovedWindow(x, 12.0));
        assert_eq!(app.active, 1, "leading edge past the neighbor midpoint: swapped");
        assert_eq!(app.tab_anim[0], TAB_SLOT_W, "displaced neighbor at the old slot");
        assert!(
            (app.tab_anim[1] - (vx - TAB_SLOT_W)).abs() < 0.5,
            "dragged tab follows the pointer: {:?}",
            app.tab_anim[1]
        );
        // the tick eases the displaced neighbor home
        let before = app.tab_anim[0];
        let _ = update(&mut app, Message::Tick);
        assert!(app.tab_anim[0] < before && app.tab_anim[0] > 0.0);
        // and on release the dragged tab glides home from its pose
        let _ = update(&mut app, Message::MouseReleased);
        assert!(
            (app.tab_anim[1] - (90.0 - TAB_SLOT_W)).abs() > 1.0,
            "release leaves the drag pose to ease"
        );
        let _ = update(&mut app, Message::Tick);
        for _ in 0..30 {
            let _ = update(&mut app, Message::Tick);
        }
        assert!(
            app.tab_anim.iter().all(|o| o.abs() < 0.5),
            "all offsets settle: {:?}",
            app.tab_anim
        );
    }

    #[test]
    fn dragging_the_right_tab_left_keeps_both_tabs_visible() {
        // user report: dragging the right tab over the left one made a tab
        // vanish from the strip (clickable but invisible)
        let mut app = two_tab_app();
        app.window_cursor = Some(Point::new(STRIP_LEFT + TAB_SLOT_W + 80.0, 12.0));
        let _ = update(&mut app, Message::TabPressed(1));
        // walk the pointer left in small steps across the midpoint
        let mut x = STRIP_LEFT + TAB_SLOT_W + 80.0;
        while x > STRIP_LEFT + 10.0 {
            x -= 20.0;
            let _ = update(&mut app, Message::MouseMovedWindow(x, 12.0));
            // every tab's drawn slot+offset must land inside the strip
            for (i, off) in app.tab_anim.iter().enumerate() {
                let drawn = STRIP_LEFT + i as f32 * TAB_SLOT_W + off;
                assert!(
                    drawn >= 0.0 && drawn <= STRIP_LEFT + (app.tabs.len() - 1) as f32 * TAB_SLOT_W,
                    "tab {i} drawn off-strip at {drawn} (anim {:?}, tabs {})",
                    app.tab_anim,
                    app.tabs.len()
                );
            }
            assert_eq!(app.tabs.len(), 2, "no tab lost");
        }
        // settled after release + ticks
        let _ = update(&mut app, Message::MouseReleased);
        for _ in 0..40 {
            let _ = update(&mut app, Message::Tick);
        }
        assert!(app.tab_anim.iter().all(|o| o.abs() < 0.5), "{:?}", app.tab_anim);
        assert_eq!(app.tabs.len(), 2);
    }

    fn two_tab_app() -> App {
        // tab 0: the sample; tab 1: another document
        let mut app = App::with_source(SAMPLE_DBML.to_string());
        let _ = update(&mut app, Message::NewTab);
        app.cur_mut().doc_open = true; // the start tab becomes a document
        app.cur_mut().editor.set_text("Table other { id integer }");
        app.cur_mut().set_source("Table other { id integer }");
        app.cur_mut().saved_text = app.source().to_string();
        app
    }

    #[test]
    fn new_tab_shows_the_start_screen_and_preserves_the_previous_tab() {
        let mut app = App::with_source(SAMPLE_DBML.to_string());
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 't'));
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1, "the new tab is active");
        assert!(!app.doc_open(), "it is the start screen");
        assert_eq!(app.source(), "");
        assert_eq!(app.cur().title(), "Start");
        // the first tab is untouched and reachable again
        let _ = update(&mut app, Message::TabPressed(0));
        assert_eq!(app.active, 0);
        assert_eq!(app.source(), SAMPLE_DBML);
        assert!(app.doc_open());
        assert_eq!(app.cur().title(), "Untitled");
    }

    #[test]
    fn switching_tabs_swaps_buffer_editor_and_view_state() {
        let mut app = two_tab_app();
        // per-tab cursor, vim mode, scroll and zoom (tab 0 is the sample)
        let _ = update(&mut app, Message::TabPressed(0));
        app.editor_mut().jump(1, 3);
        let _ = update(&mut app, key(KeyModifiers::NONE, 'i')); // insert mode
        app.cur_mut().editor_scroll = 7.0;
        app.cur_mut().zoom = 55.0;
        let _ = update(&mut app, Message::TabPressed(1));
        assert_eq!(app.source(), "Table other { id integer }");
        assert_eq!(app.editor().mode, editor::Mode::Normal, "vim state is per tab");
        assert_eq!(app.cur().editor_scroll, 0.0);
        assert_eq!(app.cur().zoom, crate::view::BASE_ZOOM);
        let _ = update(&mut app, Message::TabPressed(0));
        assert_eq!(app.source(), SAMPLE_DBML);
        assert_eq!(app.editor().mode, editor::Mode::Insert, "mode survives the round trip");
        assert_eq!(app.editor().buf.cursor(), (1, 3), "cursor survives");
        assert_eq!(app.cur().editor_scroll, 7.0);
        assert_eq!(app.cur().zoom, 55.0);
    }

    #[test]
    fn unsaved_changes_and_undo_history_survive_tab_switches() {
        let mut app = two_tab_app(); // active: tab 1 ("other")
        // unsaved edit at the end of tab 1
        let last = app.editor().buf.line_count() - 1;
        app.editor_mut().jump(last, 30);
        let _ = update(&mut app, key(KeyModifiers::NONE, 'i'));
        let _ = update(&mut app, key(KeyModifiers::NONE, '!'));
        assert!(app.cur().is_modified());
        let _ = update(&mut app, Message::TabPressed(0));
        let _ = update(&mut app, Message::TabPressed(1));
        assert!(app.source().ends_with("integer }!"), "{}", app.source());
        assert_eq!(
            app.editor().mode,
            editor::Mode::Insert,
            "insert mode is per tab and came back with it"
        );
        // leave insert, then one u undoes the edit: history is per buffer
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'u'));
        assert_eq!(app.source(), "Table other { id integer }");
        assert!(!app.is_modified());
    }

    #[test]
    fn move_element_reorders_by_remove_and_insert() {
        let mut v = vec![0, 1, 2, 3];
        move_element(&mut v, 0, 3);
        assert_eq!(v, vec![1, 2, 3, 0]);
        move_element(&mut v, 3, 1);
        assert_eq!(v, vec![1, 0, 2, 3]);
        move_element(&mut v, 2, 2);
        assert_eq!(v, vec![1, 0, 2, 3], "same slot is a no-op");
    }

    #[test]
    fn dragging_a_tab_reorders_live_and_the_active_tab_follows() {
        let mut app = two_tab_app(); // [sample, other], "other" active
        // press tab 0 (the sample) ...
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + 10.0, 12.0));
        let _ = update(&mut app, Message::TabPressed(0));
        assert_eq!(app.active, 0);
        // ... and drag it well past the second slot: it drops at the last
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + 2.5 * TAB_SLOT_W, 12.0));
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1, "the dragged tab stays active under the pointer");
        assert_eq!(app.source(), SAMPLE_DBML, "the view follows the dragged tab");
        assert_eq!(app.tabs[0].title(), "Untitled");
        // release drops it in place; further moves do nothing
        let _ = update(&mut app, Message::MouseReleased);
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + 90.0 * TAB_SLOT_W, 12.0));
        assert_eq!(app.active, 1);
        assert_eq!(app.tabs[0].title(), "Untitled");

        // and the other way: drag the last tab left onto the first slot
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + TAB_SLOT_W + 10.0, 12.0));
        let _ = update(&mut app, Message::TabPressed(1));
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + 2.0, 12.0));
        assert_eq!(app.active, 0, "dragged back to the front");
        assert_eq!(app.source(), SAMPLE_DBML);
        assert_eq!(app.tabs[1].title(), "Untitled");
    }

    #[test]
    fn a_tab_click_without_drag_only_activates() {
        let mut app = two_tab_app();
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + TAB_SLOT_W + 5.0, 12.0));
        let _ = update(&mut app, Message::TabPressed(1));
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + TAB_SLOT_W + 7.0, 12.0));
        let _ = update(&mut app, Message::MouseReleased);
        assert_eq!(app.active, 1);
        assert_eq!(app.tabs.len(), 2, "no reorder below the drag threshold");
        assert_eq!(app.tabs[0].title(), "Untitled");
    }

    #[test]
    fn close_tab_activates_the_right_neighbor_else_the_last() {
        let mut app = two_tab_app(); // [sample, other]
        // close a BACKGROUND tab on the left: the active choice survives
        let _ = update(&mut app, Message::TabPressed(1));
        let _ = update(&mut app, Message::CloseTab(0));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active, 0);
        assert_eq!(app.source(), "Table other { id integer }");

        // close the active tab: the tab to its right wins, else the last
        let mut app = App::with_source(SAMPLE_DBML.to_string());
        let _ = update(&mut app, Message::NewTab);
        let _ = update(&mut app, Message::NewTab); // [s, s, s] active 2
        let _ = update(&mut app, Message::TabPressed(1));
        let _ = update(&mut app, Message::CloseTab(1));
        assert_eq!(app.active, 1, "the right neighbor (now at the same index)");
        let _ = update(&mut app, Message::TabPressed(2));
        let _ = update(&mut app, Message::CloseTab(2));
        assert_eq!(app.active, 1, "no right neighbor: the new last tab");
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn closing_the_last_tab_leaves_a_fresh_start_screen() {
        let mut app = two_tab_app();
        let _ = update(&mut app, Message::CloseTab(0));
        let _ = update(&mut app, Message::CloseTab(0));
        assert_eq!(app.tabs.len(), 1, "the window stays alive");
        assert_eq!(app.active, 0);
        assert!(!app.doc_open());
        assert_eq!(app.source(), "");
    }

    #[test]
    fn close_all_leaves_one_start_tab() {
        let mut app = two_tab_app();
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 't')); // 3 tabs
        let _ = update(&mut app, Message::TabMenuCloseAll);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active, 0);
        assert!(!app.doc_open(), "a fresh start-screen tab remains");
        assert_eq!(app.source(), "");
    }

    #[test]
    fn ctrl_t_and_ctrl_w_new_and_close_tabs() {
        let mut app = App::with_source(SAMPLE_DBML.to_string());
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 't'));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 't'));
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.active, 2);
        // Ctrl+W works from insert mode too (Chrome-like global shortcut)
        let _ = update(&mut app, key(KeyModifiers::NONE, 'i'));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'w'));
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1, "the right neighbor took over");
        assert_eq!(app.editor().mode, editor::Mode::Normal, "no mode leaks across tabs");
    }

    #[test]
    fn the_tab_menu_closes_its_own_tab_and_dismisses_on_any_key() {
        let mut app = two_tab_app();
        // right-click the background tab (pointer where it last moved)
        let _ = update(&mut app, Message::MouseMovedWindow(STRIP_LEFT + 10.0, 12.0));
        let _ = update(&mut app, Message::TabMenu(0));
        let menu = app.tab_menu.as_ref().expect("menu open");
        assert_eq!(menu.tab, 0);
        assert_eq!(menu.at, Point::new(STRIP_LEFT + 10.0, 12.0), "anchored at the pointer");
        // Close from the menu: only that tab goes
        let _ = update(&mut app, Message::TabMenuClose);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.source(), "Table other { id integer }");
        assert!(app.tab_menu.is_none(), "the menu went with the switch");

        // the menu is modal: any key dismisses it without acting
        let mut app = two_tab_app();
        let _ = update(&mut app, Message::TabMenu(1));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'x'));
        assert!(app.tab_menu.is_none());
        assert_eq!(app.tabs.len(), 2, "the key did not reach the editor");
        // clicking elsewhere dismisses
        let _ = update(&mut app, Message::TabMenu(1));
        let _ = update(&mut app, Message::DismissTabMenu);
        assert!(app.tab_menu.is_none());
    }

    #[test]
    fn a_modified_tab_closes_without_a_prompt_and_only_itself() {
        // no autosave/confirm precedent in the codebase: closing is
        // Chrome-like (no prompt) and never touches the other tabs
        let mut app = two_tab_app();
        let _ = update(&mut app, Message::TabPressed(0));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'i'));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'x'));
        assert!(app.cur().is_modified());
        let _ = update(&mut app, Message::CloseTab(0));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.source(), "Table other { id integer }", "the other tab is intact");
    }

    #[test]
    fn tab_titles_track_files_and_dirty_state() {
        assert_eq!(Tab::blank().title(), "Start");
        assert_eq!(Tab::with_source("Table t {}".into()).title(), "Untitled");
        let mut tab = Tab::with_source("Table t {}".into());
        tab.file = Some(std::path::PathBuf::from("/tmp/notes.dbml"));
        assert_eq!(tab.title(), "notes.dbml");
        tab.editor.set_text("Table t { id }");
        assert!(tab.is_modified());
    }
}

#[cfg(test)]
mod clipboard_menu_tests {
    use super::*;
    use crate::clipboard::FakeClipboard;
    use crate::editor::buffer::Pos;

    fn regular_app() -> App {
        let mut app = App {
            config: crate::config::Config {
                vim: false,
                ..Default::default()
            },
            ..App::default()
        };
        app.editor_mut().set_text("hello world\nsecond line\n");
        app.cur_mut().set_source("hello world\nsecond line\n");
        app
    }

    /// Select "hello" (chars 0..5 of line 0) with shift+arrows.
    fn select_hello(app: &mut App) {
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, 0));
        for _ in 0..4 {
            let _ = update(app, Message::KeyPressed(KeyCode::Right, KeyModifiers::SHIFT));
        }
    }

    fn key(mods: KeyModifiers, c: char) -> Message {
        Message::KeyPressed(KeyCode::Char(c), mods)
    }

    // canvas-local px inside a buffer position's cell (pos_to_px alone
    // sits on the exact cell boundary, where the px_to_pos floor flips on
    // float rounding — a real pointer lands inside the cell)
    fn local_px(pos: Pos) -> Point {
        let p = crate::canvas::editor::pos_to_px(pos, 0.0, 0.0);
        Point::new(p.x + 2.0, p.y + 2.0)
    }

    // window px of a buffer position: the drag path converts back with
    // view::EDITOR_CANVAS_TOP, so the roundtrip must be exact
    fn window_px(pos: Pos) -> Point {
        let l = local_px(pos);
        Point::new(l.x, l.y + crate::view::EDITOR_CANVAS_TOP)
    }

    #[test]
    fn regular_ctrl_c_copies_to_system_and_keeps_the_selection() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        select_hello(&mut app);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'c'));
        assert_eq!(app.clipboard_text().as_deref(), Some("hello"));
        assert_eq!(app.message.as_deref(), Some("copied 5 chars"));
        assert_eq!(
            app.editor().buf.selection_text().as_deref(),
            Some("hello"),
            "the selection survives a copy"
        );
        assert_eq!(app.editor().register_lines(), vec!["hello".to_string()]);
    }

    #[test]
    fn clipboard_failures_become_toasts_not_panics() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard {
            fail_set: true,
            ..Default::default()
        }));
        select_hello(&mut app);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'c'));
        assert_eq!(app.message.as_deref(), Some("clipboard: no display"));
        // the register still got the text
        assert_eq!(app.editor().register_lines(), vec!["hello".to_string()]);
    }

    #[test]
    fn regular_ctrl_x_cuts_to_system() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        select_hello(&mut app);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'x'));
        assert_eq!(app.clipboard_text().as_deref(), Some("hello"));
        assert_eq!(app.editor().buf.line(0), " world");
        assert!(app.editor().buf.selection_anchor().is_none());
    }

    #[test]
    fn regular_ctrl_v_pastes_multiline_from_system() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::with_text("x\ny")));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert_eq!(app.editor().buf.lines(), ["x", "yhello world", "second line"]);
    }

    #[test]
    fn regular_paste_replaces_the_selection() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::with_text("PASTED")));
        select_hello(&mut app);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert_eq!(app.editor().buf.line(0), "PASTED world");
    }

    #[test]
    fn paste_falls_back_to_the_register_without_a_system_clipboard() {
        let mut app = regular_app();
        // an unreachable system clipboard: reads fail, so the internal
        // register (the pre-arboard paste source) is used
        app.clipboard = Some(Box::new(FakeClipboard {
            fail_get: true,
            ..Default::default()
        }));
        app.editor_mut().set_register(vec!["reg text".to_string()]);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert_eq!(app.editor().buf.line(0), "reg texthello world");

        // nothing anywhere: a toast, no panic
        app.editor_mut().set_register(Vec::new());
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert_eq!(app.message.as_deref(), Some("nothing to paste"));
    }

    #[test]
    fn crlf_paste_is_normalized() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::with_text("a\r\nb")));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert_eq!(app.editor().buf.lines(), ["a", "bhello world", "second line"]);
    }

    #[test]
    fn insert_mode_ctrl_v_pastes_from_system() {
        let mut app = App {
            clipboard: Some(Box::new(FakeClipboard::with_text("INS"))),
            ..App::default()
        }; // vim mode
        let _ = update(&mut app, key(KeyModifiers::NONE, 'i'));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'v'));
        assert!(app.source().starts_with("INS"));
    }

    #[test]
    fn vim_yank_reaches_the_system_clipboard() {
        // the sample: line 0 "Project blog {"
        let mut app = App {
            clipboard: Some(Box::new(FakeClipboard::default())),
            ..App::default()
        };
        // yy
        let _ = update(&mut app, key(KeyModifiers::NONE, 'y'));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'y'));
        assert_eq!(app.clipboard_text().as_deref(), Some("Project blog {\n"));

        // visual y (drain happens through after_edit after each key)
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'v'));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'e'));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'y'));
        assert_eq!(app.clipboard_text().as_deref(), Some("Project"));
    }

    #[test]
    fn vim_delete_leaves_the_system_clipboard_alone() {
        let mut app = App {
            clipboard: Some(Box::new(FakeClipboard::with_text("keepme"))),
            ..App::default()
        };
        let _ = update(&mut app, key(KeyModifiers::NONE, 'd'));
        let _ = update(&mut app, key(KeyModifiers::NONE, 'd'));
        assert_eq!(
            app.clipboard_text().as_deref(),
            Some("keepme"),
            "a delete is not a yank"
        );
        assert_eq!(app.editor().register_lines()[0], "Project blog {");
    }

    #[test]
    fn mouse_drag_selects_in_regular_mode() {
        let mut app = regular_app();
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 1 })));
        assert!(matches!(app.drag, Drag::EditorSelect));
        let to = window_px(Pos { row: 0, col: 4 });
        let _ = update(&mut app, Message::MouseMovedWindow(to.x, to.y));
        let _ = update(&mut app, Message::MouseReleased);
        assert_eq!(app.drag, Drag::None);
        assert_eq!(
            app.editor().buf.selection_text().as_deref(),
            Some("ello"),
            "anchor at col 1, drag to col 4"
        );
    }

    #[test]
    fn mouse_drag_clamps_when_it_leaves_the_pane() {
        let mut app = regular_app();
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 0 })));
        // way below/right of the buffer: clamps to the buffer end
        let _ = update(&mut app, Message::MouseMovedWindow(10_000.0, 10_000.0));
        let _ = update(&mut app, Message::MouseReleased);
        assert_eq!(
            app.editor().buf.selection_text().as_deref(),
            Some("hello world\nsecond line"),
            "drag extends to the last char"
        );
    }

    #[test]
    fn mouse_click_collapses_the_selection() {
        let mut app = regular_app();
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 3 })));
        let _ = update(&mut app, Message::MouseReleased);
        assert!(app.editor().buf.selection_anchor().is_none(), "a click places the caret");
        assert_eq!(app.editor().buf.pos(), Pos { row: 0, col: 3 });
    }

    #[test]
    fn mouse_drag_selects_in_vim_mode_too() {
        // gvim-style mouse selection: a drag in vim normal mode extends a
        // selection; a plain click collapses to a caret
        let mut app = App::default(); // vim mode is the default
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 1 })));
        assert!(matches!(app.drag, Drag::EditorSelect));
        let to = window_px(Pos { row: 0, col: 4 });
        let _ = update(&mut app, Message::MouseMovedWindow(to.x, to.y));
        let _ = update(&mut app, Message::MouseReleased);
        assert!(
            app.editor().buf.selection_text().is_some(),
            "drag selected in vim mode"
        );
        // a plain click collapses
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 3 })));
        let _ = update(&mut app, Message::MouseReleased);
        assert!(app.editor().buf.selection_anchor().is_none());
        assert_eq!(app.editor().buf.pos(), Pos { row: 0, col: 3 });
    }

    #[test]
    fn mouse_drag_selects_in_insert_mode_too() {
        let mut app = App::default();
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE));
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 1 })));
        let to = window_px(Pos { row: 0, col: 4 });
        let _ = update(&mut app, Message::MouseMovedWindow(to.x, to.y));
        let _ = update(&mut app, Message::MouseReleased);
        assert!(
            app.editor().buf.selection_text().is_some(),
            "drag selected in insert mode"
        );
    }

    #[test]
    fn editor_menu_flags_snapshot_at_open() {
        // no selection, empty clipboard: both disabled
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let menu = app.editor_menu.as_ref().expect("menu open");
        assert!(!menu.has_selection);
        assert!(!menu.clipboard_has_text);

        // a selection and clipboard text: both enabled
        select_hello(&mut app);
        // any key dismissed the menu above; reopen
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let menu = app.editor_menu.as_ref().expect("menu open");
        assert!(menu.has_selection);
        assert!(!menu.clipboard_has_text, "the default fake holds no text");

        // a FAILED clipboard read disables Paste, never panics
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard {
            fail_get: true,
            text: Some("unreachable".to_string()),
            ..Default::default()
        }));
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let menu = app.editor_menu.as_ref().expect("menu open");
        assert!(!menu.clipboard_has_text);
    }

    #[test]
    fn editor_menu_dismisses_on_any_key_or_backdrop() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        assert!(app.editor_menu.is_some());
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.editor_menu.is_none(), "any key dismisses");
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let _ = update(&mut app, Message::DismissEditorMenu);
        assert!(app.editor_menu.is_none(), "backdrop click dismisses");
    }

    #[test]
    fn editor_menu_actions_run() {
        // Copy: selection reaches the system clipboard, menu closes
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        select_hello(&mut app);
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let _ = update(&mut app, Message::EditorMenuCopy);
        assert!(app.editor_menu.is_none());
        assert_eq!(app.clipboard_text().as_deref(), Some("hello"));

        // Cut
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        select_hello(&mut app);
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let _ = update(&mut app, Message::EditorMenuCut);
        assert_eq!(app.editor().buf.line(0), " world");
        assert_eq!(app.clipboard_text().as_deref(), Some("hello"));

        // Paste
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::with_text("P")));
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        let _ = update(&mut app, Message::EditorMenuPaste);
        assert_eq!(app.editor().buf.line(0), "Phello world");

    }

    #[test]
    fn dragging_then_menu_items_use_the_shared_paths() {
        // a mouse-dragged selection feeds Copy through the same code as
        // shift+arrows: drag, then menu Copy
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 0 })));
        let to = window_px(Pos { row: 0, col: 4 });
        let _ = update(&mut app, Message::MouseMovedWindow(to.x, to.y));
        let _ = update(&mut app, Message::MouseReleased);
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        assert!(app.editor_menu.as_ref().unwrap().has_selection);
        let _ = update(&mut app, Message::EditorMenuCopy);
        assert_eq!(app.clipboard_text().as_deref(), Some("hello"));
        // pasting a DIFFERENT text replaces the drag's selection (the
        // first inserted op consumes it)
        app.clipboard = Some(Box::new(FakeClipboard::with_text("XY")));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('v'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.editor().buf.line(0), "XY world");
    }

    #[test]
    fn selecting_with_the_mouse_then_typing_replaces_it() {
        let mut app = regular_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, Message::EditorPressed(local_px(Pos { row: 0, col: 0 })));
        let to = window_px(Pos { row: 0, col: 4 });
        let _ = update(&mut app, Message::MouseMovedWindow(to.x, to.y));
        let _ = update(&mut app, Message::MouseReleased);
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Char('X'), KeyModifiers::NONE));
        assert_eq!(app.editor().buf.line(0), "X world", "typing replaced the selection");
    }

    #[test]
    fn tab_switch_closes_the_editor_menu() {
        let mut app = two_tab_app();
        app.clipboard = Some(Box::new(FakeClipboard::default()));
        let _ = update(&mut app, Message::EditorMenu(Point::new(30.0, 120.0)));
        assert!(app.editor_menu.is_some());
        let _ = update(&mut app, Message::TabPressed(1));
        assert!(app.editor_menu.is_none());
    }

    // a local twin of tab_tests::two_tab_app keeps the modules independent
    fn two_tab_app() -> App {
        let mut app = regular_app();
        app.tabs.push(Tab::with_source("second".into()));
        app
    }

    #[test]
    fn the_drag_anchor_clamps_into_the_buffer() {
        // px_to_pos clamps: a press far left of the text anchors at
        // column 0 of the pressed row
        let mut app = regular_app();
        let _ = update(
            &mut app,
            Message::EditorPressed(Point::new(2.0, local_px(Pos { row: 1, col: 0 }).y)),
        );
        assert_eq!(app.editor().buf.pos(), Pos { row: 1, col: 0 });
        let _ = update(&mut app, Message::MouseReleased);
    }
}

#[cfg(test)]
mod undo_redo_tests {
    use super::*;

    fn key(mods: KeyModifiers, c: char) -> Message {
        Message::KeyPressed(KeyCode::Char(c), mods)
    }

    fn type_in_insert(app: &mut App, text: &str) {
        let _ = update(
            app,
            Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        for c in text.chars() {
            let _ = update(
                app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
    }

    fn vim_app(source: &str) -> App {
        let mut app = App::default();
        app.editor_mut().set_text(source);
        app.cur_mut().set_source(source);
        app
    }

    fn regular(source: &str) -> App {
        let mut app = App {
            config: crate::config::Config {
                vim: false,
                ..Default::default()
            },
            ..App::default()
        };
        app.editor_mut().set_text(source);
        app.cur_mut().set_source(source);
        app
    }

    #[test]
    fn insert_mode_ctrl_z_ctrl_y_roundtrip() {
        let mut app = vim_app("");
        type_in_insert(&mut app, "abc");
        assert_eq!(app.source(), "abc");
        assert_eq!(app.editor().buf.cursor(), (0, 3));
        assert!(app.editor().buf.can_undo());

        // Ctrl+Z steps back one edit at a time, cursor on the edit point
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "ab");
        assert_eq!(app.editor().buf.cursor(), (0, 2));
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "a");
        // Ctrl+Y redoes, Ctrl+Shift+Z is the same redo alias
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'y'));
        assert_eq!(app.source(), "ab");
        assert_eq!(app.editor().buf.cursor(), (0, 2));
        let _ = update(
            &mut app,
            key(KeyModifiers::CONTROL.union(KeyModifiers::SHIFT), 'z'),
        );
        assert_eq!(app.source(), "abc");
        // the session stayed in insert mode throughout
        assert_eq!(app.editor().mode, editor::Mode::Insert);
    }

    #[test]
    fn regular_mode_ctrl_z_ctrl_y_roundtrip() {
        let mut app = regular("");
        let k = |c: char| Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE);
        for c in "hello".chars() {
            let _ = update(&mut app, k(c));
        }
        assert_eq!(app.source(), "hello");
        // bare z/y still type (the chords are gated on CONTROL)
        let _ = update(&mut app, k('z'));
        let _ = update(&mut app, k('y'));
        assert_eq!(app.source(), "hellozy");

        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "helloz", "one undo step");
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "hello");
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'y'));
        assert_eq!(app.source(), "helloz");
        let _ = update(
            &mut app,
            key(KeyModifiers::CONTROL.union(KeyModifiers::SHIFT), 'z'),
        );
        assert_eq!(app.source(), "hellozy");
    }

    #[test]
    fn toolbar_messages_undo_and_redo_in_vim_normal_mode() {
        let mut app = vim_app("");
        type_in_insert(&mut app, "hello");
        let esc = Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE);
        let _ = update(&mut app, esc);
        assert_eq!(app.editor().mode, editor::Mode::Normal);

        // the button path (Message::Undo/Redo) shares the vim u / Ctrl+r
        // stack and works in normal mode with no mode switch
        let _ = update(&mut app, Message::Undo);
        assert_eq!(app.source(), "hell");
        assert_eq!(app.editor().mode, editor::Mode::Normal);
        let _ = update(&mut app, Message::Redo);
        assert_eq!(app.source(), "hello");
        assert_eq!(
            app.editor().buf.cursor(),
            (0, 4),
            "cursor on the edit point"
        );

        // vim's own keys still walk the same stack
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        assert_eq!(app.source(), "hell");
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.source(), "hello");
    }

    #[test]
    fn ctrl_z_is_not_mapped_in_vim_normal_mode() {
        // vim purists use u; Ctrl+Z stays inert there (only the toolbar
        // buttons undo in normal mode)
        let mut app = vim_app("");
        type_in_insert(&mut app, "hi");
        let esc = Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE);
        let _ = update(&mut app, esc);
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "hi", "Ctrl+Z does nothing in vim normal");
        assert!(app.editor().buf.can_undo(), "the stack was not touched");
    }

    #[test]
    fn can_undo_and_can_redo_track_the_stacks() {
        let mut app = vim_app("");
        assert!(!app.editor().buf.can_undo());
        assert!(!app.editor().buf.can_redo());

        type_in_insert(&mut app, "ab");
        assert!(app.editor().buf.can_undo());
        assert!(!app.editor().buf.can_redo(), "a fresh edit clears redo");

        let _ = update(&mut app, Message::Undo);
        assert!(app.editor().buf.can_undo());
        assert!(app.editor().buf.can_redo());

        let _ = update(&mut app, Message::Redo);
        assert!(app.editor().buf.can_undo());
        assert!(!app.editor().buf.can_redo(), "redo stack drained");

        // undoing all the way back empties the undo stack again
        let _ = update(&mut app, Message::Undo);
        let _ = update(&mut app, Message::Undo);
        let _ = update(&mut app, Message::Undo); // beyond the history: no-op
        assert!(!app.editor().buf.can_undo());
        assert_eq!(app.source(), "", "over-undoing leaves the start text");
    }

    #[test]
    fn undo_dismisses_the_completion_popup() {
        let mut app = App::default(); // SAMPLE_DBML: completion has items
        let _ = update(&mut app, Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Space, KeyModifiers::CONTROL),
        );
        assert!(app.completion.is_some(), "popup open");
        // Ctrl+Z lands even with the popup up (the chord arm precedes the
        // popup's typing interception) and closes it
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert!(app.completion.is_none());
    }

    #[test]
    fn ctrl_z_right_after_the_ctrl_d_chord_is_not_rollout() {
        // the chord-rollout guard swallows a BARE d/u after ctrl+d/u; the
        // ctrl-modified undo chord must pass through
        let mut app = vim_app("");
        app.window_size = (1400.0, 900.0);
        type_in_insert(&mut app, "ab");
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let _ = update(&mut app, key(KeyModifiers::CONTROL, 'z'));
        assert_eq!(app.source(), "a", "Ctrl+Z survived the rollout window");
    }
}
