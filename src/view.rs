//! Window layout: tab strip on top, then the editor and diagram panes
//! flush against each other (their shared border is the pane seam, with
//! the invisible resize strip stacked over it) and flush with the
//! window's bottom edge; the toast pill, help overlay, the record view
//! and the tab context menu stack above all.

use crate::app::{
    App, Drag, EditorMenu, FileLiveness, Maximized, Message, PickerKind, RecordView,
    TableHeaderMenu, TabMenu, STRIP_LEFT, TAB_SLOT_W,
};
use crate::canvas::diagram::{Diagram, DiagramOverlay};
use crate::canvas::editor::{EditorOverlay, EditorView};
use dbml_ui::dbml::ast::RecordValue;
use crate::editor::{KeyCode, KeyModifiers};
use crate::theme;
use iced::widget::{
    button, canvas, column, container, horizontal_space, mouse_area, row, scrollable, stack, text,
};
use iced::{Color, Element, Fill, Font, Length, Point, Rectangle, Size};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

/// Vertical chrome around the editor canvas: every pixel of the root
/// layout above and below it, which `App::page_lines` must subtract from
/// the window height to get the true visible-line count.
///
/// Term by term (iced line height is 1.3x the text size):
/// above the canvas — 30.0 tab strip canvas (fixed height) + 1.0 pane
/// padding + 20.3 editor toolbar (text 11 + button padding [3,8]) + 4.0
/// editor spacing = 55.3; below the canvas — 1.0 pane padding, and
/// nothing else (the content is flush with the window's bottom edge).
///
/// Keep this in sync with `root`; if the layout chrome changes,
/// re-measure and update both.
pub const EDITOR_CHROME: f32 = 56.3;

/// Window-y of the editor canvas's top edge: every pixel of chrome above
/// it (tab strip 30 + pane padding 1 + toolbar 20.3 + spacing 4 = 55.3).
/// The Regular-mode selection drag converts window pointer px to the
/// canvas-local coords px_to_pos expects with this constant, so it must
/// move in lockstep with `EDITOR_CHROME` (which is this plus the 1.0
/// below-canvas pane padding) and `root`.
pub const EDITOR_CANVAS_TOP: f32 = 55.3;

/// Window-y of the diagram canvas's top edge: tab strip 30 + the pane's
/// 1px padding. Only true while a document is open (the start screen has
/// no toolbar row above the panes); the start screen's empty diagram
/// never needs fit math. Keep in sync with `root`.
pub const DIAGRAM_CANVAS_TOP: f32 = 31.0;

/// Every pixel of chrome around the diagram canvas: DIAGRAM_CANVAS_TOP
/// above it and the pane's bottom padding 1 below it. The action buttons
/// float OVER the canvas (no layout of their own), so this is all of it.
/// The canvas height is `window height - DIAGRAM_CHROME`. Keep in sync
/// with `root`.
pub const DIAGRAM_CHROME: f32 = DIAGRAM_CANVAS_TOP + 1.0;

pub fn root(app: &App) -> Element<'_, Message> {
    let editor_body: Element<'_, Message> = if app.doc_open() {
        column![
            editor_toolbar(app),
            // the overlay (tooltips, completion) is a separate canvas
            // STACKED above: iced flattens one canvas widget's geometries
            // into a mesh pass + a text pass, so a second-geometry tooltip
            // background painted behind the editor text (transparent look)
            // — the find bar and the vim `/` prompt float above both as
            // real widgets (opaque: canvas geometry always sits UNDER the
            // text pass, so the old canvas-drawn bar bled buffer glyphs)
            {
                let mut layers: Vec<Element<'_, Message>> = vec![
                    canvas(EditorView { app }).width(Fill).height(Fill).into(),
                    canvas(EditorOverlay { app })
                        .width(Fill)
                        .height(Fill)
                        .into(),
                ];
                if let Some(bar) = find_bar_overlay(app) {
                    layers.push(
                        container(bar)
                            .width(Fill)
                            .height(Fill)
                            .align_x(iced::alignment::Horizontal::Right)
                            .align_y(iced::alignment::Vertical::Top)
                            .padding(10)
                            .into(),
                    );
                }
                if let Some(prompt) = vim_search_overlay(app) {
                    layers.push(
                        container(prompt)
                            .width(Fill)
                            .height(Fill)
                            .align_x(iced::alignment::Horizontal::Left)
                            .align_y(iced::alignment::Vertical::Bottom)
                            .padding(10)
                            .into(),
                    );
                }
                let layered: Element<'_, Message> = container(
                    iced::widget::Stack::with_children(layers)
                        .width(Fill)
                        .height(Fill),
                )
                .width(Fill)
                .height(Fill)
                .into();
                layered
            }
        ]
        .spacing(4)
        .height(Fill)
        .into()
    } else {
        canvas(EditorView { app }).width(Fill).height(Fill).into()
    };
    let editor_pane = container(editor_body)
        .padding(1.0)
        .width(Fill)
        .height(Fill)
        .style(move |_t| seam_panel_style(Seam::Right));

    // dbdiagram-style problems UI: a floating count badge in the pane's
    // bottom-right corner, popping out the diagnostics list on click
    let editor_pane: Element<'_, Message> = if app.doc_open()
        && (!app.all_diagnostics().is_empty() || app.diagnostics_open)
    {
            let pane: Element<'_, Message> = editor_pane.into();
            stack![pane, diagnostics_overlay(app)].into()
        } else {
            editor_pane.into()
        };
    // a file problem gets its own actionable pill, bottom-LEFT (the
    // diagnostics badge owns the right corner)
    let editor_pane: Element<'_, Message> =
        if app.doc_open() && app.cur().liveness != FileLiveness::InSync {
            stack![editor_pane, file_status_overlay(app)].into()
        } else {
            editor_pane
        };

    // the marker/note tooltip is its own canvas STACKED above: iced
    // flattens one canvas widget's geometries into a mesh pass + a text
    // pass, so a second-geometry tooltip pill painted behind the box row
    // text (see DiagramOverlay)
    let diagram_canvas = stack![
        canvas(Diagram {
            layout: app.diagram(),
            zoom: app.cur().zoom,
            pan: app.cur().pan,
            hover: if app.drag == Drag::None
                && !app.modal_open()
                && !app.pointer_in_export_dropdown()
            {
                app.canvas_cursor
            } else {
                None
            },
            // only DIAGRAM drags grab: this flag fed the fist cursor
            // window-wide (canvas mouse_interaction is not bounds-gated),
            // so an editor text/scrollbar drag showed it over the editor
            dragging: matches!(
                app.drag,
                Drag::Pan { .. } | Drag::Table { .. } | Drag::Group { .. } | Drag::Sticky { .. }
            ),
            // raised items (recently grabbed) paint last: the overlap
            // priority persists after the drop, tables over stickies
            // and stickies over tables alike
            item_z: &app.cur().item_z,
            // a dragged TableGroup raises all its member tables
            drag_group: match &app.drag {
                Drag::Group { name, .. } => Some(name.clone()),
                _ => None,
            },
            // the sticky being dragged/resized paints after every other
            // note: an item dragged over another rides on top
            drag_note: match &app.drag {
                Drag::Sticky { name, .. } | Drag::StickyResize { name, .. } => {
                    Some(name.clone())
                }
                _ => None,
            },
            // while a connected table is dragged, ITS relationship lines
            // draw above the tables they cross (back beneath on release)
            drag_edges: match &app.drag {
                Drag::Table { title, .. } => app
                    .cur()
                    .diagram
                    .boxes
                    .iter()
                    .find(|b| &b.title == title)
                    .map(|b| app.cur().diagram.edges_touching_table(b.node)),
                _ => None,
            },
            // the selected sticky note draws its border (its text is
            // edited in the source editor, never on the canvas)
            selected_note: app.selected_note.clone(),
            edge_anim: &app.cur().edge_anim,
            clock: app.anim_clock,
        })
        .width(Fill)
        .height(Fill),
        canvas(DiagramOverlay {
            layout: app.diagram(),
            zoom: app.cur().zoom,
            pan: app.cur().pan,
            // the popups draw in this stacked widget (see DiagramOverlay's
            // draw note): the same gated hover the chrome uses
            hover: if app.drag == Drag::None
                && !app.modal_open()
                && !app.pointer_in_export_dropdown()
            {
                app.canvas_cursor
            } else {
                None
            },
        })
        .width(Fill)
        .height(Fill),
        // the maximize button rides the top-right corner (F2 does the
        // same) — the toolbar's own icon chrome, flush with the corner
        container(
            diagram_max_button(app.maximized == Maximized::Diagram),
        )
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top),

        // the right-click twin (the header-color menu's entry point): the
        // Diagram canvas itself only forwards left presses
        canvas(DiagramRightClick).width(Fill).height(Fill),

    ]
    .width(Fill)
    .height(Fill);
    // dbdiagram-style diagram actions FLOAT over the canvas — auto-arrange
    // the tables left-to-right along their refs, and zoom so everything
    // fits — no opaque bar behind them, centered on the pane's bottom
    // edge. (A plain container overlay doesn't intercept canvas events;
    // only the buttons themselves do.) The exports consolidate into one
    // dropdown at the TOP of the pane, centered.
    let export_ink = format!(
        "#{:02x}{:02x}{:02x}",
        (theme::TEXT.r * 255.0) as u32,
        (theme::TEXT.g * 255.0) as u32,
        (theme::TEXT.b * 255.0) as u32
    );
    let export_svg = iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{}' width='16' height='16'><path d='{}' fill='{export_ink}'/></svg>",
            ICON_EXPORT.1,
            ICON_EXPORT.0,
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0));
    let mut export_dialect_layer: Option<Element<'_, Message>> = None;
    let mut export_dropdown = column![
        button(
            container(
                row![
                    export_svg,
                    text("Export".to_string()),
                    // the caret stays DOWN in both states (an up caret
                    // read as a strange sideways flick when opening)
                    chevron_svg(true),
                ]
                .spacing(6)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center),
        )
        // FIXED 112x30 — the same slot the menu card occupies below, so
        // the trigger, the card and the dialect card are one aligned
        // column (the natural-height trigger made the card read as
        // left-shifted under it)
        .width(Length::Fixed(112.0))
        .height(Length::Fixed(30.0))
        .style(move |_t, s| {
            let mut st = tool_button_style();
            st.background = Some(iced::Background::Color(
                // OPEN wins over HOVER: the pointer rests on the trigger
                // when the menu opens, and the button must read as held
                // down (near-black) the whole time, not as hovered. At
                // rest: the same surface as every toolbar button. HOVER
                // is a whisper — one opaque step up from the rest state,
                // not the toolbar's gray lift and not a translucent wash
                // (this button sits ON the diagram: a translucent layer
                // composites with the canvas colors behind it and shifts
                // hue with every group/table it crosses).
                if app.export_menu_open {
                    theme::BG
                } else if s == iced::widget::button::Status::Hovered {
                    Color::from_rgb8(0x26, 0x2b, 0x31)
                } else {
                    theme::PANEL
                },
            ));
            st
        })
        .on_press(Message::ToggleExportMenu),
    ]
    .spacing(4)
    .align_x(iced::alignment::Horizontal::Center);
    if app.export_menu_open {
        // the menu drops under the button, centered over the canvas: the
        // image exports, then the SQL row whose dialect list cascades to
        // the RIGHT on hover (a click toggles it too — hover delivery can
        // be starved). Leaving the whole dropdown folds the list back up.
        let menu_style = |_t: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(theme::PANEL)),
            border: iced::border::rounded(6).color(theme::BORDER),
            shadow: iced::Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 10.0,
            },
            ..Default::default()
        };
        // the image rows fold the SQL list back up on hover (a different
        // item hovered = the submenu's gone, dbdiagram-style)
        let image_item = |label: &'static str| -> Element<'static, Message> {
            let msg = if label == "Export SVG" {
                Message::ExportAs(crate::app::ExportKind::Svg)
            } else {
                Message::ExportAs(crate::app::ExportKind::Png)
            };
            mouse_area(export_menu_item(label, msg))
                .on_enter(Message::ExportSqlMenuOpen(false))
                .into()
        };
        // the SQL row STAYS dark while its dialect list is open — the
        // open state is visible on the row that owns it
        let sql_row = mouse_area(
            button(
                container(
                    row![
                        text("Export SQL".to_string())
                            .size(12)
                            .color(theme::TEXT),
                        chevron_svg_colored(false, DIAG_GRAY_200, 10.0),
                    ]
                    .spacing(6)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Fill)
            .height(Length::Fixed(28.0))
            .padding([0, 10])
            .style(move |_t, s| iced::widget::button::Style {
                background: Some(iced::Background::Color(match s {
                    iced::widget::button::Status::Hovered => theme::TOOLTIP,
                    // the open state is visible on the row that owns the
                    // dialect list
                    _ if app.export_menu_sql_open => theme::TOOLTIP,
                    _ => Color::TRANSPARENT,
                })),
                text_color: theme::TEXT,
                border: Default::default(),
                ..Default::default()
            })
            .on_press(Message::ToggleExportSqlMenu),
        )
        .on_enter(Message::ExportSqlMenuOpen(true));
        // the main card: three FIXED-height rows (28px each) so the
        // dialect list's alignment offset is exact, not a magic guess.
        // NO outer mouse_area re-sync: its on_enter published a
        // BUILD-TIME snapshot of export_menu_sql_open — a stale false
        // that stomped the SQL row's enter(true) the moment the pointer
        // reached the row. The rows carry their own enter state.
        let main_menu = container(
            column![
                image_item("Export SVG"),
                image_item("Export PNG"),
                sql_row,
            ],
        )
        .width(Length::Fixed(112.0))
        .style(menu_style);
        // (no outer mouse_area re-sync: its on_enter published a
        // build-time snapshot of export_menu_sql_open — a stale false
        // that stomped the SQL row's enter(true). The rows own the state.)
        export_dropdown = export_dropdown.push(main_menu);
        if app.export_menu_sql_open {
            // the dialect card is a SEPARATE floating card beside the
            // main one — a nested sibling inside the dropdown stretched
            // the whole card and double-bordered it, and a row sibling
            // overflow is impossible: iced flex passes the remaining
            // space (0) as the next child's max limit, clamping its
            // Fixed width to nothing. It rides the pane's layer stack
            // instead, anchored beside the SQL row: card border 1 + the
            // two fixed-height image rows (2 x 30) + the dropdown top
            // pad 8 puts the SQL row at pane-y 99; the card's own border
            // aligns a pixel above.
            let dialects = [
                ("PostgreSQL", dbml_ui::dbml::sql::Dialect::Postgres),
                ("MySQL", dbml_ui::dbml::sql::Dialect::MySQL),
                ("SQL Server", dbml_ui::dbml::sql::Dialect::MSSQL),
                ("Oracle", dbml_ui::dbml::sql::Dialect::Oracle),
                ("SQLite", dbml_ui::dbml::sql::Dialect::SQLite),
            ];
            let mut sub = column![].spacing(0);
            for (label, dialect) in dialects {
                sub = sub.push(export_menu_item(label, Message::ExportSql(dialect)));
            }
            // pane geometry, EXACT: dropdown top pad 8 + trigger 30 +
            // spacing 4 = the main card's top at 42; + its 1px border +
            // two 28px rows = the SQL row's top at 99 pane-relative. The
            // dialect card's border aligns the row's top edge (98). The
            // main card spans [pane_w/2 - 56, pane_w/2 + 56] (112 wide,
            // centered on the pane) — the dialect card's LEFT edge pins
            // flush to the main card's RIGHT (pane_w/2 + 56): the zones
            // are adjacent, so the pointer crosses from the SQL row into
            // the submenu without a gap to lose.
            let pane_w = (app.window_size.0 - app.seam_x()).max(220.0);
            let card = container(sub)
                .width(Length::Fixed(118.0))
                .style(menu_style);
            // the mouse_area bounds = THE CARD only (the padding lives
            // OUTSIDE it): its enter fires exactly when the pointer
            // crosses the flush edge from the SQL row. A full-pane
            // mouse_area here entered at spawn and never re-fired at the
            // boundary — the main card's exit closed the menu mid-cross.
            // No hover-out close anywhere: the submenu folds via the
            // SVG/PNG rows, a pick, or the backdrop click.
            export_dialect_layer = Some(
                container(
                    mouse_area(card).on_enter(Message::ExportSqlMenuOpen(true)),
                )
                .width(Fill)
                .height(Fill)
                .align_y(iced::alignment::Vertical::Top)
                .align_x(iced::alignment::Horizontal::Left)
                .padding(iced::Padding {
                    top: 98.0,
                    left: (pane_w / 2.0 + 56.0).max(0.0),
                    ..Default::default()
                })
                .into(),
            );
        }
    }
    let diagram_buttons = container(
        row![
            diagram_button("Add Sticky Note", Message::AddStickyNote, false),
            diagram_button(
                "Highlight Relationships",
                Message::ToggleEdgeHighlight,
                app.cur().highlight_all,
            ),
            diagram_button("Auto-arrange", Message::AutoArrange, false),
            diagram_button("Zoom to Fit", Message::ZoomToFit, false),
        ]
        .spacing(8),
    )
    .width(Fill)
    .height(Fill)
    .align_x(iced::alignment::Horizontal::Center)
    .align_y(iced::alignment::Vertical::Bottom)
    .padding(iced::Padding { bottom: 8.0, ..Default::default() });
    // while the export menu is open, a full-pane backdrop sits between the
    // canvas and the dropdown: clicking anywhere outside closes the menu
    // (the canvas click is consumed — standard menu behavior)
    let export_backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::ToggleExportMenu);
    let mut diagram_layers: Vec<Element<'_, Message>> = vec![diagram_canvas.into()];
    if app.export_menu_open {
        diagram_layers.push(export_backdrop.into());
    }
    diagram_layers.push(
        // the export dropdown rides the top edge, centered
        container(export_dropdown)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Top)
            .padding(iced::Padding { top: 8.0, ..Default::default() })
            .into(),
    );
    if let Some(layer) = export_dialect_layer {
        diagram_layers.push(layer);
    }
    diagram_layers.push(diagram_buttons.into());
    let diagram_pane = container(iced::widget::Stack::with_children(diagram_layers))
    .padding(1.0)
    .width(Fill)
    .height(Fill)
    .style(move |_t| {
        // the seam only exists once a document is open (the start screen
        // keeps its rounded gap to the diagram)
        let seam = if app.doc_open() { Seam::Left } else { Seam::None };
        seam_panel_style(seam)
    });

    let editor = container(editor_pane)
        .width(Length::FillPortion((app.split * 1000.0) as u16))
        .height(Fill);
    let diagram = container(diagram_pane)
        .width(Length::FillPortion(((1.0 - app.split) * 1000.0) as u16))
        .height(Fill);

    let panes: Element<'_, Message> = if !app.doc_open() {
        // start screen: one centered welcome view — no split, no diagram
        container(start_screen(app))
            .width(Fill)
            .height(Fill)
            .into()
    } else if app.maximized == Maximized::Editor {
        // editor maximized: it takes the diagram's space (F1 / the
        // toolbar button); the diagram pane is not laid out at all
        row![editor].height(Fill).into()
    } else if app.maximized == Maximized::Diagram {
        // diagram maximized (F2 / the floating button): diagram only
        row![diagram].height(Fill).into()
    } else {
        // flush panes: the seam is the two pane borders back to back —
        // one crisp vertical edge, no gap. The invisible resize strip is
        // stacked over it below.
        row![editor, diagram].height(Fill).into()
    };

    // panes (the start screen's is the centered welcome view, the split's
    // is editor + diagram)
    let base = column![
        canvas(TabStrip { app })
            .height(Length::Fixed(STRIP_H))
            .width(Fill),
        // panes sit flush under the tab strip (no gap bar) and flush with
        // the window's left/right/bottom edges
        column![panes]
    ];

    // the pane seam's resize strip, only once a document is open — and
    // not while a pane is maximized: the seam is off screen and the
    // maximized pane has its own restore
    // affordances (F1/F2 and the pane's own button)
    let base: Element<'_, Message> = if app.doc_open() && app.maximized == Maximized::None {
        stack![base, edge_overlay(app)].into()
    } else {
        base.into()
    };
    // the toast pill floats over the panes' bottom-right corner
    let base: Element<'_, Message> = if app.message.is_some() {
        stack![base, toast_overlay(app)].into()
    } else {
        base
    };
    // the toolbar button's hover tooltip floats above everything (it
    // anchors to the pointer, so it must escape the toolbar's own stack)
    let base: Element<'_, Message> = if let Some(tip) = toolbar_tip_overlay(app) {
        stack![base, tip].into()
    } else {
        base
    };
    // the editor-mode select menu floats over the toolbar
    let base: Element<'_, Message> = if let Some(at) = &app.vim_menu {
        stack![base, vim_menu_overlay(app, at)].into()
    } else {
        base
    };
    let base: Element<'_, Message> = if let Some(menu) = &app.tab_menu {
        stack![base, tab_menu_overlay(app, menu)].into()
    } else {
        base
    };
    let base: Element<'_, Message> = if let Some(menu) = &app.editor_menu {
        stack![base, editor_menu_overlay(app, menu)].into()
    } else {
        base
    };
    // the header-color menu shares the editor menu's stacking level (the
    // two never coexist: one is over the editor pane, one over the diagram)
    let base: Element<'_, Message> = if let Some(menu) = &app.table_header_menu {
        stack![base, table_header_menu_overlay(app, menu)].into()
    } else {
        base
    };
    // the record view is the topmost modal (help cannot be open beneath it:
    // the record view swallows keys first)
    let base: Element<'_, Message> = if let Some(rv) = &app.record_view {
        stack![base, record_view_overlay(app, rv)].into()
    } else {
        base
    };
    // the exit confirmation sits above everything
    let base: Element<'_, Message> = if app.exit_confirm {
        stack![base, exit_confirm_overlay()].into()
    } else {
        base
    };
    // ...and the tab-close confirmation above that (the two never stack
    // in practice, but the tab one wins if they did)
    let base: Element<'_, Message> = if let Some(idx) = app.tab_close_confirm {
        stack![base, tab_close_overlay(app, idx)].into()
    } else {
        base
    };
    base
}

/// The record view: a centered modal over everything (topmost in `root`),
/// showing a table's `records` sample rows as a scrollable grid. Escape,
/// the × button, or a backdrop click closes it.
fn record_view_overlay(app: &App, rv: &RecordView) -> Element<'static, Message> {
    // their RecordPreviewModal: w-4/5 h-4/5 (max-h 800), p-8, bg #222
    let w = (app.window_size.0 * 0.8).clamp(340.0, 896.0);
    let h = (app.window_size.1 * 0.8).clamp(240.0, 800.0);
    let body = match app.diagram().boxes.iter().find(|b| b.node == rv.node) {
        Some(b) if b.records.is_some() => record_panel(b, &app.diagram().boxes, rv),
        _ => container(
            text("No records for this table".to_string())
                .size(13.0)
                .color(theme::DIM),
        )
        .padding(24)
        .into(),
    };
    let panel = container(
        stack![
            container(body).padding([28, 32]).width(Fill).height(Fill),
            // their close × floats absolute top-2 right-2, over the p-8
            // padding zone
            container(close_records_button())
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top)
                .padding([6, 10]),
        ]
        .width(Fill)
        .height(Fill),
    )
    .width(Length::Fixed(w))
    .height(Length::Fixed(h))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(REC_MODAL_BG)),
        border: iced::border::rounded(12).color(REC_DIVIDER),
        shadow: modal_shadow(),
        ..Default::default()
    });
    // the diagram dims behind their modal; click anywhere outside
    // dismisses it
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
            ..Default::default()
        })
        .on_press(Message::CloseRecords);
    // stack children land at (0,0): centering the panel is the full-size
    // container's job (its content alignment)
    stack![backdrop, container(panel).center(Fill)].into()
}

fn close_records_button() -> Element<'static, Message> {
    button(
        text("\u{00d7}".to_string())
            .size(22.0)
            .color(theme::TEXT),
    )
    .padding([4, 11])
    .style(move |_t, s| iced::widget::button::Style {
        background: Some(iced::Background::Color(match s {
            iced::widget::button::Status::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            _ => Color::TRANSPARENT,
        })),
        border: iced::border::rounded(6),
        ..Default::default()
    })
    .on_press(Message::CloseRecords)
    .into()
}

/// The modal drop shadow: without it the panels read as painted flat
/// onto the app (iced blurs this properly, unlike the canvas fakes).
fn modal_shadow() -> iced::Shadow {
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
        offset: iced::Vector::new(0.0, 12.0),
        blur_radius: 36.0,
    }
}

/// The exit confirmation: "unsaved changes will be lost" with Save all &
/// exit (primary), Exit without saving, and Cancel. The GPU-loss restart
/// never shows this — it panics past the whole UI and self-heals.
fn exit_confirm_overlay() -> Element<'static, Message> {
    confirm_modal(
        "Unsaved changes",
        "You have unsaved changes. Are you sure you want to exit?",
        &[
            ("Exit", Message::ExitDiscard, true),
            ("Cancel", Message::ExitCancel, false),
        ],
        Message::ExitCancel,
    )
}

/// The two unsaved-changes dialogs (exit + tab close) share the record
/// preview modal's surface family: a dim backdrop over the app, the #222
/// panel with a #37383e hairline, a blue primary and two filled dark
/// secondaries (the old transparent ghosts read as bare text against the
/// old #1c2128 panel — same bg, no visible body or outline).
fn confirm_modal(
    title: &'static str,
    body: &'static str,
    buttons: &[(&'static str, Message, bool)],
    cancel: Message,
) -> Element<'static, Message> {
    let mut actions = row![].spacing(10);
    for (label, msg, primary) in buttons.iter().cloned() {
        actions = actions.push(
            button(text(label.to_string()).size(13.0))
                .padding([8, 16])
                .style(move |_t, s| {
                    let (bg, ink, border) = if primary {
                        // the blue primary: darker on hover, white ink
                        match s {
                            iced::widget::button::Status::Hovered => (
                                iced::Background::Color(Color::from_rgb(0.231, 0.486, 0.878)),
                                iced::Color::WHITE,
                                Color::from_rgb(0.231, 0.486, 0.878),
                            ),
                            _ => (
                                iced::Background::Color(theme::FK),
                                iced::Color::WHITE,
                                theme::FK,
                            ),
                        }
                    } else {
                        // the filled dark secondaries (#2C2D32 body, the
                        // record band), brightening on hover
                        match s {
                            iced::widget::button::Status::Hovered => (
                                iced::Background::Color(REC_DIVIDER),
                                theme::TEXT,
                                REC_DIVIDER,
                            ),
                            _ => (
                                iced::Background::Color(REC_HEADER_BAND),
                                theme::TEXT,
                                REC_DIVIDER,
                            ),
                        }
                    };
                    iced::widget::button::Style {
                        background: Some(bg),
                        text_color: ink,
                        border: iced::border::rounded(6).color(border),
                        ..Default::default()
                    }
                })
                .on_press(msg),
        );
    }
    let panel = container(
        column![
            text(title.to_string()).size(16.0).color(theme::TEXT),
            text(body.to_string()).size(13.0).color(theme::DIM),
            container(actions)
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(14),
    )
    .padding(22)
    .max_width(440)
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(REC_MODAL_BG)),
        border: iced::border::rounded(12).color(REC_DIVIDER),
        shadow: modal_shadow(),
        ..Default::default()
    });
    // the app dims behind the modal; click outside = cancel (the record
    // view's convention)
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
            ..Default::default()
        })
        .on_press(cancel);
    stack![backdrop, container(panel).center(Fill)].into()
}

/// The record view's content: header row (table pictogram tinted with the
/// table's header color, table name) over the scrollable grid of record
/// rows.
// The records modal's inks, dbdiagram dark (their RecordPreviewModal +
// DTable CSS, live-sampled): modal #222, the grid's header band #2C2D32,
// hairline dividers on their table-divider token #37383e, muted gray
// #9B9DA3, their pk and fk blues.
const REC_MUTED: Color = Color { r: 0.607, g: 0.616, b: 0.639, a: 1.0 }; // #9B9DA3
const REC_FK_BLUE: Color = Color { r: 0.467, g: 0.643, b: 0.996, a: 1.0 }; // #77A4FE
const REC_MODAL_BG: Color = Color { r: 0.133, g: 0.133, b: 0.133, a: 1.0 }; // #222222
const REC_HEADER_BAND: Color = Color { r: 0.173, g: 0.176, b: 0.196, a: 1.0 }; // #2C2D32
const REC_DIVIDER: Color = Color { r: 0.216, g: 0.220, b: 0.243, a: 1.0 }; // #37383e
/// The row dividers: their `.5px` border-bottom of the same token reads
/// about half as strong over the #222 body.
const REC_ROW_DIVIDER: Color = Color { r: 0.216, g: 0.220, b: 0.243, a: 0.55 }; // #37383e @ 55%
/// Their chip pill: `px-1 rounded font-semibold` on `bg-[#44444B]
/// text-[#9B9DA3]` (the `s` class).
const REC_CHIP_BG: Color = Color { r: 0.267, g: 0.267, b: 0.294, a: 1.0 }; // #44444B

/// A header cell: their `p-3` on the #2C2D32 band. The column divider
/// rides BETWEEN cells as its own 1px element (iced borders have no
/// per-side widths).
fn record_header_cell(head: Element<'static, Message>, w: f32) -> Element<'static, Message> {
    container(head)
        .width(Length::Fixed(w))
        .padding(12)
        .into()
}

/// A body cell: `px-3 py-2` over the modal bg.
fn record_body_cell(content: Element<'static, Message>, w: f32) -> Element<'static, Message> {
    container(content)
        .width(Length::Fixed(w))
        .padding([8, 12])
        .into()
}

/// Their `bordered` table's 1px #37383e vertical divider, drawn BETWEEN
/// cells (the wrapper's own border closes the outline).
fn record_col_divider() -> Element<'static, Message> {
    container(horizontal_space())
        .width(Length::Fixed(1.0))
        .height(Fill)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(REC_DIVIDER)),
            ..Default::default()
        })
        .into()
}

/// The faint horizontal divider between body rows: their `.5px`
/// border-bottom of the same token, half as strong over the #222 body.
fn record_row_divider() -> Element<'static, Message> {
    container(horizontal_space())
        .width(Fill)
        .height(Length::Fixed(1.0))
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(REC_ROW_DIVIDER)),
            ..Default::default()
        })
        .into()
}

/// The strong horizontal divider under the header band (their `th`
/// border-bottom).
fn record_header_divider() -> Element<'static, Message> {
    container(horizontal_space())
        .width(Fill)
        .height(Length::Fixed(1.0))
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(REC_DIVIDER)),
            ..Default::default()
        })
        .into()
}

/// Their chip pill (`E`, `not_null`, `unique`): semibold 12px on a
/// #44444B rounded-3 sliver.
fn record_chip(label: &str) -> Element<'static, Message> {
    container(
        text(label.to_string())
            .size(11.0)
            .color(REC_MUTED)
            .font(Font {
                weight: iced::font::Weight::Semibold,
                ..Font::DEFAULT
            }),
    )
    .padding([1, 4])
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(REC_CHIP_BG)),
        border: iced::border::rounded(3),
        ..Default::default()
    })
    .into()
}

/// The records-grid pk key glyph: the diagram's own pk icon path as an
/// inline SVG in the pk blue (an emoji key renders as a question mark
/// in the UI font).
fn pk_glyph() -> Element<'static, Message> {
    let d = dbml_ui::layout::row_icon_path(dbml_ui::layout::RowIcon::Pk);
    // light gray (their text-dbx-gray-100), not the blue accent
    let (r, g, b) = (0xd5, 0xda, 0xe1);
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12' width='12' height='12'><path d='{d}' fill='#{r:02x}{g:02x}{b:02x}'/></svg>"
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(12.0))
    .height(Length::Fixed(12.0))
    .into()
}

fn record_panel(
    b: &dbml_ui::layout::TableBox,
    boxes: &[dbml_ui::layout::TableBox],
    rv: &RecordView,
) -> Element<'static, Message> {
    let rec = b.records.as_ref().expect("records for the panel");

    // their header: h2 text-base font-bold — `schema.tableName` (no
    // glyph; the user preferred the bare name)
    let mut title_row = row![
        text(b.title.clone())
            .size(16.0)
            .color(theme::TEXT)
            .font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            }),
    ]
    .align_y(iced::alignment::Vertical::Center)
    .spacing(6);
    if rec.example {
        title_row = title_row.push(
            text("example".to_string()).size(10.0).color(REC_MUTED).width(Length::Shrink),
        );
    }
    let header = title_row;

    // display order: the record's value columns with PK columns sorted
    // first (their h computed: the # column, then visible-pk columns,
    // then the rest in definition order)
    let order = records_display_order(b, rec);

    // resolve every cell label first (their `_` value chain: the
    // record's value, else the column's `default`, else the ROW NUMBER
    // on an `[increment]` column, else (null)) so the column widths fit
    // the widest rendered part
    let mut labels: Vec<Vec<(String, bool)>> = Vec::new();
    for (ri, row_vals) in rec.rows.iter().enumerate() {
        let mut line = Vec::new();
        for &p in &order {
            let v = row_vals.get(p).unwrap_or(&RecordValue::Null);
            let (label, is_null) = match v {
                RecordValue::Null => {
                    let rowi = rec.columns.get(p).and_then(|&ci| b.rows.get(ci));
                    let defl = rowi
                        .and_then(|r| r.default.as_ref())
                        .map(|d| d.trim_matches('\'').trim_matches('"').to_string());
                    let incr = rowi.is_some_and(|r| r.increment);
                    match (defl, incr) {
                        (Some(d), _) => (d, false),
                        (None, true) => (format!("{}", ri + 1), false),
                        _ => ("(null)".to_string(), true),
                    }
                }
                other => (record_value_label(other), false),
            };
            line.push((label, is_null));
        }
        labels.push(line);
    }

    // one width per display position, fitting the header parts AND every
    // value in that column
    let widths: Vec<f32> = order
        .iter()
        .enumerate()
        .map(|(pi, &p)| {
            let rowi = rec.columns.get(p).and_then(|&ci| b.rows.get(ci));
            let name = rowi.map_or_else(|| "?".to_string(), |r| r.name.clone());
            let ty = rowi.map_or_else(String::new, |r| r.ty.clone());
            let mut parts: Vec<String> = vec![name];
            for line in &labels {
                parts.push(line[pi].0.clone());
            }
            let mut w = cell_w(&parts.iter().map(String::as_str).collect::<Vec<_>>());
            // the type line is a ROW of texts (type + chips + 4px gaps) —
            // its summed width must fit or the chips clip
            if let Some(rowi) = rowi {
                let mut chips: Vec<&str> = Vec::new();
                if rowi.ty_enum {
                    chips.push("Enum");
                }
                if rowi.not_null == Some(true) {
                    chips.push("Not Null");
                }
                if rowi.unique {
                    chips.push("unique");
                }
                let type_line_w = (ty.chars().count() + chips.iter().map(|c| c.chars().count()).sum::<usize>())
                    as f32
                    * 7.2
                    + chips.len() as f32 * 4.0;
                w = w.max(type_line_w + 16.0);
            }
            w
        })
        .collect();

    // the header: the `#` cell + one p-3 cell per column, all on the
    // #2C2D32 band (their `o` class) with rounded-top corners — the band
    // is the ROW's background so it clips cleanly against the wrapper
    let mut grid = column![].spacing(0);
    let cols_len = order.len();
    let mut header_row = row![].spacing(0);
    header_row = header_row.push(
        container(
            text(" # ".to_string())
                .size(14.0)
                .color(Color { a: 0.4, ..theme::TEXT })
                .font(Font {
                    weight: iced::font::Weight::Semibold,
                    ..Font::DEFAULT
                }),
        )
        .width(Length::Fixed(36.0))
        .height(Fill)
        .align_y(iced::alignment::Vertical::Center)
        .padding([12, 10]),
    );
    for (pi, (&p, &w)) in order.iter().zip(&widths).enumerate() {
        let Some(&ci) = rec.columns.get(p) else {
            continue;
        };
        let rowi = b.rows.get(ci);
        let name = rowi.map_or_else(|| "?".to_string(), |r| r.name.clone());
        let pk = rowi.is_some_and(|r| r.pk);
        let mut name_line = row![].spacing(4).align_y(iced::alignment::Vertical::Center);
        // their header pair: a blue key icon on the visible-pk columns
        // (the diagram's own pk glyph — the U+1F512 emoji renders as a
        // question mark in the UI font)
        if pk {
            name_line = name_line.push(pk_glyph());
        }
        name_line = name_line.push(
            text(name.clone())
                .size(14.0)
                .color(theme::TEXT)
                .font(Font {
                    weight: iced::font::Weight::Semibold,
                    ..Font::DEFAULT
                }),
        );
        // the type line: the muted type (at their opacity-70 over the
        // band) + their chips (`E`, `not_null`, `unique`) as pills
        let mut type_line = row![].spacing(6).align_y(iced::alignment::Vertical::Center);
        type_line = type_line.push(
            text(
                rowi.map_or_else(String::new, |r| r.ty.clone()),
            )
            .size(12.0)
            .color(Color { a: 0.7, ..REC_MUTED }),
        );
        if let Some(rowi) = rowi {
            if rowi.ty_enum {
                type_line = type_line.push(record_chip("Enum"));
            }
            if rowi.not_null == Some(true) {
                type_line = type_line.push(record_chip("Not Null"));
            }
            if rowi.unique {
                type_line = type_line.push(record_chip("unique"));
            }
        }
        let head = column![name_line, type_line].spacing(4);
        header_row = header_row.push(record_header_cell(head.into(), w));
        if pi + 1 < cols_len {
            header_row = header_row.push(record_col_divider());
        }
    }
    // the Fill-height dividers enclosed themselves into the row's size
    // hint; Shrink it back so the scrollable's content stays Shrink (the
    // dividers still stretch to the row's laid-out height)
    let header_row = header_row.height(Length::Shrink);
    // the #2C2D32 band: rounded TOP corners (the wrapper's border rounds
    // the rest away), the header-bottom divider rides the cells' borders
    let header_band = container(header_row).style(|_t| container::Style {
        background: Some(iced::Background::Color(REC_HEADER_BAND)),
        border: iced::border::Border {
            radius: iced::border::Radius {
                top_left: 8.0,
                top_right: 8.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            ..Default::default()
        },
        ..Default::default()
    });
    grid = grid.push(header_band);
    grid = grid.push(record_header_divider());

    for (ri, row_vals) in rec.rows.iter().enumerate() {
        let followed = rv.follow == Some(ri);
        let mut line = row![].spacing(0);
        line = line.push(
            container(
                text(format!("{}", ri + 1))
                    .size(12.0)
                    .color(Color { a: 0.4, ..theme::TEXT }),
            )
            .width(Length::Fixed(36.0))
            .padding([8, 10]),
        );
        for (pi, (&p, &w)) in order.iter().zip(&widths).enumerate() {
            let (label, is_null) = labels[ri][pi].clone();
            let v = row_vals.get(p).unwrap_or(&RecordValue::Null);
            line = line.push(record_cell(
                boxes, b, rec, p, v, w, label, is_null, REC_FK_BLUE,
            ));
            if pi + 1 < cols_len {
                line = line.push(record_col_divider());
            }
        }
        let line = line.height(Length::Shrink);
        let line = if followed {
            container(line).style(|_t| container::Style {
                background: Some(iced::Background::Color(theme::ROW_HOVER_BG)),
                ..Default::default()
            })
        } else {
            container(line)
        };
        grid = grid.push(line);
        // the faint row divider between body rows (the last row's bottom
        // edge is the wrapper's border)
        if ri + 1 < rec.rows.len() {
            grid = grid.push(record_row_divider());
        }
    }
    if rec.rows.is_empty() {
        grid = grid.push(
            container(text("no rows".to_string()).size(12.0).color(REC_MUTED))
                .padding([8, 42]),
        );
    }

    // the grid: their DTable `wrapper-rounded` — 1px #37383e border,
    // 8px radius; cells carry the inner dividers
    // (the Fill-width row dividers enclosed themselves into the column's
    // size hint — Shrink it back: they stretch to the laid-out width)
    let grid = container(scrollable(grid.width(Length::Shrink).height(Length::Shrink)).direction(
        scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::new(),
            horizontal: scrollable::Scrollbar::new(),
        },
    ))
    .style(|_t| container::Style {
        border: iced::border::rounded(8).color(REC_DIVIDER).width(1.0),
        ..Default::default()
    });

    // their header-to-grid gap is mb-4 (16px)
    column![header, grid]
        .spacing(16)
        .height(Fill)
        .into()
}

/// Pixel width of a grid column that must fit the widest of `parts`.
fn cell_w(parts: &[&str]) -> f32 {
    let max = parts
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(6, 34);
    max as f32 * 7.2 + 16.0
}

/// Display order of the record's value columns: PK columns sorted first
/// (the box's rows parallel the node's columns, so the row pk flag rides
/// along), definition order within each group.
fn records_display_order(b: &dbml_ui::layout::TableBox, rec: &dbml_ui::model::NodeRecords) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rec.columns.len()).collect();
    order.sort_by_key(|&p| {
        let pk = rec
            .columns
            .get(p)
            .and_then(|&ci| b.rows.get(ci))
            .is_some_and(|r| r.pk);
        (!pk, p)
    });
    order
}

/// One record cell: the value rendered by kind; a cell whose column refs
/// a table that itself has records renders as a blue clickable that
/// follows to that table's record view (best-effort row match). The
/// column width `w` is precomputed for the whole grid.
/// A record cell's text at the grid's 12px, with the null italic.
fn record_text(label: String, c: Color, italic: bool) -> iced::widget::Text<'static> {
    text(label)
        .size(12.0)
        .color(c)
        .font(Font {
            style: if italic {
                iced::font::Style::Italic
            } else {
                iced::font::Style::Normal
            },
            ..Font::DEFAULT
        })
}

#[allow(clippy::too_many_arguments)]
fn record_cell(
    boxes: &[dbml_ui::layout::TableBox],
    b: &dbml_ui::layout::TableBox,
    rec: &dbml_ui::model::NodeRecords,
    p: usize,
    v: &RecordValue,
    w: f32,
    label: String,
    is_null: bool,
    ink: Color,
) -> Element<'static, Message> {
    let navigable = rec
        .targets
        .get(p)
        .copied()
        .flatten()
        .is_some_and(|_| !matches!(v, RecordValue::Null));
    // their cell inks: FK cells blue + UNDERLINED + clickable; nulls
    // italic "(null)" at half opacity; everything else plain white
    if !navigable {
        let ink = if is_null {
            Color { a: 0.5, ..theme::TEXT }
        } else {
            theme::TEXT
        };
        return record_body_cell(record_text(label, ink, is_null).into(), w);
    }
    record_body_cell(
        button(record_text(label, ink, false))
            .padding(0)
            .style(move |_t, _s| iced::widget::button::Style {
                background: None,
                ..Default::default()
            })
            .on_press(follow_message(boxes, b, rec, p, v))
        .into(),
        w,
    )
}

/// The message for a navigable cell: open the target table's record view
/// with the row whose value matches the cell highlighted, or just open
/// the table when nothing matches (nulls never navigate).
fn follow_message(
    boxes: &[dbml_ui::layout::TableBox],
    b: &dbml_ui::layout::TableBox,
    rec: &dbml_ui::model::NodeRecords,
    p: usize,
    v: &RecordValue,
) -> Message {
    let Some(&(tnode, tcol)) = rec.targets.get(p).copied().flatten().as_ref() else {
        return Message::OpenRecords(b.node);
    };
    let open = Message::OpenRecords(tnode);
    let Some(tb) = boxes.iter().find(|x| x.node == tnode) else {
        return open;
    };
    let Some(trec) = tb.records.as_ref() else {
        return open;
    };
    let Some(q) = trec.columns.iter().position(|&c| c == tcol) else {
        return open;
    };
    match trec.rows.iter().position(|r| r.get(q) == Some(v)) {
        Some(ri) => Message::RecordsFollow(tnode, ri),
        None => open,
    }
}

/// Display text for a record value.
pub(crate) fn record_value_label(v: &RecordValue) -> String {
    match v {
        RecordValue::Null => "(null)".to_string(),
        RecordValue::Text(s) => s.clone(),
        RecordValue::Number(n) => n.clone(),
        RecordValue::Bool(b) => b.to_string(),
        RecordValue::Ident(s) => s.clone(),
        RecordValue::Expr(s) => format!("`{s}`"),
    }
}

/// The table pictogram in the record view's header row.
/// Chrome-like tab strip, canvas-drawn so drag/slide animation has full
/// control of each tab's x (a widget row cannot offset or overlap tabs).
/// One tab per open document with its close X, then a +; click activates,
/// drag reorders (math in `App::tab_drag_moved`), right-click opens the
/// context menu, middle-click closes.
pub struct TabStrip<'a> {
    pub app: &'a App,
}

/// Visual tab width and the strip height.
const TAB_W: f32 = TAB_SLOT_W - 4.0;
pub const STRIP_H: f32 = 30.0;

/// Chrome's active-tab teardrop: convex rounded top corners, then bottom
/// corners that flare outward (concave curls) meeting the strip's bottom
/// line — where the tab, filled with the pane color, merges into the pane
/// below. The outline is deliberately OPEN along the bottom: fills
/// auto-close it, strokes never run a line across the merge.
fn teardrop_path(x0: f32, x1: f32, top: f32, bottom: f32, top_r: f32, curl_r: f32) -> canvas::Path {
    canvas::Path::new(|p| {
        p.move_to(Point::new(x0, bottom));
        // left curl: flare out of the bottom line into the vertical edge
        p.quadratic_curve_to(
            Point::new(x0 + curl_r, bottom),
            Point::new(x0 + curl_r, bottom - curl_r),
        );
        p.line_to(Point::new(x0 + curl_r, top + top_r));
        // convex top-left corner
        p.quadratic_curve_to(
            Point::new(x0 + curl_r, top),
            Point::new(x0 + curl_r + top_r, top),
        );
        p.line_to(Point::new(x1 - curl_r - top_r, top));
        // convex top-right corner
        p.quadratic_curve_to(
            Point::new(x1 - curl_r, top),
            Point::new(x1 - curl_r, top + top_r),
        );
        p.line_to(Point::new(x1 - curl_r, bottom - curl_r));
        // right curl, ending on the bottom line — path left open
        p.quadratic_curve_to(
            Point::new(x1 - curl_r, bottom),
            Point::new(x1, bottom),
        );
    })
}

/// The close button's round hover disc: centered in the 18px click zone
/// at the tab's CURRENT right edge (the open-grow sliver included — a
/// full-TAB_W offset paints the disc outside the growing tab).
fn close_disc(tab_x: f32, width: f32, cy: f32) -> Rectangle {
    // the zone ends at the tab BODY's right edge — the teardrop's curl
    // (6px) is past it, so the disc never rides the slope
    Rectangle::new(
        Point::new(tab_x + width - 28.0, cy - 17.0),
        Size::new(18.0, 34.0),
    )
}

/// Whether the strip boundary before tab `i` (between i-1 and i) shows
/// one of Chrome's short separators: hidden next to the active tab (the
/// merge makes one redundant) and around the dragged tab.
fn separator_visible(i: usize, active: usize, dragged: Option<usize>) -> bool {
    let hidden = |t: usize| t == active || Some(t) == dragged;
    !hidden(i - 1) && !hidden(i)
}

impl TabStrip<'_> {
    /// The tab under a strip-local point, if any (X-zone included). The
    /// point is unshifted; the strip's scroll is added here so hit tests
    /// run in content space, where the slots live.
    fn tab_at(&self, x: f32, y: f32) -> Option<usize> {
        if !(0.0..STRIP_H).contains(&y) {
            return None;
        }
        let x = x + self.app.tab_scroll;
        let i = ((x - STRIP_LEFT) / TAB_SLOT_W).floor();
        if !(0.0..self.app.tabs.len() as f32).contains(&i) {
            return None;
        }
        let i = i as usize;
        (0.0..TAB_W).contains(&(x - STRIP_LEFT - i as f32 * TAB_SLOT_W)).then_some(i)
    }

    fn plus_at(&self, x: f32, y: f32) -> bool {
        // the disc's square bounds: at its DRAWN position (the animated
        // offset included, so a click during the slide lands on it)
        let cx = self.plus_center_x() - self.app.tab_scroll + self.app.plus_anim;
        (0.0..STRIP_H).contains(&y) && (cx - 11.0..cx + 11.0).contains(&x)
    }

    /// The + glyph/disc center: 8px past the last tab's right edge
    /// (Chrome tucks it right up against the strip's last tab), in
    /// CONTENT space.
    fn plus_center_x(&self) -> f32 {
        STRIP_LEFT + self.app.tabs.len() as f32 * TAB_SLOT_W + 8.0
    }
}

impl canvas::Program<Message> for TabStrip<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG);
        let pointer = cursor
            .position()
            .map(|c| Point::new(c.x - bounds.x, c.y - bounds.y));
        let hover_i = pointer.and_then(|l| self.tab_at(l.x, l.y));
        // the scroll shift: strip-space content x moves LEFT by this much
        // (the slots, ghosts, separators and the + all live in content
        // space; hit tests add it back)
        let scroll = self.app.tab_scroll;
        // Chrome's short separators between tab slots, hidden around the
        // active tab (its merge makes one redundant) and the dragged tab
        let dragged_now = self.app.tab_drag.as_ref().map(|d| d.from);
        if self.app.tabs.len() > 1 {
            for i in 1..self.app.tabs.len() {
                if !separator_visible(i, self.app.active, dragged_now) {
                    continue;
                }
                // centered in the 4px gap before the slot
                let x = STRIP_LEFT + i as f32 * TAB_SLOT_W - 3.0 - scroll;
                // 2px, but BORDER at half alpha: the full-brightness gray
                // shouted next to the dim strip (subtle, like Chrome's)
                let sep = Color { a: 0.45, ..theme::BORDER };
                frame.fill_rectangle(
                    Point::new(x, STRIP_H * 0.5 - 9.0),
                    Size::new(2.0, 18.0),
                    sep,
                );
            }
        }
        // the dragged tab draws LAST and OPAQUE (Chrome): while it overlaps
        // a displaced neighbor mid-slide, the neighbor's text must not
        // bleed through it
        let mut order: Vec<usize> = (0..self.app.tabs.len()).collect();
        // closing ghosts draw BEHIND everything (Chromium z-orders the
        // closing tab below): a dimming slab shrinking in place, its title
        // fading out with the width (canvas text can't clip to a rect)
        for g in &self.app.tab_closing {
            let rect = Rectangle::new(
                Point::new(STRIP_LEFT + g.x - scroll, 2.0),
                Size::new(g.w, STRIP_H - 4.0),
            );
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    rect.position(),
                    rect.size(),
                    iced::border::Radius::from(6.0),
                ),
                Color::from_rgba(0.11, 0.129, 0.157, 0.5),
            );
            if g.w > TAB_W / 2.0 {
                let alpha = (g.w / TAB_W).clamp(0.0, 1.0);
                frame.fill_text(canvas::Text {
                    content: g.title.clone(),
                    position: Point::new(rect.x + 12.0, rect.center_y()),
                    color: Color { a: alpha, ..theme::DIM },
                    size: 12.0.into(),
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
            }
        }
        // Chrome's drag z-order: the dragged tab draws LAST (on top) and
        // opaque, so a displaced tab sliding beneath is hidden; everything
        // else draws inactive-first, active last as before.
        let dragged = self.app.tab_drag.as_ref().map(|d| d.from);
        // the dragged tab's drawn rect: any tab intersecting it is HIDDEN
        // (skipped entirely) while the drag is over it. Draw order cannot
        // do this: iced flattens one canvas widget's fills into a single
        // mesh pass and ALL its text into a single later text pass, so the
        // dragged tab's opaque fill covers a neighbor's body but never its
        // title — the overlapped tab must simply not draw.
        let drag_rect = dragged.map(|i| {
            let off = self.app.tab_anim.get(i).copied().unwrap_or(0.0);
            Rectangle::new(
                Point::new(STRIP_LEFT + i as f32 * TAB_SLOT_W + off - scroll, 2.0),
                Size::new(TAB_W, STRIP_H - 4.0),
            )
        });
        order.sort_by_key(|&i| i == self.app.active || Some(i) == dragged);
        // one tab's full paint (fill, border, title, close glyph), shared
        // by the normal path and the clipped-sliver path below
        let paint_tab = |frame: &mut canvas::Frame,
                         i: usize,
                         rect: Rectangle,
                         active: bool,
                         on_top: bool,
                         hovered: bool,
                         close_hover: bool,
                         width: f32| {
            let tab = &self.app.tabs[i];
            if active || on_top {
                // Chrome's teardrop: the shape runs flush to the strip's
                // bottom line, where its PANEL fill merges into the pane
                // below; the stroke stays open across that merge
                let path = teardrop_path(
                    rect.x,
                    rect.x + rect.width,
                    rect.y,
                    STRIP_H,
                    7.0,
                    6.0,
                );
                frame.fill(&path, theme::PANEL);
                frame.stroke(
                    &path,
                    canvas::Stroke {
                        style: theme::BORDER.into(),
                        width: 1.0,
                        ..Default::default()
                    },
                );
            } else if hovered {
                // Chrome's inactive hover: an inset stadium pill
                let pill = Rectangle::new(
                    Point::new(rect.x, 3.0),
                    Size::new(rect.width, STRIP_H - 6.0),
                );
                frame.fill(
                    &canvas::Path::rounded_rectangle(
                        pill.position(),
                        pill.size(),
                        iced::border::Radius::from(7.0),
                    ),
                    Color::from_rgba(0.11, 0.129, 0.157, 0.5),
                );
            }
            if close_hover {
                // Chrome's close button: a round disc behind the ×
                let disc = close_disc(rect.x, width, rect.center_y());
                let c = Point::new(disc.center_x(), rect.center_y());
                frame.fill(
                    &canvas::Path::circle(c, 8.5),
                    Color::from_rgba(1.0, 1.0, 1.0, 0.09),
                );
            }
            let mut title = tab.title();
            if tab.liveness != FileLiveness::InSync {
                // disk problem (conflict / deleted / moved): the durable
                // signal; the toast is the moment signal
                title = format!("! {title}");
            }
            // during the open grow the content fades in past half width
            // (canvas text can't clip to the growing rect)
            let grow = (width / TAB_W).clamp(0.0, 1.0);
            if grow > 0.5 {
                let mut base = if active { theme::TEXT } else { theme::DIM };
                base.a = grow;
                // the unsaved marker: an eight-spoked asterisk DRAWN on
                // the canvas (the mono face has no ✳ glyph — text renders
                // a ? for it) — vertically centered on the tab text's
                // line, in its own slot so the label never shifts
                if tab.is_modified() {
                    // +0.5 optical: the drawn spokes center a half-pixel
                    // high of the × glyph's ink (measured off the strip)
                    let c = Point::new(rect.x + 15.0, rect.center_y() + 0.5);
                    let r = 4.5;
                    for i in 0..4 {
                        let a = i as f32 * std::f32::consts::FRAC_PI_4;
                        let (dx, dy) = (r * a.cos(), r * a.sin());
                        frame.stroke(
                            &canvas::Path::line(
                                Point::new(c.x - dx, c.y - dy),
                                Point::new(c.x + dx, c.y + dy),
                            ),
                            canvas::Stroke {
                                style: base.into(),
                                width: 1.4,
                                ..Default::default()
                            },
                        );
                    }
                }
                frame.fill_text(canvas::Text {
                    // 20px inset so the label doesn't hug the tab's left
                    // edge; vertically CENTERED on the same line the ×
                    // centers on (the old 2px optical lift lifted the
                    // label high of the glyph — the eye catches that)
                    content: truncate_title(&title, 16),
                    // +1.5 optical: the font centers the ink high of the
                    // × glyph's band (measured off the strip)
                    position: Point::new(rect.x + 25.0, rect.center_y() + 1.0),
                    color: base,
                    size: 12.0.into(),
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
                frame.fill_text(canvas::Text {
                    content: "\u{00d7}".to_string(),
                    // CENTER-aligned: the position is the click zone's
                    // center — with the old Left default the glyph's left
                    // edge sat here and it overran the tab's right side
                    position: Point::new(rect.x + width - 19.0, rect.center_y()),
                    color: if close_hover {
                        Color { a: grow, ..theme::TEXT }
                    } else {
                        Color { a: grow, ..theme::DIM }
                    },
                    size: 14.0.into(),
                    horizontal_alignment: iced::alignment::Horizontal::Center,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
            }
        };
        for i in order {
            let off = self.app.tab_anim.get(i).copied().unwrap_or(0.0);
            let active = i == self.app.active;
            let on_top = Some(i) == dragged;
            let x = STRIP_LEFT + i as f32 * TAB_SLOT_W + off - scroll;
            // the open animation: the newest tab grows rightward from the
            // preceding tab's edge (Chromium's left-glued sliver)
            let width = if self.app.tab_opening.is_some() && i + 1 == self.app.tabs.len() {
                self.app.tab_opening.unwrap_or(TAB_W)
            } else {
                TAB_W
            };
            let rect = Rectangle::new(Point::new(x, 2.0), Size::new(width, STRIP_H - 4.0));
            let hovered = hover_i == Some(i) && self.app.tab_drag.is_none();
            // the × disc lights up when the pointer sits in its click zone
            let close_hover = pointer.is_some_and(|p| {
                hovered && p.x >= x + width - 28.0 && p.x <= x + width - 10.0
            });
            // an overlapped tab PARTIALLY hides: draw it only in the
            // slivers sticking out beyond the dragged tab, via clip
            // rectangles (with_clip). Full-hide looked like the tab
            // vanishing; unclipped draw-order cannot hide the title (iced
            // flattens a canvas's fills into one mesh pass and ALL its
            // text into a single later text pass, above every fill).
            let clipped_by = if on_top { None } else { drag_rect.filter(|dr| {
                rect.intersection(dr).is_some_and(|r| r.width > 0.0 && r.height > 0.0)
            }) };
            match clipped_by {
                None => paint_tab(&mut frame, i, rect, active, on_top, hovered, close_hover, width),
                Some(dr) => {
                    let slivers = [
                        // left of the dragged tab
                        Rectangle::new(
                            rect.position(),
                            Size::new((dr.x - rect.x).max(0.0).min(rect.width), rect.height),
                        ),
                        // right of the dragged tab
                        Rectangle::new(
                            Point::new(dr.x + dr.width, rect.y),
                            Size::new(
                                (rect.x + rect.width - (dr.x + dr.width)).max(0.0),
                                rect.height,
                            ),
                        ),
                    ];
                    for sliver in slivers {
                        if sliver.width < 2.0 {
                            continue;
                        }
                        frame.with_clip(sliver, |f| {
                            // the draft frame is pre-translated to the clip
                            // origin: draw the tab shifted back under it
                            let local = Rectangle::new(
                                Point::new(rect.x - sliver.x, rect.y - sliver.y),
                                rect.size(),
                            );
                            paint_tab(
                                f,
                                i,
                                local,
                                active,
                                on_top,
                                hovered,
                                close_hover,
                                width,
                            );
                        });
                    }
                }
            }
        }
        // the + button: Chrome's circular hover disc, tucked against the
        // right-most tab — sliding with the tabs on open/close (its
        // offset eases alongside the slots', so a sliding tab never
        // overlaps a snapped-in-place +)
        let cx = self.plus_center_x() - scroll + self.app.plus_anim;
        let c = Point::new(cx, STRIP_H * 0.5);
        let plus_hover = pointer.is_some_and(|p| {
            (p.x - c.x).abs() <= 11.0 && (p.y - c.y).abs() <= 11.0
        }) && self.app.tab_drag.is_none();
        if plus_hover {
            frame.fill(
                &canvas::Path::circle(c, 10.5),
                Color::from_rgba(1.0, 1.0, 1.0, 0.09),
            );
        }
        frame.fill_text(canvas::Text {
            content: "+".to_string(),
            // measured on-canvas: the '+' glyph's ink centers 1px BELOW
            // its advance center in this font — lift it onto the disc
            position: Point::new(c.x, c.y - 1.0),
            color: if plus_hover { theme::TEXT } else { theme::DIM },
            size: 15.0.into(),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        vec![frame.into_geometry()]
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use iced::mouse::Button;
        let over = cursor
            .position_over(bounds)
            .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y));
        // the wheel scrolls the strip horizontally (Chrome's tab-strip
        // wheel), never the page
        if let canvas::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) = event {
            if over.is_some() {
                // wheel-up = toward the first tab (Chrome): winit hands
                // wheel-up as +1 line, so the scroll delta negates it
                let d = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => -y * 60.0,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => -y,
                };
                if d != 0.0 {
                    return (canvas::event::Status::Captured, Some(Message::TabScroll(d)));
                }
            }
        }
        if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(button)) = event {
            let Some(p) = over else {
                return (canvas::event::Status::Ignored, None);
            };
            match button {
                Button::Left => {
                    if self.plus_at(p.x, p.y) {
                        return (canvas::event::Status::Captured, Some(Message::NewTab));
                    }
                    if let Some(i) = self.tab_at(p.x, p.y) {
                        // matches the drawn zone: 18px ending at the tab
                        // body's right edge (inside the teardrop's curl),
                        // in content space like the drawn rect
                        let x_zone = STRIP_LEFT + i as f32 * TAB_SLOT_W + TAB_W - 28.0
                            - self.app.tab_scroll;
                        let msg = if p.x >= x_zone {
                            Message::CloseTab(i)
                        } else {
                            Message::TabPressed(i)
                        };
                        return (canvas::event::Status::Captured, Some(msg));
                    }
                }
                Button::Middle => {
                    if let Some(i) = self.tab_at(p.x, p.y) {
                        return (canvas::event::Status::Captured, Some(Message::CloseTab(i)));
                    }
                }
                Button::Right => {
                    if let Some(i) = self.tab_at(p.x, p.y) {
                        return (canvas::event::Status::Captured, Some(Message::TabMenu(i)));
                    }
                }
                _ => {}
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        // the tab strip keeps the plain arrow everywhere: hovering and
        // dragging tabs are ordinary actions, not links (no hyperlink
        // hand, no grabbing fist — the rearrange drag reads without one)
        iced::mouse::Interaction::default()
    }
}

/// Clip a tab title to `max` chars, ellipsis at the end.
pub(crate) fn truncate_title(title: &str, max: usize) -> String {
    let count = title.chars().count();
    if count <= max {
        return title.to_string();
    }
    format!(
        "{}\u{2026}",
        title.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

/// The tab context menu (Close / Close all), anchored at the pointer and
/// clamped into the window, over an invisible click-away backdrop.
/// The editor-mode select menu: anchored under the toolbar's mode
/// button, two options with the active one accented, click-away closes.
fn vim_menu_overlay(app: &App, at: &Point) -> Element<'static, Message> {
    const MENU_W: f32 = 170.0;
    const MENU_H: f32 = 76.0;
    let x = at.x.min((app.window_size.0 - MENU_W).max(0.0));
    let y = (at.y + 8.0).min((app.window_size.1 - MENU_H).max(0.0));
    let item = |label: &'static str, active: bool, msg: Message| {
        button(
            row![
                text(label.to_string()).size(12.0).color(theme::TEXT),
                horizontal_space(),
                if active {
                    Element::from(mark_svg(MARK_CHECK, theme::TEXT))
                } else {
                    Element::from(
                        text("".to_string()).size(11.0).color(Color::TRANSPARENT),
                    )
                },
            ]
            .spacing(4)
            .align_y(iced::alignment::Vertical::Center),
        )
        .width(Fill)
        .padding(iced::Padding { top: 8.0, right: 10.0, bottom: 6.0, left: 10.0 })
        .style(move |_t, s| {
            let mut st = tool_button_style();
            if s == iced::widget::button::Status::Hovered {
                st.background = Some(iced::Background::Color(theme::TOOLTIP));
            }
            st
        })
        .on_press(msg)
    };
    let panel = container(
        column![
            item("Vim Mode", app.config.vim, Message::VimMode(true)),
            item(
                "Regular Editor",
                !app.config.vim,
                Message::VimMode(false)
            ),
        ]
        .spacing(2)
        .padding(4),
    )
    .width(Length::Fixed(MENU_W))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.08, 0.09, 0.11, 0.98,
        ))),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
    });
    // click anywhere outside the menu dismisses it
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::VimMenuClose);
    stack![
        backdrop,
        container(panel)
            .padding(iced::Padding {
                top: y,
                left: x,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill)
    ]
    .into()
}

fn tab_menu_overlay(app: &App, menu: &TabMenu) -> Element<'static, Message> {
    const MENU_W: f32 = 150.0;
    const MENU_H: f32 = 78.0;
    let x = menu.at.x.min((app.window_size.0 - MENU_W).max(0.0));
    let y = menu.at.y.min((app.window_size.1 - MENU_H).max(0.0));
    let item = |label: &'static str, msg: Message| {
        button(text(label).size(12.0).color(theme::TEXT))
            .padding([5, 10])
            .width(Fill)
            .style(move |_t, _s| tool_button_style())
            .on_press(msg)
    };
    let panel = container(
        column![
            item("Close", Message::TabMenuClose),
            item("Close all", Message::TabMenuCloseAll),
        ]
        .spacing(2)
        .padding(4),
    )
    .width(Length::Fixed(MENU_W))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.08, 0.09, 0.11, 0.98,
        ))),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
    });
    // click anywhere outside the menu dismisses it
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::DismissTabMenu);
    // anchor by padding offset: hspace/vspace columns don't offset in
    // iced 0.13 flex (the stack child would land at (0,0))
    stack![
        backdrop,
        container(panel)
            .padding(iced::Padding {
                top: y,
                left: x,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill)
    ]
    .into()
}

/// The editor context menu (Copy / Cut / Paste / Format), anchored at the
/// pointer and clamped into the window, over an invisible click-away
/// backdrop — same pattern as `tab_menu_overlay`. Items whose snapshot
/// condition failed (no selection / no clipboard text) render dimmed and
/// do not press.
fn editor_menu_overlay(app: &App, menu: &EditorMenu) -> Element<'static, Message> {
    const MENU_W: f32 = 150.0;
    const MENU_H: f32 = 132.0;
    let x = menu.at.x.min((app.window_size.0 - MENU_W).max(0.0));
    let y = menu.at.y.min((app.window_size.1 - MENU_H).max(0.0));
    let item = |label: &'static str, msg: Message, enabled: bool| {
        button(
            text(label)
                .size(12.0)
                .color(if enabled { theme::TEXT } else { theme::DIM }),
        )
        .padding([5, 10])
        .width(Fill)
        .style(move |_t, _s| tool_button_style())
        .on_press_maybe(enabled.then_some(msg))
    };
    let panel = container(
        column![
            item("Copy", Message::EditorMenuCopy, menu.has_selection),
            item("Cut", Message::EditorMenuCut, menu.has_selection),
            item("Paste", Message::EditorMenuPaste, menu.clipboard_has_text),
            // vim gd in button form: bottom of the menu, dimmed unless the
            // right-clicked word has a definition
            item(
                "Go to definition",
                Message::EditorMenuGotoDefinition,
                menu.has_definition,
            ),
        ]
        .spacing(2)
        .padding(4),
    )
    .width(Length::Fixed(MENU_W))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.08, 0.09, 0.11, 0.98,
        ))),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
    });
    // click anywhere outside the menu dismisses it
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::DismissEditorMenu);
    stack![
        backdrop,
        container(panel)
            .padding(iced::Padding {
                top: y,
                left: x,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill)
    ]
    .into()
}

/// dbdiagram's header-color palette (their table-header picker): rounded
/// swatch bars, the picked one ringed with a check.
/// The picker bubble's surface: lighter than the usual panels so the
/// dark swatch (the default header color) stays visible against it.
const PICKER_BG: Color = Color::from_rgb(0.312, 0.337, 0.380); // #505661

/// The picker's 9 colors, in the 3x3 grid order: blues / greens /
/// warm-and-dark.
const HEADER_PALETTE: [&str; 9] = [
    // top: blue, navy, the DEFAULT header color (theme::HEADER_BG — the
    // same dark a table without `headercolor` renders)
    "#1e69fd", "#2d5f8a", crate::theme::DARK_HEADER_HEX,
    // middle: green, teal, purple
    "#1f8a4c", "#16a085", "#8e44ad",
    // bottom: orange, red-orange, red
    "#e67e22", "#d35400", "#e74c3c",
];

/// The header-color menu (click the palette glyph on a hovered header): a
/// dark popup — dbdiagram's 3x3 circle grid plus a Default row — anchored
/// clamped into the window, over an invisible click-away backdrop — same
/// pattern as `editor_menu_overlay`. The table's current color (its
/// `headercolor` setting, or Default when none) carries a check.
fn table_header_menu_overlay(app: &App, menu: &TableHeaderMenu) -> Element<'static, Message> {
    // dbdiagram's picker: a compact 3x3 grid of CIRCLES (+ Default row,
    // + a 10th swatch when the source carries a hand-typed hex), a cell
    // of the grid 26px with the circle 16px
    const CELL: f32 = 26.0;
    // exact fit: 3 16px circles + 2 10px gaps + the Default row (10px gap
    // + 18px) + 16px padding + 2px border — the old CELL-based width left
    // 11px of dead space on the right. A custom-color row grows the panel
    // by one CELL at runtime (see menu_h below).
    const MENU_W: f32 = 86.0;
    const MENU_H: f32 = 114.0;
    const BAND_HALF: f32 = 10.0;
    // the current color by target: a table's headercolor, or the group's
    // `[color:]` (default-blue groups read as None). `pick_msg` builds
    // the right message per target.
    type PickFn = Box<dyn Fn(Option<String>) -> Message>;
    let (current, pick_msg): (Option<String>, PickFn) = match &menu.target {
            crate::app::PickerTarget::Table(node) => (
                app.diagram()
                    .boxes
                    .iter()
                    .find(|b| b.node == *node)
                    .and_then(|b| b.header_color.clone()),
                {
                    let node = *node;
                    Box::new(move |hex: Option<String>| Message::TableHeaderColor(node, hex))
                },
            ),
            crate::app::PickerTarget::Group(name) => (
                app.diagram()
                    .groups
                    .iter()
                    .find(|g| g.name == name.as_str())
                    .and_then(|g| g.color.clone()),
                {
                    let name = name.clone();
                    Box::new(move |hex: Option<String>| Message::GroupColor(name.clone(), hex))
                },
            ),
            // a relationship line: its resolved color (explicit
            // [color:] or the referenced table's inheritance)
            crate::app::PickerTarget::Edge(ei) => (
                app.diagram()
                    .edges
                    .get(*ei)
                    .and_then(|e| e.color.clone()),
                {
                    let ei = *ei;
                    Box::new(move |hex: Option<String>| Message::EdgeColor(ei, hex))
                },
            ),
        };
    let swatch = |hex: &str| -> Element<'static, Message> {
        // every swatch is EXPLICIT: the 2px white ring marks the color
        // actually in effect for this target — the dark swatch reads as
        // current only when the source literally carries #1f2128 (the
        // Default row below owns the "no explicit color" state)
        let picked = current.as_deref() == Some(hex);
        let base = theme::hex_color(hex).unwrap_or(theme::PANEL);
        button(text("").size(10.0))
        // zero padding: the button's default padding would shrink the
        // content box below the glyph and the renderer skips it entirely
        // (the label vanished on every fixed-size button until this)
        .padding(0.0)
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0))
            .style(move |_t, s| {
                // hover brightening: the swatch tints toward white, dbdiagram's
                // picker feedback
                let bg = if s == iced::widget::button::Status::Hovered {
                    brighten(base)
                } else {
                    base
                };
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::border::rounded(8)
                        .width(if picked { 2.0 } else { 1.0 })
                        .color(if picked {
                            Color::WHITE
                        } else {
                            Color::TRANSPARENT
                        }),
                    ..Default::default()
                }
            })
            .on_press(pick_msg(Some(hex.to_string())))
            .into()
    };
    // the 3x3 grid; below it the Default row — the EXPLICIT "no color"
    // pick that clears the entry so the cascade decides (group → table →
    // line inheritance). The dark swatch is a regular explicit color; this
    // row owns the "no explicit color" state (its 2px white border marks it)
    let mut grid_rows: Vec<Element<'static, Message>> = Vec::new();
    for row in HEADER_PALETTE.chunks(3) {
        grid_rows.push(
            iced::widget::Row::with_children(
                row.iter().map(|hex| swatch(hex)).collect::<Vec<_>>(),
            )
            .spacing(CELL - 16.0)
            .into(),
        );
    }
    // a color TYPED into the DBML by hand (any hex) is outside the 9
    // swatches: a 10th swatch carries it — the active marker (ring +
    // check) always has a home, and clicking it re-asserts the same hex.
    // Only the panel's height grows, by exactly one swatch row.
    let custom = current
        .as_ref()
        .filter(|c| !HEADER_PALETTE.iter().any(|h| h == c));
    let menu_h = MENU_H + if custom.is_some() { CELL } else { 0.0 };
    // the panel targets a fixed offset from WHAT it edits (the same math
    // as before, with the dynamic height): a table or group gets the band
    // treatment, an edge gets the pointer position with the bubble
    // vertically centered on the line. The clamp stays: a panel pushed
    // off the window snaps back inside, and the tail is dropped when that
    // happens (it would point at nothing).
    let (tip, target_x, target_y) = match &menu.target {
        crate::app::PickerTarget::Edge(_) => (
            Point::new(menu.at.x, menu.at.y),
            menu.at.x + 6.0,
            menu.at.y - menu_h / 2.0,
        ),
        _ => (
            Point::new(menu.at.x, menu.at.y + BAND_HALF),
            menu.at.x + 6.0,
            menu.at.y + BAND_HALF - menu_h / 2.0,
        ),
    };
    let clamped = target_x + MENU_W > app.window_size.0
        || target_y < 0.0
        || target_y + menu_h > app.window_size.1;
    let x = target_x.min((app.window_size.0 - MENU_W).max(0.0));
    let y = target_y.max(0.0).min((app.window_size.1 - menu_h).max(0.0));
    if let Some(hex) = &custom {
        grid_rows.push(
            iced::widget::Row::with_children(vec![swatch(hex.as_str())])
                .spacing(CELL - 16.0)
                .into(),
        );
    }
    let default_picked = current.is_none();
    grid_rows.push(
        // a container+text+mouse_area row, NOT a `button`: the button
        // widget painted its chrome but never its label here (bg + border
        // render, the child text never does — iced 0.13 quirk), while the
        // container path is the same one every popup label uses
        mouse_area(
            // the label keeps the readable TEXT ink in BOTH states — DIM
            // read as a disabled button — and the active state is the
            // 2px white border alone (the same marker the swatches use)
            container(text("Default").size(10.0).color(theme::TEXT))
                .width(Length::Fixed(68.0))
                .height(Length::Fixed(18.0))
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center)
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(brighten(PICKER_BG))),
                    // picked = no explicit color anywhere for this target:
                    // the white 2px border is the row's active marker (the
                    // swatches carry the same ring)
                    border: iced::border::rounded(4)
                        .width(if default_picked { 2.0 } else { 1.0 })
                        .color(if default_picked {
                            Color::WHITE
                        } else {
                            theme::BORDER
                        }),
                    ..Default::default()
                }),
        )
        .on_press(pick_msg(None))
        .into(),
    );
    let panel = container(column(grid_rows).spacing(CELL - 16.0).padding(8))
    .width(Length::Fixed(MENU_W))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(PICKER_BG)),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
    });
    // click outside the diagram canvas dismisses the menu — ON THE PRESS
    // (mousedown), so the same gesture that closes the picker can already
    // act underneath: dragging a table by its header needs no extra click
    // to close the picker first. Presses over the diagram canvas fall
    // through the sheet entirely (canvas_pressed dismisses the menu
    // itself, then the same press grabs the table); presses over the
    // editor, the tab strip and every other surface land on the sheet.
    let backdrop = canvas(DismissSheet {
        hole: diagram_canvas_rect(app),
    })
    .width(Fill)
    .height(Fill);
    stack![
        backdrop,
        // the speech-bubble tail: only when unclamped (the clamped panel
        // no longer sits next to the header, so the tail would lie)
        canvas(PickerTail {
            tip,
            base_x: x,
            center_y: tip.y,
            draw: !clamped,
        })
        .width(Fill)
        .height(Fill),
        container(panel)
            .padding(iced::Padding {
                top: y,
                left: x,
                ..Default::default()
            })
            .width(Fill)
            .height(Fill)
    ]
    .into()
}

/// The picker's speech-bubble tail: a small triangle from the panel's
/// left edge to the header band (tip ON the band), filled with the
/// panel's background so it reads as part of the bubble. Drawn under the
/// panel, so the base seam is covered.
struct PickerTail {
    /// The tail's tip: ON the header band (its right edge).
    tip: Point,
    /// The panel's left edge x (the base sits here).
    base_x: f32,
    /// The band's vertical center (the tail is symmetric about it).
    center_y: f32,
    /// False while the panel is clamped: no tail.
    draw: bool,
}

impl canvas::Program<Message> for PickerTail {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        if self.draw {
            const HALF: f32 = 5.0; // half-height at the base
            let bg = PICKER_BG;
            frame.fill(
                &iced::widget::canvas::Path::new(|p| {
                    p.move_to(self.tip);
                    p.line_to(Point::new(self.base_x, self.center_y - HALF));
                    p.line_to(Point::new(self.base_x, self.center_y + HALF));
                    p.line_to(self.tip);
                }),
                bg,
            );
            // the two diagonal edges carry the panel's border so the tail
            // reads as part of the bubble, not a sticker
            frame.stroke(
                &iced::widget::canvas::Path::new(|p| {
                    p.move_to(self.tip);
                    p.line_to(Point::new(self.base_x, self.center_y - HALF));
                    p.move_to(self.tip);
                    p.line_to(Point::new(self.base_x, self.center_y + HALF));
                }),
                canvas::Stroke {
                    style: theme::BORDER.into(),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }
}

/// The picker's dismiss sheet: an invisible full-window canvas that
/// closes the menu on MOUSE PRESS — everywhere except the diagram
/// canvas's rect, where presses fall through so the same mousedown can
/// drag a table (canvas_pressed dismisses the menu itself, first thing).
/// The old full-window backdrop button dismissed on RELEASE and
/// swallowed the whole gesture: closing a picker took one click and the
/// drag underneath it another.
struct DismissSheet {
    /// The diagram canvas's window rect; presses inside fall through.
    hole: Option<Rectangle>,
}

impl canvas::Program<Message> for DismissSheet {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                let inside_hole = self
                    .hole
                    .zip(cursor.position_over(bounds))
                    .is_some_and(|(h, p)| h.contains(p));
                if inside_hole {
                    // the diagram canvas handles it: dismiss + grab in one
                    // gesture happens there
                    (canvas::event::Status::Ignored, None)
                } else {
                    (canvas::event::Status::Captured, Some(Message::DismissTableHeaderMenu))
                }
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // nothing to paint: an empty frame keeps the sheet invisible
        let frame = canvas::Frame::new(renderer, bounds.size());
        vec![frame.into_geometry()]
    }
}

/// The diagram canvas's window rect, when the menu could sit over one:
/// presses inside it fall through the dismiss sheet to the canvas (which
/// closes the menu mousedown-first and lets the press act). The hole is
/// grown a pixel up and left so ±1px FillPortion rounding on the pane
/// split never eats a canvas press — a press that lands on the dismissed
/// sheet instead of the canvas is exactly the second click this sheet
/// exists to remove.
fn diagram_canvas_rect(app: &App) -> Option<Rectangle> {
    if !app.doc_open() || app.maximized == Maximized::Editor {
        return None;
    }
    let w = app.window_size.0;
    let h = app.window_size.1;
    let x = if app.maximized == Maximized::Diagram {
        0.0
    } else {
        (app.split * w - 1.0).max(0.0)
    };
    Some(Rectangle::new(
        Point::new(x, DIAGRAM_CANVAS_TOP - 1.0),
        Size::new(
            (w - x).max(0.0),
            (h - DIAGRAM_CANVAS_TOP + 1.0).max(0.0),
        ),
    ))
}

/// A swatch's hover tint: every channel nudged toward white.
fn brighten(c: Color) -> Color {
    Color {
        r: c.r + (1.0 - c.r) * 0.18,
        g: c.g + (1.0 - c.g) * 0.18,
        b: c.b + (1.0 - c.b) * 0.18,
        a: c.a,
    }
}

// ---- toolbar glyphs: Font Awesome solid paths lifted from dbdiagram's
// bundle (save / edit / search) and the FA6 free set (undo / redo /
// format wand / maximize expand + compress), rendered as inline SVGs
// with the fill baked in (iced SVGs can't recolor currentColor).
const ICON_SAVE: (&str, &str) = ("M433.941 129.941l-83.882-83.882A48 48 0 0 0 316.118 32H48C21.49 32 0 53.49 0 80v352c0 26.51 21.49 48 48 48h352c26.51 0 48-21.49 48-48V163.882a48 48 0 0 0-14.059-33.941zM272 80v80H144V80h128zm122 352H54a6 6 0 0 1-6-6V86a6 6 0 0 1 6-6h42v104c0 13.255 10.745 24 24 24h176c13.255 0 24-10.745 24-24V83.882l78.243 78.243a6 6 0 0 1 1.757 4.243V426a6 6 0 0 1-6 6zM224 232c-48.523 0-88 39.477-88 88s39.477 88 88 88 88-39.477 88-88-39.477-88-88-88zm0 128c-22.056 0-40-17.944-40-40s17.944-40 40-40 40 17.944 40 40-17.944 40-40 40z", "0 0 448 512");
const ICON_SAVE_AS: (&str, &str) = ("M433.941 129.941l-83.882-83.882A48 48 0 0 0 316.118 32H48C21.49 32 0 53.49 0 80v352c0 26.51 21.49 48 48 48h352c26.51 0 48-21.49 48-48V163.882a48 48 0 0 0-14.059-33.941zM272 80v80H144V80h128zm122 352H54a6 6 0 0 1-6-6V86a6 6 0 0 1 6-6h42v104c0 13.255 10.745 24 24 24h176c13.255 0 24-10.745 24-24V83.882l78.243 78.243a6 6 0 0 1 1.757 4.243V426a6 6 0 0 1-6 6zM224 232c-48.523 0-88 39.477-88 88s39.477 88 88 88 88-39.477 88-88-39.477-88-88-88zm0 128c-22.056 0-40-17.944-40-40s17.944-40 40-40 40 17.944 40 40-17.944 40-40 40z|M438.6 278.6c12.5-12.5 12.5-32.8 0-45.3l-160-160c-12.5-12.5-32.8-12.5-45.3 0s-12.5 32.8 0 45.3L338.8 224 32 224c-17.7 0-32 14.3-32 32s14.3 32 32 32l306.7 0L233.4 393.4c-12.5 12.5-12.5 32.8 0 45.3s32.8 12.5 45.3 0l160-160z", "0 0 448 512");
const ICON_SEARCH: (&str, &str) = ("M505 442.7L405.3 343c-4.5-4.5-10.6-7-17-7H372c27.6-35.3 44-79.7 44-128C416 93.1 322.9 0 208 0S0 93.1 0 208s93.1 208 208 208c48.3 0 92.7-16.4 128-44v16.3c0 6.4 2.5 12.5 7 17l99.7 99.7c9.4 9.4 24.6 9.4 33.9 0l28.3-28.3c9.4-9.4 9.4-24.6.1-34zM208 336c-70.7 0-128-57.2-128-128 0-70.7 57.2-128 128-128 70.7 0 128 57.2 128 128 0 70.7-57.2 128-128 128z", "0 0 512 512");
const ICON_UNDO: (&str, &str) = ("M125.7 160l50.3 0c17.7 0 32 14.3 32 32s-14.3 32-32 32L48 224c-17.7 0-32-14.3-32-32L16 64c0-17.7 14.3-32 32-32s32 14.3 32 32l0 51.2L97.6 97.6c87.5-87.5 229.3-87.5 316.8 0s87.5 229.3 0 316.8s-229.3 87.5-316.8 0c-12.5-12.5-12.5-32.8 0-45.3s32.8-12.5 45.3 0c62.5 62.5 163.8 62.5 226.3 0s62.5-163.8 0-226.3s-163.8-62.5-226.3 0L125.7 160z", "0 0 512 512");
const ICON_REDO: (&str, &str) = ("M386.3 160L336 160c-17.7 0-32 14.3-32 32s14.3 32 32 32l128 0c17.7 0 32-14.3 32-32l0-128c0-17.7-14.3-32-32-32s-32 14.3-32 32l0 51.2L414.4 97.6c-87.5-87.5-229.3-87.5-316.8 0s-87.5 229.3 0 316.8s229.3 87.5 316.8 0c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0c-62.5 62.5-163.8 62.5-226.3 0s-62.5-163.8 0-226.3s163.8-62.5 226.3 0L386.3 160z", "0 0 512 512");
const ICON_FORMAT: (&str, &str) = ("M234.7 42.7L197 56.8c-3 1.1-5 4-5 7.2s2 6.1 5 7.2l37.7 14.1L248.8 123c1.1 3 4 5 7.2 5s6.1-2 7.2-5l14.1-37.7L315 71.2c3-1.1 5-4 5-7.2s-2-6.1-5-7.2L277.3 42.7 263.2 5c-1.1-3-4-5-7.2-5s-6.1 2-7.2 5L234.7 42.7zM46.1 395.4c-18.7 18.7-18.7 49.1 0 67.9l34.6 34.6c18.7 18.7 49.1 18.7 67.9 0L529.9 116.5c18.7-18.7 18.7-49.1 0-67.9L495.3 14.1c-18.7-18.7-49.1-18.7-67.9 0L46.1 395.4zM484.6 82.6l-105 105-23.3-23.3 105-105 23.3 23.3zM7.5 117.2C3 118.9 0 123.2 0 128s3 9.1 7.5 10.8L64 160l21.2 56.5c1.7 4.5 6 7.5 10.8 7.5s9.1-3 10.8-7.5L128 160l56.5-21.2c4.5-1.7 7.5-6 7.5-10.8s-3-9.1-7.5-10.8L128 96 106.8 39.5C105.1 35 100.8 32 96 32s-9.1 3-10.8 7.5L64 96 7.5 117.2zm352 256c-4.5 1.7-7.5 6-7.5 10.8s3 9.1 7.5 10.8L416 416l21.2 56.5c1.7 4.5 6 7.5 10.8 7.5s9.1-3 10.8-7.5L480 416l56.5-21.2c4.5-1.7 7.5-6 7.5-10.8s-3-9.1-7.5-10.8L480 352l-21.2-56.5c-1.7-4.5-6-7.5-10.8-7.5s-9.1 3-10.8 7.5L416 352l-56.5 21.2z", "0 0 576 512");
const ICON_EXPAND: (&str, &str) = ("M344 0L488 0c13.3 0 24 10.7 24 24l0 144c0 9.7-5.8 18.5-14.8 22.2s-19.3 1.7-26.2-5.2l-39-39-87 87c-9.4 9.4-24.6 9.4-33.9 0l-32-32c-9.4-9.4-9.4-24.6 0-33.9l87-87L327 41c-6.9-6.9-8.9-17.2-5.2-26.2S334.3 0 344 0zM168 512L24 512c-13.3 0-24-10.7-24-24L0 344c0-9.7 5.8-18.5 14.8-22.2s19.3-1.7 26.2 5.2l39 39 87-87c9.4-9.4 24.6-9.4 33.9 0l32 32c9.4 9.4 9.4 24.6 0 33.9l-87 87 39 39c6.9 6.9 8.9 17.2 5.2 26.2s-12.5 14.8-22.2 14.8z", "0 0 512 512");
/// Font Awesome's file-export (a document with an outbound arrow) —
/// reads as "export" at a glance; the Export trigger button uses it.
const ICON_EXPORT: (&str, &str) = ("M0 64C0 28.7 28.7 0 64 0L224 0l0 128c0 17.7 14.3 32 32 32l128 0 0 128-168 0c-13.3 0-24 10.7-24 24s10.7 24 24 24l168 0 0 112c0 35.3-28.7 64-64 64L64 512c-35.3 0-64-28.7-64-64L0 64zM384 336l0-48 110.1 0-39-39c-9.4-9.4-9.4-24.6 0-33.9s24.6-9.4 33.9 0l80 80c9.4 9.4 9.4 24.6 0 33.9l-80 80c-9.4 9.4-24.6 9.4-33.9 0s-9.4-24.6 0-33.9l39-39L384 336zm0-208l-128 0L256 0 384 128z", "0 0 576 512");
const MARK_CHECK: &str = "M438.6 105.4c12.5 12.5 12.5 32.8 0 45.3l-256 256c-12.5 12.5-32.8 12.5-45.3 0l-128-128c-12.5-12.5-12.5-32.8 0-45.3s32.8-12.5 45.3 0L160 338.7 393.4 105.4c12.5-12.5 32.8-12.5 45.3 0z";
const ICON_COMPRESS: (&str, &str) = ("M439 7c9.4-9.4 24.6-9.4 33.9 0l32 32c9.4 9.4 9.4 24.6 0 33.9l-87 87 39 39c6.9 6.9 8.9 17.2 5.2 26.2s-12.5 14.8-22.2 14.8l-144 0c-13.3 0-24-10.7-24-24l0-144c0-9.7 5.8-18.5 14.8-22.2s19.3-1.7 26.2 5.2l39 39L439 7zM72 272l144 0c13.3 0 24 10.7 24 24l0 144c0 9.7-5.8 18.5-14.8 22.2s-19.3 1.7-26.2-5.2l-39-39L73 505c-9.4 9.4-24.6 9.4-33.9 0L7 473c-9.4-9.4-9.4-24.6 0-33.9l87-87L55 313c-6.9-6.9-8.9-17.2-5.2-26.2s12.5-14.8 22.2-14.8z", "0 0 512 512");

/// The toolbar's state-mark glyph (the fmt checkbox's check, the mode
/// menu's check) as an inline svg in `ink`.
fn mark_svg(d: &str, ink: Color) -> Element<'static, Message> {
    let (r, g, b) = (
        (ink.r * 255.0) as u32,
        (ink.g * 255.0) as u32,
        (ink.b * 255.0) as u32,
    );
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 512 512' width='9' height='9'><path d='{d}' fill='#{r:02x}{g:02x}{b:02x}'/></svg>",
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(9.0))
    .height(Length::Fixed(9.0))
    .into()
}

/// Small toolbar above the editor: Undo, Redo, Save, Save As, Format,
/// the fmt-on-save switch and the vim/Regular mode switch.
fn editor_toolbar(app: &App) -> Element<'static, Message> {
    use iced::widget::button;
    // an icon button: the FA glyph tinted per state, with a hover tooltip
    // label (icon-only buttons need their name on hover); disabled = dim
    // + inert. `active` fills blue, matching the Highlight toggle.
    let icon_btn = |icon: (&'static str, &'static str),
                    label: &'static str,
                    msg: Message,
                    enabled: bool,
                    active: bool| {
        // "path|path" composes a main glyph + a corner badge (Save As:
        // the floppy with a right-arrow, dbdiagram's save-as read)
        let (paths, vb) = icon;
        let (d, badge) = match paths.split_once('|') {
            Some((main, badge)) => (main, Some(badge)),
            None => (paths, None),
        };
        let ink = if !enabled {
            theme::DIM
        } else if active {
            Color::WHITE
        } else {
            theme::TEXT
        };
        let (r, g, b) = (
            (ink.r * 255.0) as u32,
            (ink.g * 255.0) as u32,
            (ink.b * 255.0) as u32,
        );
        let ink_hex = format!("#{r:02x}{g:02x}{b:02x}");
        let badge_svg = badge.map(|bd| {
            format!(
                "<g transform='translate(300 330) scale(0.5)'><path d='{bd}' fill='#{:02x}{:02x}{:02x}'/></g>",
                (theme::BG.r * 255.0) as u32,
                (theme::BG.g * 255.0) as u32,
                (theme::BG.b * 255.0) as u32,
            )
        });
        let svg = iced::widget::svg(iced::widget::svg::Handle::from_memory(
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{vb}' width='14' height='14'><path d='{d}' fill='{ink_hex}'/>{}</svg>",
                badge_svg.unwrap_or_default(),
            )
            .into_bytes(),
        ))
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0));
        mouse_area(
            button(svg)
                .padding([5, 8])
                .style(move |_t, s| {
                    let mut st = tool_button_style();
                    if active {
                        st.background = Some(iced::Background::Color(match s {
                            iced::widget::button::Status::Hovered => {
                                Color { a: 0.85, ..theme::FK }
                            }
                            _ => theme::FK,
                        }));
                        st.border = iced::border::rounded(6).color(theme::FK);
                    }
                    st
                })
                .on_press_maybe(enabled.then_some(msg)),
        )
        .on_enter(Message::ToolbarTip(Some(label)))
        .on_exit(Message::ToolbarTip(None))
    };
    // the two mode switches stay text: they are states with on/off
    // accents, not triggers
    // the state mark: a checkbox — a rounded square with the check
    // inside when on, an empty box when off (the check/xmark codepoints
    // render as question marks in the UI font)
    let checkbox = |checked: bool, ink: Color| {
        let (r, g, b) = (
            (ink.r * 255.0) as u32,
            (ink.g * 255.0) as u32,
            (ink.b * 255.0) as u32,
        );
        let ink_hex = format!("#{r:02x}{g:02x}{b:02x}");
        let check = if checked {
            format!(
                "<g transform='translate(96 96) scale(0.625)'><path d='{}' fill='{ink_hex}'/></g>",
                MARK_CHECK
            )
        } else {
            String::new()
        };
        iced::widget::svg(iced::widget::svg::Handle::from_memory(
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 512 512' width='11' height='11'><rect x='48' y='48' width='416' height='416' rx='80' fill='none' stroke='{ink_hex}' stroke-width='56'/>{check}</svg>",
            )
            .into_bytes(),
        ))
        .width(Length::Fixed(11.0))
        .height(Length::Fixed(11.0))
    };
    let (checked, mark_ink) = (app.config.fmt_on_save, if app.config.fmt_on_save {
        theme::FK
    } else {
        theme::DIM
    });
    let fmt_switch = button(
        row![
            text("Format on Save".to_string()).size(11.0).color(mark_ink),
            container(checkbox(checked, mark_ink)),
        ]
        .spacing(5)
        .align_y(iced::alignment::Vertical::Center),
    )
    // the 11px label's ink sits ~2.5px above its line-box center (no
    // descenders): the heavier top padding drops it onto the button's
    // true center, checkbox riding along
    .padding(iced::Padding { top: 5.0, right: 8.0, bottom: 4.0, left: 8.0 })
    .style(move |_t, _s| tool_button_style())
    .on_press(Message::ToggleFmtOnSave);
    // the mode switch shows the CURRENT mode (what you are in, not what a
    // press would switch to); its accent mirrors the fmt switch's on/off
    // the editor mode as a select menu: the button shows the CURRENT
    // mode and opens the two-option menu (Vim Mode / Regular Editor)
    let vim_label = if app.config.vim {
        "Vim Mode"
    } else {
        "Regular Editor"
    };
    let vim_switch = mouse_area(
        button(
            row![
                text(vim_label.to_string()).size(11.0).color(theme::TEXT),
                chevron_svg(true),
            ]
            .spacing(5)
            .align_y(iced::alignment::Vertical::Center),
        )
        .padding(iced::Padding { top: 5.0, right: 8.0, bottom: 4.0, left: 8.0 })
        .style(move |_t, _s| tool_button_style())
        .on_press(Message::VimMenuOpen),
    )
    .on_enter(Message::ToolbarTip(Some("Editor mode")))
    .on_exit(Message::ToolbarTip(None));
    // left-aligned to the editor's line-number gutter (56px), so the
    // toolbar reads as part of the editor column
    let maximized = app.maximized == Maximized::Editor;
    let save_slot: Element<'static, Message> = icon_btn(
        ICON_SAVE,
        "Save (Ctrl+S)",
        Message::KeyPressed(KeyCode::Char('s'), KeyModifiers::CONTROL),
        true,
        false,
    )
    .into();
    let row = row![
        save_slot,
        icon_btn(ICON_SAVE_AS, "Save As (F12)", Message::SaveAsPrompt, true, false),
        icon_btn(
            ICON_UNDO,
            "Undo (Ctrl+Z)",
            Message::Undo,
            app.editor().buf.can_undo(),
            false
        ),
        icon_btn(
            ICON_REDO,
            "Redo (Ctrl+Y)",
            Message::Redo,
            app.editor().buf.can_redo(),
            false
        ),
        icon_btn(
            ICON_SEARCH,
            "Find (Ctrl+F)",
            Message::FindToggle,
            true,
            app.editor().find_open()
        ),
        icon_btn(ICON_FORMAT, "Format", Message::FormatNow, true, false),
        fmt_switch,
        vim_switch,
        horizontal_space(),
        // F1 in button form: the editor over the whole pane row; the
        // expand/contract glyph + fill track the maximized state
        icon_btn(
            if maximized {
                ICON_COMPRESS
            } else {
                ICON_EXPAND
            },
            if maximized {
                "Restore editor"
            } else {
                "Maximize editor"
            },
            Message::ToggleEditorMaximize,
            true,
            maximized
        ),
    ]
    .spacing(4)
    .align_y(iced::alignment::Vertical::Center);
    // the toolbar's own band: a darker strip than the editor beneath it,
    // so the row reads as chrome instead of blending into the buffer
    container(row)
        .width(Fill)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(theme::BG)),
            ..Default::default()
        })
        .into()
}

/// The gui find bar (ctrl+f / the toolbar Find button): an opaque widget
/// pinned to the pane's top-right — the live query, the match counter,
/// and the prev/next/close buttons (the keys still route through
/// `handle_find_key`; the buttons only click).
fn find_bar_overlay(app: &App) -> Option<Element<'_, Message>> {
    if !app.editor().find_open() {
        return None;
    }
    let find = app.editor().find.as_ref();
    let input = find.map(|f| f.input.clone()).unwrap_or_default();
    let (idx, total) = app.editor().find_position();
    // the field owns the keys while the bar is open (focused input): a
    // blinking block caret on the app's anim tick (hot while the bar is
    // open), ~600ms on / 500ms off
    let caret_on = app.anim_clock % 1.1 < 0.6;
    let caret = || {
        // 1px — a native input caret's width (the 2px bar read as a blob
        // at the 12px font)
        container(horizontal_space())
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(13.0))
            .style(move |_t| container::Style {
                background: Some(iced::Background::Color(if caret_on {
                    theme::TEXT
                } else {
                    Color::TRANSPARENT
                })),
                ..Default::default()
            })
    };
    // the caret + selection as char offsets (the field is a mini input:
    // ctrl+a/c/x/v and shift/alt+arrows move and select)
    let (caret_pos, sel) = find
        .map(|f| {
            let len = f.input.chars().count();
            let c = f.caret.min(len);
            match f.anchor {
                Some(a) => {
                    let a = a.min(len);
                    (c, Some((a.min(c), a.max(c))))
                }
                None => (c, None),
            }
        })
        .unwrap_or((0, None));
    // the field text is MONO: deterministic char widths, so the caret
    // overlay lands exactly on the boundary. The caret is an OVERLAY —
    // as a row sibling it occupied layout space and the characters
    // shifted every time it moved (the editor's canvas caret is an
    // overlay for the same reason)
    const FIELD_ADVANCE: f32 = 7.2; // 12px mono at the editor's 0.6 ratio
    let mono = |t: &str, color: iced::Color| {
        text(t.to_string())
            .size(12.0)
            .color(color)
            .font(theme::mono())
    };
    let bytes = |ci: usize| {
        input
            .char_indices()
            .nth(ci)
            .map(|(i, _)| i)
            .unwrap_or(input.len())
    };
    let span = |t: &str, color: iced::Color| mono(t, color);
    // layer 0: the text spans (no caret — it overlays above). Fill width:
    // a Shrink layer in a Stack collapses to zero alongside the Fill
    // caret layer (both must fill for the stack to size correctly)
    let text_row: iced::widget::Row<'_, Message> = if input.is_empty() {
        row![mono("find\u{2026}", theme::DIM)]
    } else {
        let (lo, hi) = sel.unwrap_or((caret_pos, caret_pos));
        let mut inner = row![span(&input[..bytes(lo)], theme::TEXT)];
        if hi > lo {
            inner = inner.push(
                container(mono(&input[bytes(lo)..bytes(hi)], theme::TEXT))
                    .style(move |_t| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgba(
                            0.3, 0.5, 0.9, 0.45,
                        ))),
                        ..Default::default()
                    }),
            );
        }
        inner.push(span(&input[bytes(hi)..], theme::TEXT))
    };
    // layer 1: the caret bar, x = the chars before it (the insertion
    // boundary), vertically centered in the field
    let caret_x = caret_pos as f32 * FIELD_ADVANCE;
    let caret_layer = container(caret())
        .width(Fill)
        .height(Fill)
        .align_y(iced::alignment::Vertical::Center)
        .align_x(iced::alignment::Horizontal::Left)
        .padding(iced::Padding {
            left: caret_x,
            ..Default::default()
        });
    // the inset input surface: darker than the bar, hairline border, the
    // query (or the placeholder) with the caret overlaying its boundary —
    // the placeholder is GHOST text with the caret BEFORE it when nothing
    // is typed
    let field = container(
        iced::widget::stack![
            // both layers center in the 22px field so the text baseline
            // matches the caret bar's center
            container(text_row)
                .width(Fill)
                .height(Fill)
                .align_y(iced::alignment::Vertical::Center),
            caret_layer
        ]
        .width(Fill)
        .height(Fill),
    )
        .padding([0, 8])
        .width(Fill)
        .height(Length::Fixed(22.0))
        .align_y(iced::alignment::Vertical::Center)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(theme::BG)),
            border: iced::border::rounded(4).color(theme::BORDER),
            ..Default::default()
        });
    let zone_btn = |label: &'static str, msg: Message| {
        button(text(label.to_string()).size(12.0).color(theme::TEXT))
            .padding([3, 8])
            .style(|_t, s| iced::widget::button::Style {
                background: Some(iced::Background::Color(match s {
                    iced::widget::button::Status::Hovered => {
                        Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                    }
                    _ => Color::TRANSPARENT,
                })),
                border: iced::border::rounded(4),
                ..Default::default()
            })
            .on_press(msg)
    };
    let mut bar = row![field]
        .spacing(6)
        .align_y(iced::alignment::Vertical::Center);
    if total > 0 {
        bar = bar.push(
            text(format!("{idx}/{total}"))
                .size(11.0)
                .color(theme::DIM),
        );
    }
    bar = bar
        .push(zone_btn("<", Message::FindPrev))
        .push(zone_btn(">", Message::FindNext))
        .push(zone_btn("\u{00d7}", Message::FindClose));
    // the bar swallows its own clicks: a press on its surface (field,
    // counter, padding) must not fall through to the editor canvas and
    // move the caret to the clicked line
    let bar: Element<'_, Message> = mouse_area(bar)
        .on_press(Message::PanelSwallowed)
        .into();
    // the bar floats as a card (clearly lighter than the editor bg), its
    // content centered in the 34px strip
    Some(
        container(bar)
            .width(Length::Fixed(300.0))
            .height(Length::Fixed(34.0))
            .padding([0, 6])
            .align_y(iced::alignment::Vertical::Center)
            .style(|_t| container::Style {
                background: Some(iced::Background::Color(theme::TABLE_BG)),
                border: iced::border::rounded(6).color(theme::BORDER),
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: 16.0,
                },
                ..Default::default()
            })
            .into(),
    )
}

/// The vim `/` prompt: a bottom-left command line (vim-style, NOT the gui
/// find popup) — the live query with a caret and the match counter. Keys
/// route through `handle_vim_search_key`.
fn vim_search_overlay(app: &App) -> Option<Element<'_, Message>> {
    let q = app.editor().vim_search.as_ref()?.clone();
    let (idx, total) = app.editor().find_position();
    let mut line = row![
        text("/".to_string()).size(12.0).color(theme::DIM),
        text(q).size(12.0).color(theme::TEXT),
        container(horizontal_space())
            .width(Length::Fixed(1.5))
            .height(Length::Fixed(14.0))
            .style(|_t| container::Style {
                background: Some(iced::Background::Color(theme::TEXT)),
                ..Default::default()
            }),
    ]
    .spacing(3)
    .align_y(iced::alignment::Vertical::Center);
    if total > 0 {
        line = line.push(
            text(format!("{idx}/{total}"))
                .size(11.0)
                .color(theme::DIM),
        );
    }
    // the prompt swallows its clicks too: same pass-through hazard as
    // the find bar (a press would land on the editor underneath)
    let line: Element<'_, Message> = mouse_area(line)
        .on_press(Message::PanelSwallowed)
        .into();
    Some(
        container(line)
            .padding([5, 10])
            .style(|_t| container::Style {
                background: Some(iced::Background::Color(theme::PANEL)),
                border: iced::border::rounded(6).color(theme::BORDER),
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: 16.0,
                },
                ..Default::default()
            })
            .into(),
    )
}

/// The no-document start screen: Open file / New diagram. Shown in place
/// of the editor pane; this state is not collapsible.
/// The start screen (a tab with no document): one full-width, centered
/// welcome view — title, New/Open actions, recent files. No diagram pane
/// and no split: a fresh tab has nothing to show beside it yet.
/// The ANSI-shadow 3D block letters for the word mark: figlet's
/// "ANSI Shadow" face, whose extruded ╚══╝ edges read as depth.
const WORD_ART: &str = concat!(
    "██████╗  ██████╗  ███╗   ███╗ ██╗               ██╗   ██╗ ██╗\n",
    "██╔══██╗ ██╔══██╗ ████╗ ████║ ██║               ██║   ██║ ██║\n",
    "██║  ██║ ██████╔╝ ██╔████╔██║ ██║       ██████╗ ██║   ██║ ██║\n",
    "██║  ██║ ██╔══██╗ ██║╚██╔╝██║ ██║       ╚═════╝ ██║   ██║ ██║\n",
    "██████╔╝ ██████╔╝ ██║ ╚═╝ ██║ ███████╗          ╚██████╔╝ ██║\n",
    "╚═════╝  ╚═════╝  ╚═╝     ╚═╝ ╚══════╝           ╚═════╝  ╚═╝"
);

fn start_screen(_app: &App) -> Element<'static, Message> {
    // the three entry points as vertically stacked cards — icon, title,
    // short description (a card reads as a choice; a bare button doesn't
    // say what it does)
    let card = |icon: &'static [u8],
                title: &'static str,
                desc: &'static str,
                msg: Message| {
        button(
            row![
                iced::widget::svg(iced::widget::svg::Handle::from_memory(icon))
                    .width(Length::Fixed(22.0))
                    .height(Length::Fixed(22.0)),
                column![
                    text(title.to_string())
                        .size(14.0)
                        .color(theme::TEXT)
                        .font(Font {
                            weight: iced::font::Weight::Semibold,
                            ..Font::DEFAULT
                        }),
                    text(desc.to_string()).size(11.5).color(theme::DIM),
                ]
                .spacing(4)
                .width(Fill),
            ]
            .spacing(12)
            .align_y(iced::alignment::Vertical::Center),
        )
        .width(Length::Fixed(360.0))
        .padding([14, 18])
        .style(move |_t, s| iced::widget::button::Style {
            // hover lifts the fill one step lighter than the splash bg —
            // the old hover fill (theme::PANEL) EQUALS the splash bg and
            // the card vanished under the pointer
            background: Some(iced::Background::Color(match s {
                iced::widget::button::Status::Hovered => {
                    Color::from_rgb8(0x33, 0x36, 0x3e)
                }
                _ => Color::from_rgb8(0x2a, 0x2d, 0x35),
            })),
            text_color: theme::TEXT,
            border: iced::border::rounded(8)
                .width(1.0)
                .color(match s {
                    iced::widget::button::Status::Hovered => theme::TEXT,
                    _ => theme::BORDER,
                }),
            ..Default::default()
        })
        .on_press(msg)
    };
    let cards = column![
        card(
            include_bytes!("../icons/file-plus-icon.svg").as_slice(),
            "New diagram",
            "Start from an empty DBML file",
            Message::NewDiagram,
        ),
        card(
            include_bytes!("../icons/folder-open-icon.svg").as_slice(),
            "Open DBML",
            "Continue working on a .dbml diagram",
            Message::OpenFilePicker(PickerKind::Dbml),
        ),
        card(
            include_bytes!("../icons/import-icon.svg").as_slice(),
            "Import SQL schema",
            "Convert a .sql dump into DBML",
            Message::OpenFilePicker(PickerKind::Sql),
        ),
    ]
    .spacing(10);

    let masthead: Element<'static, Message> = text(WORD_ART.to_string())
        .size(13.0)
        .font(theme::mono())
        .color(theme::TEXT)
        .into();

    let caption: Element<'static, Message> = text("DBML DIAGRAM EDITOR".to_string())
        .size(12.0)
        .color(theme::DIM)
        .into();

    let content = column![masthead, caption, cards]
        .spacing(22)
        .align_x(iced::alignment::Horizontal::Center);

    container(container(content).padding([24, 0]))
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(move |_t| panel_style())
        .into()
}

/// The tab-close confirmation, same contract as the exit one but scoped
/// to one tab: Save & close / Close without saving / Cancel.
fn tab_close_overlay(app: &App, idx: usize) -> Element<'static, Message> {
    let title = app
        .tabs
        .get(idx)
        .map(|t| t.title())
        .unwrap_or_else(|| "This tab".to_string());
    confirm_modal(
        Box::leak(format!("\"{title}\" has unsaved changes").into_boxed_str()),
        "Close it anyway?",
        &[
            ("Save & close", Message::TabCloseSave, true),
            ("Close without saving", Message::TabCloseDiscard, false),
            ("Cancel", Message::TabCloseCancel, false),
        ],
        Message::TabCloseCancel,
    )
}

/// Quiet chrome button (toolbar, start screen).
fn tool_button_style() -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: Some(iced::Background::Color(theme::PANEL)),
        text_color: theme::TEXT,
        border: iced::border::rounded(6).color(theme::BORDER),
        ..Default::default()
    }
}

/// A dropdown menu entry inside the export menu: full-width quiet row,
/// 12px label, hover lift.
fn export_menu_item(label: &'static str, msg: Message) -> Element<'static, Message> {
    // the content wraps in a Fill + align-LEFT container: a bare text
    // child in a Fill-width button is CENTERED by iced's flex — the
    // three rows read as differently indented. FIXED height: the dialect
    // card's alignment offset is computed from the rows above it.
    button(
        container(text(label.to_string()).size(12).color(theme::TEXT))
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Center),
    )
    .width(Fill)
    .height(Length::Fixed(28.0))
    .padding([0, 10])
    .style(|_t, s| iced::widget::button::Style {
        background: Some(iced::Background::Color(match s {
            iced::widget::button::Status::Hovered => theme::TOOLTIP,
            _ => Color::TRANSPARENT,
        })),
        text_color: theme::TEXT,
        border: Default::default(),
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

/// Small button in the diagram pane's bottom row (Auto-arrange, Zoom to
/// Fit) — same look and metrics as the editor toolbar's (text 11 +
/// padding [3,8]).
fn diagram_button(
    label: &'static str,
    msg: Message,
    active: bool,
) -> iced::widget::Button<'static, Message> {
    button(text(label).size(11.0).color(if active {
        Color::WHITE
    } else {
        theme::TEXT
    }))
    .padding([3, 8])
    .style(move |_t, s| {
        let mut st = tool_button_style();
        if active {
            // the toggle's on-state: a filled blue — the primary accent
            // (the pane switches read the same when on)
            st.background = Some(iced::Background::Color(match s {
                iced::widget::button::Status::Hovered => {
                    Color { a: 0.85, ..theme::FK }
                }
                _ => theme::FK,
            }));
            st.border = iced::border::rounded(6).color(theme::FK);
        }
        st
    })
    .on_press(msg)
}

/// The invisible full-height strip over the pane seam that starts the
/// split-resize drag (tracked window-wide via the event subscription).
fn edge_overlay(app: &App) -> Element<'_, Message> {
    // below the tab strip: strip 30, panes flush
    const PANES_TOP: f32 = 30.0;
    container(canvas(EdgeHandle).width(Length::Fixed(EDGE_STRIP_W)).height(Fill))
        .width(Fill)
        .height(Fill)
        .padding(iced::Padding {
            top: PANES_TOP,
            right: 0.0,
            bottom: 0.0,
            left: (app.seam_x() - EDGE_STRIP_W / 2.0).max(0.0),
        })
        .into()
}

/// Width of the invisible resize strip hugging the seam (a few px into
/// each pane: the visible edge stays the panes' 1px borders). Presses on
/// the editor's half fall through to the editor canvas — the overlay
/// scrollbar lives in that band, and stealing them made the scrollbar
/// unclickable (dbdiagram's splitter is its own element between the
/// panes for the same reason).
const EDGE_STRIP_W: f32 = 15.0;

/// The seam's resize hit strip. Draws the pane divider: a 2px vertical
/// line at the seam (the two pane borders it used to be), full pane
/// height. Resizing itself has no chrome, just the resize cursor on the
/// diagram half.
struct EdgeHandle;

impl canvas::Program<Message> for EdgeHandle {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // The canvas widget forwards every window event; only presses
        // that land on the strip act. (Ungated, any click in the editor
        // also began a resize, so a click near the window's left edge
        // snapped the split to its minimum.)
        let over = cursor.position_over(bounds);
        if over.is_none() {
            return (canvas::event::Status::Ignored, None);
        }
        if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) =
            event
        {
            let p = over.unwrap_or_default();
            let seam_x = bounds.x + bounds.width / 2.0;
            if p.x < seam_x {
                // editor side of the seam: the overlay scrollbar band (and
                // the text margin beside it) belongs to the editor canvas,
                // which places the caret or starts a scrollbar drag there
                return (canvas::event::Status::Ignored, None);
            }
            return (
                canvas::event::Status::Captured,
                Some(Message::DividerPressed),
            );
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // the divider: 2px centered on the seam (the editor's right border
        // + the diagram's left border it replaces)
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let cx = bounds.width / 2.0;
        frame.fill_rectangle(
            Point::new(cx - 1.0, 0.0),
            Size::new(2.0, bounds.height),
            theme::BORDER,
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        use iced::mouse::Interaction;
        // canvas mouse_interaction is not bounds-gated by iced — without
        // this check the resize cursor leaked over the whole window
        let Some(p) = cursor.position_over(bounds) else {
            return Interaction::default();
        };
        if p.x < bounds.x + bounds.width / 2.0 {
            // editor side: not a resize affordance (the scrollbar lives there)
            Interaction::default()
        } else {
            Interaction::ResizingHorizontally
        }
    }
}

/// The diagram pane's floating maximize button (F2 does the same): a
/// quiet pill in the canvas's top-right corner — just the button, no
/// toolbar bar behind it. Expand icon while split (two arrows leaving the
/// center along the diagonal), restore icon while maximized (two
/// overlapping squares), with the pane-switch accent on the border.
/// The diagram pane's maximize button (F2 in button form): the editor
/// toolbar's expand/contract glyph and quiet chrome — no accent ring —
/// flush with the pane's top-right corner.
fn diagram_max_button(maximized: bool) -> Element<'static, Message> {
    let ink = if maximized { Color::WHITE } else { theme::TEXT };
    let (r, g, b) = (
        (ink.r * 255.0) as u32,
        (ink.g * 255.0) as u32,
        (ink.b * 255.0) as u32,
    );
    let svg = iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 512 512' width='14' height='14'><path d='{}' fill='#{r:02x}{g:02x}{b:02x}'/></svg>",
            if maximized {
                ICON_COMPRESS.0
            } else {
                ICON_EXPAND.0
            },
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(14.0))
    .height(Length::Fixed(14.0));
    mouse_area(
        button(svg)
            .padding([5, 8])
            .style(move |_t, s| {
                let mut st = tool_button_style();
                if maximized {
                    st.background = Some(iced::Background::Color(match s {
                        iced::widget::button::Status::Hovered => {
                            Color { a: 0.85, ..theme::FK }
                        }
                        _ => theme::FK,
                    }));
                    st.border = iced::border::rounded(6).color(theme::FK);
                } else if s == iced::widget::button::Status::Hovered {
                    st.background = Some(iced::Background::Color(theme::TOOLTIP));
                }
                st
            })
            .on_press(Message::ToggleDiagramMaximize),
    )
    .on_enter(Message::ToolbarTip(Some(if maximized {
        "Restore diagram"
    } else {
        "Maximize diagram"
    })))
    .on_exit(Message::ToolbarTip(None))
    .into()
}



/// Invisible listener canvas covering the diagram pane: the frozen
/// `Diagram` widget only forwards LEFT presses, so this stacked twin turns
/// a RIGHT press into `CanvasRightPressed` (canvas-local px) — the entry
/// point of the header-color menu. It draws nothing and captures nothing
/// else, so pan / hover / table drags are untouched.
struct DiagramRightClick;

impl canvas::Program<Message> for DiagramRightClick {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Right)) =
            event
        {
            if let Some(p) = cursor.position_over(bounds) {
                return (
                    canvas::event::Status::Captured,
                    Some(Message::CanvasRightPressed(Point::new(
                        p.x - bounds.x,
                        p.y - bounds.y,
                    ))),
                );
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        _renderer: &iced::Renderer,
        _theme: &iced::Theme,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        Vec::new()
    }
}

fn panel_style() -> container::Style {
    seam_panel_style(Seam::None)
}

/// Which side of a pane is the seam against the other pane: those corners
/// are squared so two flush panes read as one straight vertical edge (the
/// dbdiagram look) instead of rounded corners facing each other.
#[derive(Clone, Copy)]
enum Seam {
    None,
    /// The editor pane's right edge.
    Right,
    /// The diagram pane's left edge.
    Left,
}

fn seam_panel_style(_seam: Seam) -> container::Style {
    // PANEL background, no border: the panes sit flush with the window
    // edges (no perimeter frame), and the editor/diagram divider is drawn
    // by the EdgeHandle strip at the seam instead
    container::Style {
        background: Some(iced::Background::Color(theme::PANEL)),
        border: iced::border::Border {
            radius: iced::border::Radius::from(0.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The toast: a small pill floating over the window's bottom-right corner
/// (transient feedback — save confirmations, clipboard errors, mode
/// switches — that used to share the status bar). Set via
/// `App::set_message`; the tick clears it after `TOAST_TTL` and any
/// keypress supersedes it. Anchored by a full-size container aligned to
/// the right/bottom edges (stack children land at (0,0), so padding alone
/// cannot bottom-right a Shrink child).
/// dbdiagram's problems UI, ported from the live site (dark-mode pixel
/// spec): two floating pills in the editor pane's bottom-right corner —
/// a red "N error(s)" pill, and (only while no errors stand) a blue
/// warning-count pill, both hidden when zero. Clicking a pill pops the
/// 520px diagnostics panel above it; rows carry a severity dot, the
/// message, and a `line:col` position; clicking a row moves the editor
/// caret to that line (panel stays open); Esc / outside click closes.
/// FA chevron-right, lifted from their bundle (the group header's
/// expand chevron; their expanded state rotates it to point down — the
/// down path is the same glyph rotated, inlined here).
const CHEVRON_RIGHT: &str = "M285.476 272.971L91.132 467.314c-9.373 9.373-24.569 9.373-33.941 0l-22.667-22.667c-9.357-9.357-9.375-24.522-.04-33.901L188.505 256 34.484 101.255c-9.335-9.379-9.317-24.544.04-33.901l22.667-22.667c9.373-9.373 24.569-9.373 33.941 0L285.475 239.03c9.373 9.372 9.373 24.568.001 33.941z";
const CHEVRON_DOWN: &str = "M31.3 192h257.3c17.8 0 26.7 21.5 14.1 34.1L174.1 354.8c-7.8 7.8-20.5 7.8-28.3 0L17.2 226.1C4.6 213.5 13.5 192 31.3 192z";


/// FA times-circle (the error pill's icon), lifted from their bundle.
const TIMES_CIRCLE: &str = "M256 8C119 8 8 119 8 256s111 248 248 248 248-111 248-248S393 8 256 8zm121.6 313.1c4.7 4.7 4.7 12.3 0 17L338 377.6c-4.7 4.7-12.3 4.7-17 0L256 312l-65.1 65.6c-4.7 4.7-12.3 4.7-17 0L134.4 338c-4.7-4.7-4.7-12.3 0-17l65.6-65-65.6-65.1c-4.7-4.7-4.7-12.3 0-17l39.6-39.6c4.7-4.7 12.3-4.7 17 0l65 65.7 65.1-65.6c4.7-4.7 12.3-4.7 17 0l39.6 39.6c4.7 4.7 4.7 12.3 0 17L312 256l65.6 65.1z";
/// FA exclamation-triangle (their `triangle-exclamation` warning severity
/// icon; the FA5-solid twin's path, lifted from their bundle). The ⚠ text
/// glyph has no coverage in the UI font and rendered as a question mark.
const WARN_TRIANGLE: &str = "M569.517 440.013C587.975 472.007 564.806 512 527.94 512H48.054c-36.937 0-59.999-40.055-41.577-71.987L246.423 23.985c18.467-32.009 64.72-31.951 82.179 0l239.915 416.028zM288 352c-35.3 0-64 28.7-64 64s28.7 64 64 64 64-28.7 64-64-28.7-64-64-64zm.3-176c-17.7 0-32 14.3-32 32v96c0 17.7 14.3 32 32 32s32-14.3 32-32v-96c0-17.7-14.3-32-32-32z";

/// The warning-triangle glyph in a baked fill (inline SVGs carry no
/// currentColor through iced's renderer).
fn triangle_svg(fill: iced::Color, size: f32) -> Element<'static, Message> {
    let ink = format!(
        "#{:02x}{:02x}{:02x}",
        (fill.r * 255.0) as u8,
        (fill.g * 255.0) as u8,
        (fill.b * 255.0) as u8
    );
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 576 512' width='{size}' height='{size}'><path d='{WARN_TRIANGLE}' fill='{ink}'/></svg>"
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

/// A 12px chevron glyph in the group-header gray (their
/// `w-3 h-3 text-dbx-gray-500`).
fn chevron_svg(down: bool) -> Element<'static, Message> {
    chevron_svg_colored(down, Color::from_rgb8(0x76, 0x7a, 0x97), 10.0)
}


/// The chevron in an arbitrary ink (the fix selector's sits on a mid-gray
/// button, where the default gray-500 disappears).
fn chevron_svg_colored(down: bool, ink: Color, size: f32) -> Element<'static, Message> {
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8
    );
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 320 512' width='{size}' height='{size}'><path d='{}' fill='{hex}'/></svg>",
            if down { CHEVRON_DOWN } else { CHEVRON_RIGHT },
        )
        .into_bytes(),
    ))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

/// The problems panel's inks (live-sampled computed styles from their
/// dark theme): the panel surface family, the gray text scale, the
/// severity dots, and the NOW/FIXED diff accents.
const DIAG_BG: Color = Color { r: 0.133, g: 0.133, b: 0.133, a: 1.0 }; // #222222
/// The NOW/FIXED diff cells: an inset darker than the #222 panel (the
/// editor canvas shares the old panel tone, so the panel surface moved
/// to the record-modal family).
const DIAG_CELL_BG: Color = Color { r: 0.110, g: 0.129, b: 0.157, a: 1.0 }; // #1C2128
const DIAG_DIVIDER: Color = Color { r: 0.184, g: 0.184, b: 0.184, a: 1.0 }; // #2F2F2F
const DIAG_BORDER: Color = Color { r: 0.275, g: 0.298, b: 0.345, a: 1.0 }; // #464C58
const DIAG_FOOTER: Color = Color { r: 0.153, g: 0.165, b: 0.180, a: 1.0 }; // #272A2E
const DIAG_ROW_ACTIVE: Color = Color { r: 0.157, g: 0.157, b: 0.157, a: 1.0 }; // #282828
const DIAG_GRAY_50: Color = Color { r: 0.937, g: 0.941, b: 0.953, a: 1.0 }; // #EFF0F3
const DIAG_GRAY_100: Color = Color { r: 0.835, g: 0.855, b: 0.882, a: 1.0 }; // #D5DAE1
const DIAG_GRAY_200: Color = Color { r: 0.733, g: 0.757, b: 0.800, a: 1.0 }; // #BBC1CC
const DIAG_GRAY_300: Color = Color { r: 0.627, g: 0.659, b: 0.718, a: 1.0 }; // #A0A8B7
const DIAG_GRAY_500: Color = Color { r: 0.463, g: 0.510, b: 0.592, a: 1.0 }; // #768297
const DIAG_GRAY_600: Color = Color { r: 0.404, g: 0.451, b: 0.522, a: 1.0 }; // #677385
const DIAG_GRAY_700: Color = Color { r: 0.337, g: 0.373, b: 0.431, a: 1.0 }; // #565F6E
const DIAG_GRAY_800: Color = Color { r: 0.275, g: 0.298, b: 0.345, a: 1.0 }; // #464C58
const DIAG_GRAY_900: Color = Color { r: 0.196, g: 0.216, b: 0.251, a: 1.0 }; // #323740
const DIAG_RED_DOT: Color = Color { r: 0.980, g: 0.510, b: 0.510, a: 1.0 }; // #FA8282
const DIAG_BLUE_DOT: Color = Color { r: 0.349, g: 0.733, b: 1.0, a: 1.0 }; // #59BBFF
const DIAG_RED_EDGE: Color = Color { r: 0.945, g: 0.471, b: 0.455, a: 1.0 }; // #F17874
const DIAG_GREEN_EDGE: Color = Color { r: 0.247, g: 0.745, b: 0.553, a: 1.0 }; // #3FBE8D
const DIAG_RED_LABEL: Color = Color { r: 0.965, g: 0.647, b: 0.635, a: 1.0 }; // #F6A5A2
const DIAG_GREEN_LABEL: Color = Color { r: 0.541, g: 0.851, b: 0.729, a: 1.0 }; // #8AD9BA
const DIAG_RED_MARK_TEXT: Color = Color { r: 1.0, g: 0.831, b: 0.851, a: 1.0 }; // #FFD4D9
const DIAG_GREEN_MARK_TEXT: Color = Color { r: 0.867, g: 0.953, b: 0.925, a: 1.0 }; // #DDF3EC
const DIAG_RED_MARK_BG: Color = Color { r: 0.933, g: 0.267, b: 0.251, a: 1.0 }; // #EE4440
const DIAG_GREEN_MARK_BG: Color = Color { r: 0.0, g: 0.478, b: 0.294, a: 1.0 }; // #007A4B
const DIAG_APPLY: Color = Color { r: 0.016, g: 0.549, b: 1.0, a: 1.0 }; // #048CFF

/// The hairline between group headers and entry rows (their
/// `border-b border-main-gray-dark`, #2F2F2F).
fn diagnostics_divider() -> Element<'static, Message> {
    container(horizontal_space())
        .width(Fill)
        .height(Length::Fixed(1.0))
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.184, 0.184, 0.184,
            ))),
            ..Default::default()
        })
        .into()
}

/// The toolbar button's hover tooltip: a small label pill anchored just
/// below the pointer, clamped into the window. Rendered at the ROOT
/// layer — the toolbar's own stack is a single row tall, and a pill
/// padded past it gets a zero-area content rect (invisible).
fn toolbar_tip_overlay(app: &App) -> Option<Element<'static, Message>> {
    let label = app.toolbar_tip?;
    let (cx, cy) = app
        .window_cursor
        .map(|p| (p.x, p.y))
        .unwrap_or((0.0, 0.0));
    let x = (cx + 14.0).clamp(2.0, (app.window_size.0 - 160.0).max(2.0));
    let y = (cy + 18.0).clamp(2.0, (app.window_size.1 - 40.0).max(2.0));
    Some(
        container(
            container(text(label.to_string()).size(11.0).color(theme::TEXT))
                .padding([4, 8])
                .style(|_t| container::Style {
                    background: Some(iced::Background::Color(theme::TOOLTIP)),
                    border: iced::border::rounded(5).color(theme::BORDER),
                    ..Default::default()
                }),
        )
        .width(Fill)
        .height(Fill)
        .padding(iced::Padding {
            top: y,
            left: x,
            ..Default::default()
        })
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .into(),
    )
}

fn diagnostics_overlay(app: &App) -> Element<'_, Message> {
    use dbml_ui::dbml::validate::Severity;
    let entries = app.all_diagnostics();
    let errors = entries.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = entries.len() - errors;

    let pill_style = |open: bool, kind: &'static str| {
        move |_t: &iced::Theme, s: iced::widget::button::Status| {
            let (bg, fg) = match (kind, open) {
                // red error pill: #EF4444, hover #DC2626, open #DC2626
                ("error", true) => (iced::Color::from_rgb(0.863, 0.149, 0.149), theme::TEXT),
                ("error", false) => (
                    if s == iced::widget::button::Status::Hovered {
                        iced::Color::from_rgb(0.863, 0.149, 0.149)
                    } else {
                        iced::Color::from_rgb(0.937, 0.267, 0.267)
                    },
                    iced::Color::WHITE,
                ),
                // warning pill: translucent slate + blue count, hover
                // #374151, open solid blue
                (_, true) => (iced::Color::from_rgb(0.231, 0.510, 0.965), iced::Color::WHITE),
                (_, false) => (
                    if s == iced::widget::button::Status::Hovered {
                        iced::Color::from_rgba8(55, 65, 81, 1.0)
                    } else {
                        iced::Color::from_rgba8(31, 41, 55, 0.9)
                    },
                    iced::Color::from_rgb(0.376, 0.647, 0.980),
                ),
            };
            iced::widget::button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: fg,
                border: iced::border::rounded(8),
                ..Default::default()
            }
        }
    };

    // the two pills; each hidden at zero (they are mutually exclusive in
    // practice: warnings are suppressed while errors stand)
    let mut pills = row![].spacing(6);
    if errors > 0 {
        let open = app.diagnostics_open;
        let error_icon = iced::widget::svg(iced::widget::svg::Handle::from_memory(
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 512 512' width='14' height='14'><path d='{TIMES_CIRCLE}' fill='#ffffff'/></svg>"
            )
            .into_bytes(),
        ))
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0));
        pills = pills.push(
            button(
                row![
                    error_icon,
                    text(if errors == 1 { "1 error".to_string() } else { format!("{errors} errors") })
                        .size(13),
                ]
                .spacing(6)
                .align_y(iced::alignment::Vertical::Center),
            )
            .padding([6, 10])
            .style(pill_style(open, "error"))
            .on_press(Message::ToggleDiagnostics),
        );
    }
    if warnings > 0 {
        let open = app.diagnostics_open;
        // their warning severity icon; the count's blue while closed,
        // white on the solid-blue open pill (the error pill's rule)
        let ink = if open {
            iced::Color::WHITE
        } else {
            iced::Color::from_rgb(0.376, 0.647, 0.980)
        };
        pills = pills.push(
            button(
                row![
                    triangle_svg(ink, 14.0),
                    text(warnings.to_string()).size(13),
                ]
                .spacing(6)
                .align_y(iced::alignment::Vertical::Center),
            )
            .padding([6, 10])
            .style(pill_style(open, "warn"))
            .on_press(Message::ToggleDiagnostics),
        );
    }

    if !app.diagnostics_open {
        // bottom inset 8, right inset 20 (their mb-2 mr-2)
        return container(pills)
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 20.0,
                bottom: 8.0,
                left: 0.0,
            })
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .into();
    }

    // the panel: 520 wide, a FIXED height — dbdiagram's problems
    // container does not shrink with the viewport (the list scrolls
    // inside, the frame never grows and never resizes under the mouse)
    const PANEL_H: f32 = 560.0;
    let panel_h = PANEL_H;
    let title = if errors > 0 { "Errors" } else { "Warnings" };

    // group the entries by category ("Other" when uncategorized), headers
    // collapsible with a count
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, d) in entries.iter().enumerate() {
        let key = d.category.clone().unwrap_or_else(|| "Other".to_string());
        match groups.iter_mut().find(|(g, _)| *g == key) {
            Some((_, items)) => items.push(i),
            None => groups.push((key, vec![i])),
        }
    }

    let src = app.source();
    let mut rows: Vec<Element<'_, Message>> = Vec::new();
    for (name, items) in &groups {
        let collapsed = app.diagnostics_collapsed.contains(name);
        rows.push(
            button(
                row![
                    chevron_svg(!collapsed),
                    text(name.to_uppercase())
                        .size(11)
                        .color(DIAG_GRAY_500),
                    text(format!("{}", items.len()))
                        .size(11)
                        .color(DIAG_GRAY_700),
                ]
                .spacing(6)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Fill)
            .padding([7, 12])
            .style(|_t, _s| iced::widget::button::Style {
                background: Some(iced::Background::Color(DIAG_BG)),
                text_color: DIAG_GRAY_500,
                border: Default::default(),
                ..Default::default()
            })
            .on_press(Message::ToggleDiagnosticsGroup(name.clone()))
            .into(),
        );
        // the group header's border-b
        rows.push(diagnostics_divider());
        if !collapsed {
            for i in items {
                let d = &entries[*i];
                let dot = match d.severity {
                    Severity::Error => DIAG_RED_DOT,
                    Severity::Warning => DIAG_BLUE_DOT,
                };
                let (line, col) = dbml_ui::dbml::parser::line_col_at(src, d.offset);
                let expanded = app.diagnostics_expanded == Some(*i);
                let row_bg = if expanded {
                    DIAG_ROW_ACTIVE
                } else {
                    Color::TRANSPARENT
                };
                // the 6px round severity dot, dropped 8px so it lines up
                // with the message's first text line (their mt-[8px]
                // items-start row)
                let dot_el = container(container(horizontal_space())
                        .width(Length::Fixed(6.0))
                        .height(Length::Fixed(6.0))
                        .style(move |_t| container::Style {
                            background: Some(iced::Background::Color(dot)),
                            border: iced::border::rounded(3),
                            ..Default::default()
                        }))
                    .padding(iced::Padding { top: 8.0, ..Default::default() });
                // the message: backticked tokens render as the inline code
                // chips, flow-wrapped to the row's width budget
                let panel_w = 520.0f32.min(app.seam_x().max(220.0));
                let msg_budget = panel_w - 24.0 - 6.0 - 20.0 - 46.0;
                let msg_rows: Vec<Element<'_, Message>> = message_rows(&d.message, msg_budget)
                    .iter()
                    .map(|segs| message_row(segs))
                    .collect();
                let msg_col: Element<'_, Message> = column(msg_rows).spacing(1).into();
                rows.push(
                    button(
                        row![
                            dot_el,
                            container(msg_col).width(Fill),
                            container(
                                text(format!("{}:{}", line + 1, col + 1))
                                    .size(12)
                                    .color(DIAG_GRAY_700)
                                    .font(theme::mono()),
                            )
                            .padding(iced::Padding { top: 3.0, ..Default::default() }),
                        ]
                        .spacing(10)
                        .align_y(iced::alignment::Vertical::Top),
                    )
                    .width(Fill)
                    .padding([11, 12])
                    .style(move |_t, s| iced::widget::button::Style {
                        background: Some(iced::Background::Color(
                            if s == iced::widget::button::Status::Hovered {
                                DIAG_ROW_ACTIVE
                            } else {
                                row_bg
                            },
                        )),
                        text_color: theme::TEXT,
                        border: Default::default(),
                        ..Default::default()
                    })
                    .on_press(Message::DiagnosticGoto(*i))
                    .into(),
                );
                // the entry row's border-b
                rows.push(diagnostics_divider());
                // the accordion: the explanation bullets, the fix selector
                // + Apply fix, and the NOW/FIXED diff (one row expanded at
                // a time; the selected fix drives the diff)
                if expanded && (!d.explanation.is_empty() || !d.quick_fixes.is_empty()) {
                    let mut body = column![].spacing(12);
                    if !d.explanation.is_empty() {
                        let mut bullets = column![].spacing(4);
                        for b in &d.explanation {
                            bullets = bullets.push(
                                text(format!("\u{2022} {b}"))
                                    .size(13)
                                    .color(DIAG_GRAY_300),
                            );
                        }
                        body = body.push(
                            container(bullets)
                                .padding(iced::Padding { left: 16.0, ..Default::default() }),
                        );
                    }
                    if !d.quick_fixes.is_empty() {
                        // the fix selector: a dropdown when several fixes
                        // exist (the label shows the pick), the blue Apply
                        // beside it — and the inline menu when open
                        let selected = app
                            .diagnostics_selected_fix
                            .min(d.quick_fixes.len() - 1);
                        let mut fix_col = column![].spacing(6);
                        let mut fix_row = row![].spacing(8).align_y(iced::alignment::Vertical::Center);
                        if !d.quick_fixes.is_empty() {
                            fix_row = fix_row.push(
                                button(
                                    container(
                                        row![
                                            text(d.quick_fixes[selected].short_title.clone())
                                                .size(12)
                                                .color(DIAG_GRAY_200),
                                            chevron_svg_colored(true, DIAG_GRAY_100, 10.0),
                                        ]
                                        .spacing(6)
                                        .align_y(iced::alignment::Vertical::Center),
                                    )
                                    .height(Fill)
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding([0, 10])
                                .height(Length::Fixed(32.0))
                                .style(move |_t, s| iced::widget::button::Style {
                                    background: Some(iced::Background::Color(match s {
                                        // their hover:brightness-110: one step up
                                        iced::widget::button::Status::Hovered => {
                                            Color { a: 1.0, ..DIAG_GRAY_700 }
                                        }
                                        _ => DIAG_GRAY_800,
                                    })),
                                    text_color: DIAG_GRAY_200,
                                    border: iced::border::rounded(4).color(DIAG_GRAY_600),
                                    ..Default::default()
                                })
                                .on_press(Message::DiagnosticToggleFixMenu),
                            );
                        }
                        fix_row = fix_row.push(
                            button(
                                container(text("Apply fix".to_string()).size(12).color(Color::WHITE))
                                    .height(Fill)
                                    .align_y(iced::alignment::Vertical::Center),
                            )
                            .padding([0, 12])
                            .height(Length::Fixed(32.0))
                                .style(move |_t, s| iced::widget::button::Style {
                                    background: Some(iced::Background::Color(match s {
                                        iced::widget::button::Status::Hovered => {
                                            Color { a: 1.0, ..DIAG_APPLY }
                                        }
                                        _ => DIAG_APPLY,
                                    })),
                                    text_color: Color::WHITE,
                                    border: iced::border::rounded(4),
                                    ..Default::default()
                                })
                                .on_press(Message::ApplyDiagnosticFix(*i, selected)),
                        );
                        // the NOW/FIXED pair for the SELECTED fix: the
                        // line holding the fix's span, as-is vs rewritten,
                        // the changed token marked in each
                        let mut diff_part: Option<Element<'_, Message>> = None;
                        if let Some(fix) = d.quick_fixes.get(selected) {
                            let fix_line = src[..fix.span.start].matches('\n').count();
                            if let Some(now) = src.lines().nth(fix_line) {
                                let line_start = line_col_of(src, fix_line);
                                let s = fix
                                    .span
                                    .start
                                    .saturating_sub(line_start)
                                    .min(now.len());
                                let e = fix
                                    .span
                                    .end
                                    .saturating_sub(line_start)
                                    .min(now.len())
                                    .max(s);
                                let mut before = now[..s].to_string();
                                let marked = now[s..e].to_string();
                                let mut after = now[e.min(now.len())..].to_string();
                                trim_marked_line(&mut before, &marked, &mut after);
                                // the cell's text width in px: half the
                                // panel (minus the grid gap) minus bezel,
                                // frame and the cell's own padding — the
                                // 13px mono grid is 8.1px/char
                                let cell_w = ((panel_w - 8.0) / 2.0 - 4.0 - 20.0).max(31.2);
                                if std::env::var("DBML_UI_DIFF_DEBUG").is_ok() {
                                    eprintln!(
                                        "[diff] marked={marked:?} cell_w={cell_w} rows={:?}",
                                        code_marked_rows(&before, &marked, &after, cell_w)
                                    );
                                }
                                diff_part = Some(
                                    row![
                                        diff_cell(
                                            "NOW",
                                            DIAG_RED_LABEL,
                                            DIAG_RED_EDGE,
                                            Color { a: 0.28, ..DIAG_RED_MARK_BG },
                                            DIAG_RED_MARK_TEXT,
                                            before.clone(),
                                            marked,
                                            after.clone(),
                                            cell_w,
                                        ),
                                        {
                                            let mut fb = before.clone();
                                            let fm = fix.new_text.clone();
                                            let mut fa = after.clone();
                                            trim_marked_line(&mut fb, &fm, &mut fa);
                                            diff_cell(
                                                "FIXED",
                                                DIAG_GREEN_LABEL,
                                                DIAG_GREEN_EDGE,
                                                Color { a: 0.32, ..DIAG_GREEN_MARK_BG },
                                                DIAG_GREEN_MARK_TEXT,
                                                fb,
                                                fm,
                                                fa,
                                                cell_w,
                                            )
                                        },
                                    ]
                                    .spacing(8)
                                    .into(),
                                );
                            }
                        }
                        // the selector and the diff share a stack: the
                        // open menu FLOATS over the diff (no push-down),
                        // fading in over ~120ms (their dropdown)
                        let mut fix_and_diff = column![fix_row].spacing(12);
                        if let Some(diff) = diff_part {
                            fix_and_diff = fix_and_diff.push(diff);
                        }
                        let mut fix_stack_layers: Vec<Element<'_, Message>> =
                            vec![fix_and_diff.into()];
                        if app.diagnostics_fix_menu && !d.quick_fixes.is_empty() {
                            let fade = app
                                .diagnostics_fix_menu_since
                                .map(|t| (t.elapsed().as_millis() as f32 / 120.0).min(1.0))
                                .unwrap_or(1.0);
                            let mut menu = column![].spacing(0);
                            for (f, fix) in d.quick_fixes.iter().enumerate() {
                                let selected = f == selected;
                                menu = menu.push(
                                    button(
                                        container(
                                            text(fix.short_title.clone())
                                                .size(12)
                                                .color(if selected {
                                                    Color::WHITE
                                                } else {
                                                    DIAG_GRAY_200
                                                }),
                                        )
                                        .align_y(iced::alignment::Vertical::Center),
                                    )
                                    .width(Fill)
                                    .padding([6, 10])
                                    .style(move |_t, s| iced::widget::button::Style {
                                        background: Some(iced::Background::Color(match s {
                                            // the hover lifts harder than the
                                            // selected mark: the selection is a
                                            // quiet state, the hover a response
                                            iced::widget::button::Status::Hovered => DIAG_GRAY_600,
                                            _ if selected => DIAG_GRAY_800,
                                            _ => DIAG_GRAY_900,
                                        })),
                                        text_color: DIAG_GRAY_200,
                                        border: iced::border::rounded(8),
                                        ..Default::default()
                                    })
                                    .on_press(Message::DiagnosticSelectFix(f)),
                                );
                            }
                            // the menu floats from the selector's bottom
                            // edge down, its alpha riding the fade
                            fix_stack_layers.push(
                                container(
                                    container(menu)
                                        .width(Length::Fixed(190.0))
                                        .style(move |_t| container::Style {
                                            background: Some(iced::Background::Color(Color {
                                                a: fade,
                                                ..DIAG_GRAY_900
                                            })),
                                            border: iced::border::rounded(12).color(DIAG_BORDER),
                                            shadow: iced::Shadow {
                                                color: Color::from_rgba(0.0, 0.0, 0.0, 0.35 * fade),
                                                offset: iced::Vector::new(0.0, 4.0),
                                                blur_radius: 10.0,
                                            },
                                            ..Default::default()
                                        }),
                                )
                                .width(Fill)
                                .height(Fill)
                                .align_x(iced::alignment::Horizontal::Left)
                                .align_y(iced::alignment::Vertical::Top)
                                .padding(iced::Padding { top: 32.0, ..Default::default() })
                                .into(),
                            );
                        }
                        fix_col = fix_col.push(iced::widget::Stack::with_children(fix_stack_layers));
                        body = body.push(fix_col);
                    }
                    // the expanded card is a click backdrop: a press
                    // anywhere in it outside the floating fix menu closes
                    // the menu (dbdiagram's autoHide) — the menu's own
                    // buttons sit on the stack layer above and win
                    let body: Element<'_, Message> = mouse_area(body)
                        .on_press(Message::DiagnosticDismissFixMenu)
                        .into();
                    rows.push(
                        container(body)
                            .width(Fill)
                            .padding(iced::Padding { top: 8.0, right: 12.0, bottom: 14.0, left: 28.0 })
                            .style(|_t| container::Style {
                                background: Some(iced::Background::Color(DIAG_ROW_ACTIVE)),
                                ..Default::default()
                            })
                            .into(),
                    );
                }
                rows.push(diagnostics_divider());
            }
        }
    }

    let body: Element<'_, Message> = if rows.is_empty() {
        text("No problems in this file.")
            .size(13)
            .color(DIAG_GRAY_300)
            .into()
    } else {
        // the header band: the severity title + "N in this file", closed
        // by its border-b (a divider element — iced borders have no
        // per-side widths)
        let header = column![
            container(
                row![
                    text(title).size(13).color(Color::WHITE),
                    text(format!("{} in this file", entries.len()))
                        .size(12)
                        .color(DIAG_GRAY_300),
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Fill)
            .padding([10, 12]),
            diagnostics_divider(),
        ];
        let list = container(
            scrollable(column(rows).spacing(0)).direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    // their problems-list hides the scrollbar entirely
                    // (wheel scroll still works)
                    .width(0)
                    .scroller_width(0)
                    .margin(0),
            )),
        )
        .width(Fill)
        .height(Fill);
        // the 24px footer band, closed by its border-t
        let footer = column![
            container(horizontal_space())
                .width(Fill)
                .height(Length::Fixed(23.0))
                .style(|_t| container::Style {
                    background: Some(iced::Background::Color(DIAG_FOOTER)),
                    ..Default::default()
                }),
            diagnostics_divider(),
        ];
        column![header, list, footer]
            .width(Fill)
            .height(Fill)
            .into()
    };

    // the panel's capture layer: a press anywhere inside the panel that
    // misses a button lands HERE instead of the backdrop below — the list
    // never closes on its own text (their floating panel behaves the same)
    let panel = container(stack![
        button(text(""))
            .width(Fill)
            .height(Fill)
            .style(|_t, _s| iced::widget::button::Style {
                background: None,
                ..Default::default()
            })
            .on_press(Message::PanelSwallowed),
        body,
    ])
    .width(Length::Fixed(520.0f32.min(app.seam_x().max(220.0))))
    .height(Length::Fixed(panel_h))
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(DIAG_BG)),
        border: iced::border::rounded(8).color(DIAG_BORDER),
        // their panel-container box-shadow
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 30.0,
        },
        ..Default::default()
    });

    let content = column![panel, pills]
        .spacing(4)
        .align_x(iced::alignment::Horizontal::Right);

    // click anywhere outside dismisses (floating-vue's close-on-outside;
    // our backdrop consumes the click, theirs lets it through)
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::ToggleDiagnostics);
    stack![
        backdrop,
        container(content)
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 20.0,
                bottom: 8.0,
                left: 0.0,
            })
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom),
    ]
    .into()
}

/// Byte offset of a 0-based line's start (for slicing spans within it).
fn line_col_of(src: &str, line: usize) -> usize {
    src.split('\n')
        .take(line)
        .map(|l| l.len() + 1)
        .sum()
}

/// The message flow-wrapped into rows of segments: a Row can't wrap its
/// children, so the plain segments are split into (space-prefixed) words
/// and greedily packed against the pixel budget — chips stay whole and
/// break to their own line when they don't fit, like their message flow.
fn message_rows(msg: &str, budget: f32) -> Vec<Vec<(String, bool)>> {
    let width = |t: &str, code: bool| -> f32 {
        if code {
            t.chars().count() as f32 * 7.3 + 12.0
        } else {
            t.chars().count() as f32 * 6.3
        }
    };
    let mut tokens: Vec<(String, bool)> = Vec::new();
    for (i, seg) in msg.split('`').enumerate() {
        if seg.is_empty() {
            continue;
        }
        if i % 2 == 1 {
            tokens.push((seg.to_string(), true));
        } else {
            for word in seg.split(' ') {
                if !word.is_empty() {
                    tokens.push((format!(" {word}"), false));
                }
            }
        }
    }
    let mut rows: Vec<Vec<(String, bool)>> = vec![Vec::new()];
    let mut used = 0.0f32;
    for (text, code) in tokens {
        let w = width(&text, code);
        let (text, w) = if used + w > budget && used > 0.0 && !code && text.starts_with(' ') {
            // a fresh line drops the word's leading space
            (text[1..].to_string(), w - 6.3)
        } else {
            (text, w)
        };
        if used + w > budget && used > 0.0 {
            rows.push(Vec::new());
            used = 0.0;
        }
        rows.last_mut().unwrap().push((text, code));
        used += w;
    }
    rows
}

/// One wrapped message row: plain segments as white text, code segments
/// as the inline chips.
fn message_row(segments: &[(String, bool)]) -> Element<'static, Message> {
    let mut r = row![].spacing(2).align_y(iced::alignment::Vertical::Center);
    for (seg, is_code) in segments {
        r = if *is_code {
            r.push(code_chip(seg))
        } else {
            r.push(text(seg.clone()).size(13).color(Color::WHITE))
        };
    }
    r.into()
}

/// An inline code chip: the backticked token's pill (gray-900 body, mono
/// 12px, 1px 5px pad, radius 4).
fn code_chip(seg: &str) -> Element<'static, Message> {
    container(text(seg.to_string()).size(12).color(DIAG_GRAY_50).font(theme::mono()))
        .padding(iced::Padding {
            top: 1.0,
            right: 5.0,
            bottom: 1.0,
            left: 5.0,
        })
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(DIAG_GRAY_900)),
            border: iced::border::rounded(4),
            ..Default::default()
        })
        .into()
}

/// The composed diff line wrapped for one cell — CSS
/// `white-space: pre-wrap; overflow-wrap: break-word` semantics, like
/// their `pre`: lines break at SPACES and the wrap-space is consumed BY
/// the break (rows never start or end in it), while a word is broken
/// mid-word only when it ALONE is wider than the line. Segments carry
/// their marked flag (the diff tint — a mark may span a break), and the
/// mark's pill padding (3px each side, once per contiguous marked run
/// per row) counts against the line's pixel width.
fn code_marked_rows(
    before: &str,
    marked: &str,
    after: &str,
    width_px: f32,
) -> Vec<Vec<(String, bool)>> {
    // the 13px mono grid's REAL advance (DejaVu-style mono measures
    // 8.09px here — a 7.8 estimate under-counted, the wrapped rows
    // overflowed the cell and the flex squeezed the mark pill to a zero
    // width: the deleted `>` vanished while its row still rendered)
    const CHAR_PX: f32 = 8.1;
    const PILL_PX: f32 = 6.0; // the mark's 3px + 3px

    // pixel width of `segs` appended to a line whose last segment is
    // marked (`pill_open`): a marked segment opens a pill unless it
    // directly follows marked content — a marked SPACE keeps the pill
    // open, so `not` + its space + `null` render as ONE pill
    fn segs_px(segs: &[(String, bool)], pill_open: bool) -> f32 {
        let mut px = 0.0;
        let mut open = pill_open;
        for (t, m) in segs {
            px += t.chars().count() as f32 * CHAR_PX;
            if *m && !open {
                px += PILL_PX;
            }
            open = *m;
        }
        px
    }

    // append `segs` to `row`, merging into its trailing segment when the
    // flag matches — one segment is one text run / one pill
    fn push_segs(row: &mut Vec<(String, bool)>, segs: &[(String, bool)]) {
        for (t, m) in segs {
            match row.last_mut() {
                Some((tt, tm)) if *tm == *m => tt.push_str(t),
                _ => row.push((t.clone(), *m)),
            }
        }
    }

    // close the current row (a no-op on an empty one)
    fn close(rows: &mut Vec<Vec<(String, bool)>>, cur: &mut Vec<(String, bool)>, cur_px: &mut f32) {
        if !cur.is_empty() {
            rows.push(std::mem::take(cur));
            *cur_px = 0.0;
        }
    }

    // one pixel of fit slack: CHAR_PX is the advance rounded UP to a
    // tenth (real ~8.09 — ~0.3px over a 28-char line) and the pill's
    // paddings land sub-pixel, so a line their browser fits (28 chars +
    // one pill at 232px) must not wrap a word early
    const FIT_TOL: f32 = 1.0;

    // the char stream with its flag, cut into alternating ATOMS: maximal
    // space runs and maximal words. Both carry flags — the mark can hold
    // spaces (`not null`) — and a word can mix flags (`[null,` is plain
    // `[` + marked `null` + plain `,`), each same-flag stretch its own
    // segment
    let chars = before
        .chars()
        .map(|c| (c, false))
        .chain(marked.chars().map(|c| (c, true)))
        .chain(after.chars().map(|c| (c, false)));
    let mut atoms: Vec<(bool, Vec<(String, bool)>)> = Vec::new(); // (is_space, segs)
    for (c, m) in chars {
        let sp = c.is_whitespace();
        match atoms.last_mut() {
            // same atom class (space / word): extend its last segment on
            // a flag match, else open a new segment inside it — a word
            // is NEVER cut at its flag changes (only at spaces)
            Some((s, segs)) if *s == sp => match segs.last_mut() {
                Some((tt, tm)) if *tm == m => tt.push(c),
                _ => segs.push((c.to_string(), m)),
            },
            _ => atoms.push((sp, vec![(c.to_string(), m)])),
        }
    }

    let mut rows: Vec<Vec<(String, bool)>> = Vec::new();
    let mut cur: Vec<(String, bool)> = Vec::new();
    let mut cur_px = 0.0;
    // spaces met since the last word: committed only once the next word
    // provably fits behind them, otherwise dropped at the wrap — CSS
    // consumes the whitespace at a soft break (a marked space in here
    // dies with it: the break itself renders that whitespace)
    let mut pending: Vec<(String, bool)> = Vec::new();
    for (is_space, segs) in &atoms {
        if *is_space {
            // spaces are never hard-broken; a run wider than the line
            // dies whole at the next wrap (only pathological input)
            push_segs(&mut pending, segs);
            continue;
        }
        let open = matches!(cur.last(), Some((_, true)));
        let open_after_pending = pending.last().map(|(_, m)| *m).unwrap_or(open);
        if cur_px + segs_px(&pending, open) + segs_px(segs, open_after_pending)
            <= width_px + FIT_TOL
        {
            // the word fits behind its spaces: commit both
            push_segs(&mut cur, &pending);
            push_segs(&mut cur, segs);
            pending.clear();
            cur_px = segs_px(&cur, false);
        } else {
            // wrap BEFORE the word
            pending.clear();
            close(&mut rows, &mut cur, &mut cur_px);
            let fresh = segs_px(segs, false);
            if fresh <= width_px + FIT_TOL {
                push_segs(&mut cur, segs);
                cur_px = fresh;
            } else {
                // break-words: the word ALONE is wider than the line —
                // hard-break it at the width. Pill metering stays
                // conservative: each contiguous marked fragment ON A ROW
                // pays PILL_PX once, so a mark split across rows pays
                // twice (renders a touch wider than CSS would, never
                // narrower — this can only wrap earlier, not overflow)
                for (t, m) in segs {
                    let mut remaining = t.as_str();
                    while !remaining.is_empty() {
                        let open = matches!(cur.last(), Some((_, true)));
                        let pill = if *m && !open { PILL_PX } else { 0.0 };
                        let take = (((width_px - cur_px - pill) / CHAR_PX).floor() as usize)
                            .clamp(1, remaining.chars().count());
                        let cut = remaining
                            .char_indices()
                            .nth(take)
                            .map(|(i, _)| i)
                            .unwrap_or(remaining.len());
                        let (chunk, rest) = remaining.split_at(cut);
                        match cur.last_mut() {
                            Some((tt, tm)) if *tm == *m => tt.push_str(chunk),
                            _ => cur.push((chunk.to_string(), *m)),
                        }
                        cur_px += chunk.chars().count() as f32 * CHAR_PX + pill;
                        remaining = rest;
                        if !remaining.is_empty() {
                            close(&mut rows, &mut cur, &mut cur_px);
                        }
                    }
                }
            }
        }
    }
    // trailing spaces survive only if they still fit (pre-wrap preserves
    // them; the caller trims the line's real ends before this anyway)
    let open = matches!(cur.last(), Some((_, true)));
    if cur_px + segs_px(&pending, open) <= width_px + FIT_TOL {
        push_segs(&mut cur, &pending);
    }
    close(&mut rows, &mut cur, &mut cur_px);
    if rows.is_empty() {
        rows.push(Vec::new()); // empty input still lays out one (empty) row
    }
    rows
}

#[cfg(test)]
mod diff_tests {
    /// The deleted operator must appear marked in the NOW cell: the
    /// regression — the `Change to >?` fix rendered NOW with the `>`
    /// simply gone.
    #[test]
    fn the_operator_fix_marks_the_deleted_char() {
        // the real fixture: the synthetic minimal source passed while the
        // showcase file dropped the `>` in the NOW cell
        for src in [
            "Table users {\n  id integer [pk]\n  referrer_id integer [null]\n}\nRef: users.referrer_id > users.id\n"
                .to_string(),
            std::fs::read_to_string("showcase.dbml").unwrap_or_default(),
        ] {
            if src.is_empty() {
                continue;
            }
            let app = crate::app::App::with_source(src.clone());
            let diags = app.all_diagnostics();
            let fixes: Vec<_> = diags
                .iter()
                .flat_map(|d| d.quick_fixes.iter())
                .filter(|f| f.short_title.starts_with("Change to"))
                .collect();
            assert!(!fixes.is_empty(), "the operator fix exists");
            for fix in &fixes {
                let fix_line = src[..fix.span.start].matches('\n').count();
                let now = src.lines().nth(fix_line).unwrap();
                let line_start = super::line_col_of(&src, fix_line);
                let s = fix.span.start.saturating_sub(line_start).min(now.len());
                let e = fix.span.end.saturating_sub(line_start).min(now.len()).max(s);
                let marked = &now[s..e];
                assert_eq!(
                    marked, ">",
                    "the deleted operator is the mark (fix at {}..{}, line {fix_line})",
                    fix.span.start, fix.span.end
                );
            }
        }
    }
}

#[cfg(test)]
mod wrap_tests {
    use super::code_marked_rows;

    /// each row's segments joined back into their rendered text
    fn rows_text(rows: &[Vec<(String, bool)>]) -> Vec<String> {
        rows.iter()
            .map(|r| r.iter().map(|(t, _)| t.as_str()).collect())
            .collect()
    }

    /// the wrap must read like CSS: no row opens with wrap-space or a
    /// leading comma, and `'Self-reference:` (129.6px — far under any
    /// fixture width) is never broken mid-word
    fn assert_wrap_hygiene(rows: &[Vec<(String, bool)>]) {
        let flat = rows_text(rows);
        for r in &flat {
            assert!(
                !r.starts_with(' ') && !r.starts_with(','),
                "a wrap leaked its space / comma: {flat:?}"
            );
            assert!(
                !r.ends_with("Self-") && !r.starts_with("reference"),
                "`Self-reference:` split mid-word: {flat:?}"
            );
        }
    }

    #[test]
    fn the_diff_wraps_at_word_boundaries() {
        // pre-wrap + break-words: lines break at SPACES, whole — the
        // mark's word `[null,` misses the fit behind `integer`
        // (153.9 + 8.1 + 54.6 > 184.2) and moves down WHOLE: a word that
        // fits on a line of its own is never split
        let rows = code_marked_rows(
            "referrer_id integer [",
            "null,",
            " note: 'x']",
            22.0 * 8.1 + 6.0, // 22 chars + one pill
        );
        let flat = rows_text(&rows);
        assert_eq!(flat, vec!["referrer_id integer", "[null, note: 'x']"]);
        // every row respects the budget (chars + pill allowance)
        for r in &rows {
            let px: f32 = r
                .iter()
                .map(|(t, m)| t.chars().count() as f32 * 8.1 + if *m { 6.0 } else { 0.0 })
                .sum();
            assert!(px <= 22.0 * 8.1 + 6.0 + 1.0, "{px} over: {r:?}");
        }
    }

    #[test]
    fn the_mark_never_leaks_across_rows_uncaned() {
        // the regression from the screenshot: a fixed char wrap put the
        // mark's first char at the end of the previous line as PLAIN
        // text. Every char keeps its flag where it lands.
        let rows = code_marked_rows("a [", "not null", ", note: 'x']", 12.0 * 8.1 + 6.0);
        let marked_text: String = rows
            .iter()
            .flat_map(|r| r.iter())
            .filter(|(_, m)| *m)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(marked_text, "not null", "the mark survives wrapping");
    }

    #[test]
    fn the_one_char_mark_survives_the_line_break() {
        // the regression: `Change to >?` on `Ref: users.referrer_id > users.id`
        // rendered NOW with the `>` GONE — the 1-char marked token broke to
        // a new line and vanished
        let rows = code_marked_rows(
            "Ref: users.referrer_id ",
            ">",
            " users.id",
            (520.0 - 8.0) / 2.0 - 4.0 - 20.0,
        );
        let joined: String = rows
            .iter()
            .flat_map(|r| r.iter())
            .map(|(t, _)| t.as_str())
            .collect();
        assert!(joined.contains('>'), "the > survives: {rows:?}");
        let marked: String = rows
            .iter()
            .flat_map(|r| r.iter())
            .filter(|(_, m)| *m)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(marked, ">", "exactly the > is marked: {rows:?}");
    }

    #[test]
    fn a_long_word_breaks_mid_word() {
        // break-words: a single word wider than the line still breaks
        let rows = code_marked_rows("", "a_very_long_identifier_here", "", 10.0 * 8.1 + 6.0);
        assert!(rows.len() >= 3, "the word hard-broke: {rows:?}");
        let joined: String = rows
            .iter()
            .flat_map(|r| r.iter().map(|(t, _)| t.as_str()))
            .collect();
        assert_eq!(joined, "a_very_long_identifier_here");
    }

    #[test]
    fn the_showcase_now_line_wraps_like_dbdiagram() {
        // acceptance fixture, off their screenshot: showcase.dbml's
        // referrer_id line in the NOW cell — panel 520 gives the cell
        // (520-8)/2 - 4 - 20 = 232px
        let rows = code_marked_rows(
            "referrer_id integer [",
            "null",
            ", note: 'Self-reference: who invited this user']",
            232.0,
        );
        let flat = rows_text(&rows);
        assert_eq!(
            flat,
            vec![
                "referrer_id integer [null,",
                "note: 'Self-reference: who",
                "invited this user']",
            ]
        );
        assert_wrap_hygiene(&rows);
    }

    #[test]
    fn the_showcase_fixed_line_wraps_like_dbdiagram() {
        // the FIXED twin: the mark wraps at the space after `not` —
        // ` null,` misses 232px even with the pill continuing across the
        // marked space — exactly their screenshot's break
        let rows = code_marked_rows(
            "referrer_id integer [",
            "not null",
            ", note: 'Self-reference: who invited this user']",
            232.0,
        );
        let flat = rows_text(&rows);
        assert_eq!(
            flat,
            vec![
                "referrer_id integer [not",
                "null, note: 'Self-reference:",
                "who invited this user']",
            ]
        );
        assert_wrap_hygiene(&rows);
        // the mark's own space died with the break; the halves stay
        // marked and reunite as `notnull`
        let marked: String = rows
            .iter()
            .flat_map(|r| r.iter())
            .filter(|(_, m)| *m)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(marked, "notnull");
    }
}

/// dbdiagram's `.trim()` on the composed diff line: the leading
/// whitespace of the line's before-part and the trailing whitespace of
/// the after-part never render (theirs trims the concatenated HTML).
/// With an empty mark the strip continues into the opposite part, the
/// same way `String::trim` walks a single string.
fn trim_marked_line(before: &mut String, marked: &str, after: &mut String) {
    *before = before.trim_start().to_string();
    if before.is_empty() && marked.is_empty() {
        *after = after.trim_start().to_string();
    }
    *after = after.trim_end().to_string();
    if after.is_empty() && marked.is_empty() {
        *before = before.trim_end().to_string();
    }
}

/// One NOW/FIXED cell, replicated from their DiffPreview: a 1px
/// #2F2F2F frame with the LEFT side a 3px accent bezel (#F17874 red /
/// #3FBE8D green), rounded 6 all around — the accent reaches the edge
/// (the old inset bar left a dark line between bezel and edge) — then
/// the uppercase label over the wrapped line with the changed token
/// marked (the diff tint).
#[allow(clippy::too_many_arguments)]
fn diff_cell(
    label: &'static str,
    label_color: iced::Color,
    edge: iced::Color,
    mark_bg: iced::Color,
    mark_text: iced::Color,
    before: String,
    marked: String,
    after: String,
    width_px: f32,
) -> Element<'static, Message> {
    let mut code_col: Vec<Element<'static, Message>> = Vec::new();
    for segs in code_marked_rows(&before, &marked, &after, width_px) {
        let mut r = row![].spacing(0);
        for (t, marked) in segs {
            let pill_w = t.chars().count() as f32 * 8.1 + 6.0;
            r = if marked {
                r.push(
                    container(text(t).size(13).color(mark_text).font(theme::mono()))
                        // Fixed: the pill is the row's last child and the
                        // flex squeezes exactly that child to zero width
                        // when the segments overflow — the mark vanished
                        // while its row still rendered
                        .width(Length::Fixed(pill_w))
                        .padding(iced::Padding {
                            top: 0.0,
                            right: 3.0,
                            bottom: 0.0,
                            left: 3.0,
                        })
                        .style(move |_t| container::Style {
                            background: Some(iced::Background::Color(mark_bg)),
                            border: iced::border::rounded(3),
                            ..Default::default()
                        }),
                )
            } else {
                r.push(text(t).size(13).color(DIAG_GRAY_100).font(theme::mono()))
            };
        }
        code_col.push(r.into());
    }
    // the frame trick: iced borders have no per-side widths, so the cell
    // is a DIVIDER-painted rounded frame (1px all around) over a stack
    // of [content | 3px accent overlay]. The overlay is a STACK LAYER —
    // a Shrink-height row would resolve a Fill-height bar to zero (the
    // bar painted nothing; the stack bounds it to the content instead) —
    // covering the frame's left side, so the accent reads as the
    // border's left side, like their `border-l-[3px]` over `rounded-md`.
    let stack = iced::widget::Stack::with_children(vec![
        container(
            column![
                text(label.to_string())
                    .size(10)
                    .color(label_color)
                    .font(Font {
                        weight: iced::font::Weight::Semibold,
                        ..Font::DEFAULT
                    }),
                column(code_col).spacing(0),
            ]
            .spacing(5),
        )
        .padding([8, 10])
        .width(Fill)
        .style(move |_t| container::Style {
            background: Some(iced::Background::Color(DIAG_CELL_BG)),
            border: iced::border::rounded(6),
            ..Default::default()
        })
        .into(),
        container(horizontal_space())
            .width(Length::Fixed(3.0))
            .height(Fill)
            .style(move |_t| container::Style {
                background: Some(iced::Background::Color(edge)),
                border: iced::Border {
                    radius: iced::border::Radius {
                        top_left: 5.0,
                        top_right: 0.0,
                        bottom_right: 0.0,
                        bottom_left: 5.0,
                    },
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            })
            .into(),
    ]);
    container(stack)
        .width(Fill)
        .style(move |_t| container::Style {
            background: Some(iced::Background::Color(DIAG_DIVIDER)),
            border: iced::border::rounded(6),
            ..Default::default()
        })
        .padding(1.0)
        .into()
}

/// The file-problem pill (bottom-left of the editor pane): one floating
/// bar per liveness state, each with explicit buttons — no cmdline verbs
/// needed to resolve a disk conflict, deletion, or move.
fn file_status_overlay(app: &App) -> Element<'_, Message> {
    let (icon_color, message, actions): (iced::Color, String, Vec<(&'static str, Message)>) =
        match &app.cur().liveness {
            FileLiveness::Conflict { .. } => (
                theme::WARN,
                "changed on disk — the editor has unsaved edits".to_string(),
                vec![
                    ("Reload", Message::FileReload),
                    ("Keep mine", Message::FileKeepMine),
                ],
            ),
            FileLiveness::DeletedOnDisk { .. } => (
                theme::ERR,
                "is gone from disk".to_string(),
                vec![
                    ("Recreate", Message::FileRecreate),
                    ("Save as\u{2026}", Message::SaveAsPrompt),
                ],
            ),
            FileLiveness::MovedOnDisk { to } => (
                theme::WARN,
                format!("was moved — now at {}", to.display()),
                vec![
                    ("Follow", Message::FileFollow),
                    ("Keep old path", Message::FileKeepMine),
                ],
            ),
            FileLiveness::InSync => return iced::widget::horizontal_space().into(),
        };
    let name = app
        .cur()
        .file
        .as_ref()
        .map(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| f.to_string_lossy().to_string())
        })
        .unwrap_or_default();

    let mut bar = row![
        triangle_svg(icon_color, 13.0),
        text(format!("\"{name}\" {message}"))
            .size(12)
            .color(theme::TEXT),
    ]
    .spacing(8);
    for (label, msg) in actions {
        bar = bar.push(
            button(text(label).size(11).color(theme::TEXT))
                .padding([3, 10])
                .style(move |_t, _s| tool_button_style())
                .on_press(msg),
        );
    }
    container(bar)
        .padding([6, 10])
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.11, 0.98,
            ))),
            border: iced::border::rounded(8).color(theme::BORDER),
            ..Default::default()
        })
        .into()
}

fn toast_overlay(app: &App) -> Element<'static, Message> {
    // the severity accent: a dot in the kind's color (green success, blue
    // info, amber warning, red error) before the text
    let accent = match app.message_kind {
        crate::app::ToastKind::Success => Color::from_rgb8(0x3F, 0xBE, 0x8D),
        crate::app::ToastKind::Info => Color::from_rgb8(0x59, 0xBB, 0xFF),
        crate::app::ToastKind::Warning => Color::from_rgb8(0xE8, 0xB3, 0x40),
        crate::app::ToastKind::Error => Color::from_rgb8(0xF1, 0x78, 0x74),
    };
    let pill = container(
        row![
            container(horizontal_space())
                .width(Length::Fixed(7.0))
                .height(Length::Fixed(7.0))
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(accent)),
                    border: iced::border::rounded(4),
                    ..Default::default()
                }),
            text(app.message.clone().unwrap_or_default())
                .size(12.0)
                .color(theme::TEXT),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center),
    )
    .padding([5, 10])
    .style(|_t| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.08, 0.09, 0.11, 0.98,
        ))),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
    });
    container(pill)
        .width(Fill)
        .height(Fill)
        .padding(12.0)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

#[cfg(test)]
mod tab_strip_chrome_tests {
    use super::*;

    #[test]
    fn separators_hide_around_the_active_and_dragged_tabs() {
        // between two inactive tabs (active is elsewhere): visible
        assert!(separator_visible(1, 2, None));
        // either neighbor active: hidden (the merge makes one redundant)
        assert!(!separator_visible(1, 0, None));
        assert!(!separator_visible(2, 2, None));
        // either neighbor dragged: hidden
        assert!(!separator_visible(1, 3, Some(1)));
        assert!(!separator_visible(2, 3, Some(2)));
    }

    #[test]
    fn the_close_disc_centers_on_the_click_zone() {
        // the 18px click zone at the tab's right edge — the disc shares
        // its horizontal center, so the visual matches the hit target;
        // it tracks the CURRENT width (the open-grow sliver included)
        let d = close_disc(100.0, TAB_W, 16.0);
        // centered 19px from the body edge: a little more right padding
        assert_eq!(d.center_x(), 100.0 + TAB_W - 19.0);
        assert!(d.x + d.width <= 100.0 + TAB_W - 6.0 + 0.01, "inside the body");
        let sliver = close_disc(100.0, 30.0, 16.0);
        assert_eq!(sliver.center_x(), 111.0, "the disc rides the sliver's edge");
        assert!((d.height - 34.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod edge_handle_tests {
    use super::*;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;
    use iced::{Point, Rectangle, Size};

    /// The resize strip over a seam at x=400: 15px wide, from the panes'
    /// top down the window.
    const STRIP: Rectangle = Rectangle::new(Point::new(400.0, 30.0), Size::new(15.0, 870.0));

    fn press(at: Point) -> (canvas::Event, Cursor) {
        (
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Cursor::Available(at),
        )
    }

    #[test]
    fn press_on_the_strip_starts_a_resize_drag() {
        let h = EdgeHandle;
        // diagram side of the seam (the editor half falls through)
        let at = Point::new(410.0, 300.0);
        let (event, cursor) = press(at);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(matches!(message, Some(Message::DividerPressed)));
        assert_eq!(status, canvas::event::Status::Captured);
    }

    #[test]
    fn presses_on_the_editors_half_fall_through_to_the_editor() {
        // the overlay scrollbar lives in the editor's last 10px, inside
        // the strip's editor half — stealing presses there made the
        // scrollbar unclickable (every drag resized the split instead)
        let h = EdgeHandle;
        let at = Point::new(403.0, 300.0); // editor side of the seam
        let (event, cursor) = press(at);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(message.is_none());
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn press_anywhere_else_is_ignored() {
        // regression: an ungated seam turned every click in the editor
        // into a pane resize (clicking near the left edge snapped the
        // split to its minimum)
        let h = EdgeHandle;
        let editor_area = Point::new(60.0, 50.0); // 2nd char of a line
        let (event, cursor) = press(editor_area);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(message.is_none());
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn cursor_shape_resizing_on_the_diagrams_half_only() {
        // regression: iced does not bounds-gate canvas mouse_interaction,
        // so an unconditional ResizingHorizontally put resize arrows over
        // the whole window
        let h = EdgeHandle;
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(Point::new(410.0, 300.0))),
            iced::mouse::Interaction::ResizingHorizontally
        );
        // editor half of the strip: the scrollbar band is not a resize
        // affordance, so no resize cursor there either
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(Point::new(403.0, 300.0))),
            iced::mouse::Interaction::default()
        );
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(Point::new(100.0, 100.0))),
            iced::mouse::Interaction::default()
        );
    }

}

#[cfg(test)]
mod record_view_tests {
    use super::*;
    use dbml_ui::dbml::parser;

    const SRC: &str = r#"Table users {
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
"#;

    fn app() -> App {
        App::with_source(SRC.to_string())
    }

    fn box_of<'a>(app: &'a App, title: &str) -> &'a dbml_ui::layout::TableBox {
        app.diagram().boxes.iter().find(|b| b.title == title).unwrap()
    }

    #[test]
    fn follow_message_matches_the_referenced_row() {
        let app = app();
        let boxes = &app.diagram().boxes;
        let posts = box_of(&app, "posts");
        let rec = posts.records.as_ref().unwrap();
        let uid_pos = rec.columns.iter().position(|&c| c == 1).expect("user_id position");
        // value 1 exists in users' records row 0
        let msg = follow_message(boxes, posts, rec, uid_pos, &RecordValue::Number("1".into()));
        let users = box_of(&app, "users").node;
        assert_eq!(msg, Message::RecordsFollow(users, 0));
        // no matching row: just open the table
        let msg = follow_message(boxes, posts, rec, uid_pos, &RecordValue::Number("77".into()));
        assert_eq!(msg, Message::OpenRecords(users));
        // nulls never navigate
        let msg = follow_message(boxes, posts, rec, uid_pos, &RecordValue::Null);
        assert_eq!(msg, Message::OpenRecords(users));
        // text and number spellings do not match each other (best effort)
        let msg = follow_message(boxes, posts, rec, uid_pos, &RecordValue::Text("1".into()));
        assert_eq!(msg, Message::OpenRecords(users));
    }

    #[test]
    fn display_order_puts_pk_columns_first() {
        let app = app();
        let posts = box_of(&app, "posts");
        let rec = posts.records.as_ref().unwrap();
        // posts: id (pk) then user_id — already pk-first; a reversed
        // header keeps pk first
        assert_eq!(records_display_order(posts, rec), vec![0, 1]);
        let src = r#"Table t {
  b integer
  a integer [pk]
  records (b, a) {
    1, 2
  }
}
"#;
        let app = App::with_source(src.to_string());
        let t = box_of(&app, "t");
        let rec = t.records.as_ref().unwrap();
        assert_eq!(records_display_order(t, rec), vec![1, 0], "a (pk) first");
    }

    #[test]
    fn value_labels_by_kind() {
        assert_eq!(record_value_label(&RecordValue::Null), "(null)");
        assert_eq!(record_value_label(&RecordValue::Text("x".into())), "x");
        assert_eq!(record_value_label(&RecordValue::Number("2.50".into())), "2.50");
        assert_eq!(record_value_label(&RecordValue::Bool(true)), "true");
        assert_eq!(record_value_label(&RecordValue::Ident("draft".into())), "draft");
        assert_eq!(record_value_label(&RecordValue::Expr("now()".into())), "`now()`");
        // and the records fixture parses with records attached
        let out = parser::parse(SRC);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
    }
}

#[cfg(test)]
mod record_view_overlay_tests {
    use super::*;

    #[test]
    fn the_overlay_builds_without_panicking() {
        let app = App::with_source(
            r#"Table users {
  id integer [pk]
  name varchar
  records (id, name) {
    1, 'Alice'
    2, 'Bob'
  }
}

Table empty {
  x integer
  records (x) {
  }
}

Ref: users.id > empty.x
"#
            .to_string(),
        );
        let boxes = &app.diagram().boxes;
        assert!(boxes.iter().all(|b| b.records.is_some()), "both carry records");
        let users = boxes.iter().find(|b| b.title == "users").unwrap().node;
        let empty = boxes.iter().find(|b| b.title == "empty").unwrap().node;
        // users has an FK column (id -> empty.x) that has records: the
        // cell renders as a clickable (the element builds either way)
        let el = record_view_overlay(&app, &RecordView { node: users, follow: Some(1) });
        let _ = el;
        // the empty block: "no rows" path
        let el = record_view_overlay(&app, &RecordView { node: empty, follow: None });
        let _ = el;
        // fallback for a stale node
        let el = record_view_overlay(&app, &RecordView { node: 999, follow: None });
        let _ = el;
    }
}
