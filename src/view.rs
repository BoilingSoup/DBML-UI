//! Window layout: tab strip on top, then the editor and diagram panes
//! flush against each other (their shared border is the pane seam, with
//! the collapse chevron + resize strip stacked over it), status bar at
//! the bottom; help overlay, the record view and the tab context menu
//! stack above all.

use crate::app::{
    App, Drag, EditorMenu, Focus, Message, RecordView, TabMenu, STRIP_LEFT, TAB_SLOT_W,
};
use crate::canvas::diagram::{Diagram, DiagramOverlay};
use crate::canvas::editor::{EditorOverlay, EditorView};
use dbml_ui::dbml::ast::RecordValue;
use crate::editor::{KeyCode, KeyModifiers};
use crate::theme;
use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, stack, text,
};
use iced::{Color, Element, Fill, Font, Length, Point, Rectangle, Size};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

/// Vertical chrome around the editor canvas: every pixel of the root
/// layout above and below it, which `App::page_lines` must subtract from
/// the window height to get the true visible-line count.
///
/// Term by term (iced line height is 1.3x the text size):
/// above the canvas — 30.0 tab strip canvas (fixed height) + 4.0 strip
/// spacing + 8.0 body top padding + 23.6 mode row (text 12 + button
/// padding 8) + 4.0 body spacing + 1.0 pane padding + 20.3 editor
/// toolbar (text 11 + button padding 6) + 4.0 editor spacing = 94.9;
/// below the canvas — 1.0 pane padding + 8.0 body spacing + 15.6 status
/// bar (text 12, no padding) + 0.0 (the bar is flush with the window's
/// bottom edge) = 24.6.
///
/// Measured with a real iced 0.13.1 layout pass over a widget-for-widget
/// replica of `root`: at a 900px window the canvas measures 780.5px tall
/// (900 - 119.5). Keep this in sync with `root`; if the layout chrome
/// changes, re-measure and update both.
pub const EDITOR_CHROME: f32 = 119.5;

/// Window-y of the editor canvas's top edge: every pixel of chrome above
/// it (tab strip 30 + spacing 4 + body top padding 8 + mode row 23.6 +
/// spacing 4 + pane padding 1 + toolbar 20.3 + spacing 4 = 94.9). The
/// Regular-mode selection drag converts window pointer px to the
/// canvas-local coords px_to_pos expects with this constant, so it must
/// move in lockstep with `EDITOR_CHROME` (which is this plus the 24.6
/// below-canvas chrome) and `root`.
pub const EDITOR_CANVAS_TOP: f32 = 94.9;

pub fn root(app: &App) -> Element<'_, Message> {
    let editor_body: Element<'_, Message> = if app.doc_open() {
        column![
            editor_toolbar(app),
            // the overlay (tooltips, completion) is a separate canvas
            // STACKED above: iced flattens one canvas widget's geometries
            // into a mesh pass + a text pass, so a second-geometry tooltip
            // background painted behind the editor text (transparent look)
            stack![
                canvas(EditorView { app }).width(Fill).height(Fill),
                canvas(EditorOverlay { app }).width(Fill).height(Fill)
            ]
            .width(Fill)
            .height(Fill)
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
        .style(move |_t| seam_panel_style(app.focus == Focus::Editor, Seam::Right));

    // the marker/note tooltip is its own canvas STACKED above: iced
    // flattens one canvas widget's geometries into a mesh pass + a text
    // pass, so a second-geometry tooltip pill painted behind the box row
    // text (see DiagramOverlay)
    let diagram_pane = container(
        stack![
            canvas(Diagram {
                layout: app.diagram(),
                zoom: app.cur().zoom,
                pan: app.cur().pan,
                hover: if app.drag == Drag::None {
                    app.canvas_cursor
                } else {
                    None
                },
                dragging: app.drag != Drag::None,
                edge_anim: &app.cur().edge_anim,
                clock: app.anim_clock,
            })
            .width(Fill)
            .height(Fill),
            canvas(DiagramOverlay {
                layout: app.diagram(),
                zoom: app.cur().zoom,
                pan: app.cur().pan,
                hover: if app.drag == Drag::None {
                    app.canvas_cursor
                } else {
                    None
                },
            })
            .width(Fill)
            .height(Fill),
        ]
        .width(Fill)
        .height(Fill),
    )
    .padding(1.0)
    .width(Fill)
    .height(Fill)
    .style(move |_t| {
        // the seam only exists once a document is open (the start screen
        // keeps its rounded gap to the diagram)
        let seam = if app.doc_open() { Seam::Left } else { Seam::None };
        seam_panel_style(app.focus == Focus::Canvas, seam)
    });

    let editor = container(editor_pane)
        .width(Length::FillPortion((app.split * 1000.0) as u16))
        .height(Fill);
    let diagram = container(diagram_pane)
        .width(Length::FillPortion(((1.0 - app.split) * 1000.0) as u16))
        .height(Fill);

    let panes = if !app.doc_open() {
        // start screen: the side pane is not collapsible, no seam
        let start = container(start_pane(app))
            .width(Length::FillPortion((app.split * 1000.0) as u16))
            .height(Fill);
        row![start, diagram].spacing(4).height(Fill)
    } else if app.editor_collapsed {
        row![diagram].height(Fill)
    } else {
        // flush panes: the seam is the two pane borders back to back —
        // one crisp vertical edge, no gap. The collapse chevron and the
        // invisible resize strip are stacked over it below.
        row![editor, diagram].height(Fill)
    };

    // mode row + panes (only once a document is open)
    let body = if app.doc_open() {
        let mode_btn = button(
            text(if app.config.vim {
                "Mode: Vim"
            } else {
                "Mode: Regular"
            })
            .size(12.0)
            .color(theme::TEXT),
        )
        .padding([4, 8])
        .style(move |_t, _s| tool_button_style())
        .on_press(Message::ToggleVim);
        column![row![mode_btn].padding([0, 4]), panes]
            .spacing(4)
            .height(Fill)
    } else {
        column![panes].spacing(4).height(Fill)
    };

    // the tab strip runs full-bleed across the top; the content sits below
    // it, flush with the window's left/right/bottom edges (only a little
    // top padding keeps the body off the strip)
    let base = column![
        canvas(TabStrip { app })
            .height(Length::Fixed(STRIP_H))
            .width(Fill),
        column![body, status_bar(app)].spacing(8).padding(iced::Padding {
            top: 8.0,
            ..Default::default()
        })
    ]
    .spacing(4);

    // the pane seam's edge handle (collapse chevron + resize strip), only
    // once a document is open
    let base: Element<'_, Message> = if app.doc_open() {
        stack![base, edge_overlay(app)].into()
    } else {
        base.into()
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
    // the record view is the topmost modal (help cannot be open beneath it:
    // the record view swallows keys first)
    let base: Element<'_, Message> = if let Some(rv) = &app.record_view {
        stack![base, record_view_overlay(app, rv)].into()
    } else {
        base
    };
    if app.show_help {
        stack![base, help_overlay()].into()
    } else {
        base
    }
}

/// The record view: a centered modal over everything (topmost in `root`),
/// showing a table's `records` sample rows as a scrollable grid. Escape,
/// the × button, or a backdrop click closes it.
fn record_view_overlay(app: &App, rv: &RecordView) -> Element<'static, Message> {
    let w = (app.window_size.0 * 0.8).clamp(340.0, 800.0);
    let h = (app.window_size.1 * 0.8).clamp(240.0, 520.0);
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
    let panel = container(body)
        .width(Length::Fixed(w))
        .height(Length::Fixed(h))
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(theme::PANEL)),
            border: iced::border::rounded(10).color(theme::BORDER),
            ..Default::default()
        });
    // click anywhere outside the panel dismisses it
    let backdrop = button(text(""))
        .width(Fill)
        .height(Fill)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(Message::CloseRecords);
    // stack children land at (0,0): centering the panel is the full-size
    // container's job (its content alignment)
    stack![backdrop, container(panel).center(Fill)].into()
}

/// The record view's content: header row (pictogram, table name, close)
/// over the scrollable grid of record rows.
fn record_panel(
    b: &dbml_ui::layout::TableBox,
    boxes: &[dbml_ui::layout::TableBox],
    rv: &RecordView,
) -> Element<'static, Message> {
    let rec = b.records.as_ref().expect("records for the panel");
    let close = button(
        text("\u{00d7}".to_string())
            .size(16.0)
            .color(theme::DIM),
    )
    .padding([2, 10])
    .style(move |_t, _s| tool_button_style())
    .on_press(Message::CloseRecords);
    let mut title_row = row![
        canvas(RecordGlyph).width(Length::Fixed(18.0)).height(Length::Fixed(18.0)),
        text(b.title.clone())
            .size(15.0)
            .color(theme::TEXT)
            .font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            }),
    ]
    .spacing(8)
    .align_y(iced::alignment::Vertical::Center);
    if rec.example {
        title_row = title_row.push(
            text("example".to_string())
                .size(10.0)
                .color(theme::DIM)
                .width(Length::Shrink),
        );
    }
    let header = row![title_row, horizontal_space(), close]
        .align_y(iced::alignment::Vertical::Center)
        .spacing(8);

    // display order: the record's value columns with PK columns sorted
    // first (see records_display_order)
    let order = records_display_order(b, rec);

    // one width per display position, fitting the header parts AND every
    // value in that column, so the grid lines up
    let widths: Vec<f32> = order
        .iter()
        .map(|&p| {
            let rowi = rec.columns.get(p).and_then(|&ci| b.rows.get(ci));
            let name = rowi.map_or_else(|| "?".to_string(), |r| r.name.clone());
            let ty = rowi.map_or_else(String::new, |r| r.ty.clone());
            let mut chips: Vec<&str> = Vec::new();
            if rowi.is_some_and(|r| r.not_null == Some(true)) {
                chips.push("Not Null");
            }
            if rowi.is_some_and(|r| r.unique) {
                chips.push("Unique");
            }
            let chip_text = chips.join(" \u{00b7} ");
            let mut parts: Vec<String> = vec![name, ty, chip_text];
            for row_vals in &rec.rows {
                let v = row_vals.get(p).unwrap_or(&RecordValue::Null);
                parts.push(record_value_label(v));
            }
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            cell_w(&refs)
        })
        .collect();

    let mut grid = column![].spacing(0);
    let mut header_row = row![].spacing(0);
    header_row = header_row.push(
        container(text("#".to_string()).size(11.0).color(theme::DIM))
            .width(Length::Fixed(36.0))
            .padding([4, 6]),
    );
    for (&p, &w) in order.iter().zip(&widths) {
        let Some(&ci) = rec.columns.get(p) else {
            continue;
        };
        let rowi = b.rows.get(ci);
        let name = rowi.map_or_else(|| "?".to_string(), |r| r.name.clone());
        let ty = rowi.map_or_else(String::new, |r| r.ty.clone());
        let pk = rowi.is_some_and(|r| r.pk);
        let mut chips: Vec<String> = Vec::new();
        if rowi.is_some_and(|r| r.not_null == Some(true)) {
            chips.push("Not Null".to_string());
        }
        if rowi.is_some_and(|r| r.unique) {
            chips.push("Unique".to_string());
        }
        let mut name_line = row![].spacing(2);
        if pk {
            name_line = name_line.push(
                text("\u{25c6}".to_string())
                    .size(10.0)
                    .color(theme::PK),
            );
        }
        name_line = name_line.push(
            text(name.clone())
                .size(12.0)
                .color(theme::TEXT)
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..Font::DEFAULT
                }),
        );
        let mut head = column![
            name_line,
            text(ty.clone()).size(10.0).color(theme::TYPE_TEXT),
        ]
        .spacing(1);
        if !chips.is_empty() {
            head = head.push(
                text(chips.join(" \u{00b7} "))
                    .size(9.0)
                    .color(theme::DIM),
            );
        }
        header_row = header_row.push(container(head).width(Length::Fixed(w)).padding([4, 8]));
    }
    grid = grid.push(header_row);

    for (ri, row_vals) in rec.rows.iter().enumerate() {
        let followed = rv.follow == Some(ri);
        let mut line = row![].spacing(0);
        line = line.push(
            container(
                text(format!("{}", ri + 1))
                    .size(11.0)
                    .color(theme::DIM),
            )
            .width(Length::Fixed(36.0))
            .padding([4, 6]),
        );
        for (&p, &w) in order.iter().zip(&widths) {
            let v = row_vals.get(p).unwrap_or(&RecordValue::Null);
            line = line.push(record_cell(boxes, b, rec, p, v, w));
        }
        let line = if followed {
            container(line).style(|_t| container::Style {
                background: Some(iced::Background::Color(theme::ROW_HOVER_BG)),
                ..Default::default()
            })
        } else {
            container(line)
        };
        grid = grid.push(line);
    }
    if rec.rows.is_empty() {
        grid = grid.push(
            container(text("no rows".to_string()).size(12.0).color(theme::DIM))
                .padding([8, 42]),
        );
    }

    column![
        header,
        // the scrollable must not Fill its horizontal axis when it can
        // scroll horizontally (iced validates that); rows size to content
        scrollable(grid)
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new(),
                horizontal: scrollable::Scrollbar::new(),
            })
            .height(Fill)
    ]
    .spacing(10)
    .padding(12)
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
fn record_cell(
    boxes: &[dbml_ui::layout::TableBox],
    b: &dbml_ui::layout::TableBox,
    rec: &dbml_ui::model::NodeRecords,
    p: usize,
    v: &RecordValue,
    w: f32,
) -> Element<'static, Message> {
    let label = record_value_label(v);
    let color = match v {
        RecordValue::Null => theme::DIM,
        RecordValue::Number(_) => theme::SYN_NUMBER,
        RecordValue::Ident(_) => theme::SYN_NAME,
        RecordValue::Expr(_) => theme::SYN_STRING,
        _ => theme::TEXT,
    };
    let italic = matches!(v, RecordValue::Null);
    let content = move |c: Color| {
        text(label.clone())
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
    };
    let navigable = rec
        .targets
        .get(p)
        .copied()
        .flatten()
        .is_some_and(|_| !matches!(v, RecordValue::Null));
    if !navigable {
        return container(content(color))
            .width(Length::Fixed(w))
            .padding([4, 8])
            .into();
    }
    button(content(theme::FK))
        .padding([4, 8])
        .width(Length::Fixed(w))
        .style(move |_t, _s| iced::widget::button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(follow_message(boxes, b, rec, p, v))
        .into()
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
fn record_value_label(v: &RecordValue) -> String {
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
struct RecordGlyph;

impl canvas::Program<Message> for RecordGlyph {
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
        let size = 14.0;
        crate::canvas::diagram::draw_table_pictogram(
            &mut frame,
            Point::new(bounds.width / 2.0, bounds.height / 2.0),
            size,
            theme::TEXT,
        );
        vec![frame.into_geometry()]
    }
}

/// Chrome-like tab strip, canvas-drawn so drag/slide animation has full
/// control of each tab's x (a widget row cannot offset or overlap tabs).
/// One tab per open document with its close X, then a +; click activates,
/// drag reorders (math in `App::tab_drag_moved`), right-click opens the
/// context menu, middle-click closes.
pub struct TabStrip<'a> {
    pub app: &'a App,
}

/// Visual tab width and the strip height.
const TAB_W: f32 = TAB_SLOT_W - 8.0;
const STRIP_H: f32 = 30.0;

impl TabStrip<'_> {
    /// The tab under a strip-local point, if any (X-zone included).
    fn tab_at(&self, x: f32, y: f32) -> Option<usize> {
        if !(0.0..STRIP_H).contains(&y) {
            return None;
        }
        let i = ((x - STRIP_LEFT) / TAB_SLOT_W).floor();
        if !(0.0..self.app.tabs.len() as f32).contains(&i) {
            return None;
        }
        let i = i as usize;
        (0.0..TAB_W).contains(&(x - STRIP_LEFT - i as f32 * TAB_SLOT_W)).then_some(i)
    }

    fn plus_at(&self, x: f32, y: f32) -> bool {
        let px = STRIP_LEFT + self.app.tabs.len() as f32 * TAB_SLOT_W;
        (0.0..STRIP_H).contains(&y) && (px..px + 30.0).contains(&x)
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
        let hover_i = cursor
            .position()
            .map(|c| {
                let l = Point::new(c.x - bounds.x, c.y - bounds.y);
                self.tab_at(l.x, l.y)
            })
            .unwrap_or(None);
        // the dragged tab draws LAST and OPAQUE (Chrome): while it overlaps
        // a displaced neighbor mid-slide, the neighbor's text must not
        // bleed through it
        let mut order: Vec<usize> = (0..self.app.tabs.len()).collect();
        order.sort_by_key(|&i| i == self.app.active);
        for i in order {
            let tab = &self.app.tabs[i];
            let off = self.app.tab_anim.get(i).copied().unwrap_or(0.0);
            let active = i == self.app.active;
            let x = STRIP_LEFT + i as f32 * TAB_SLOT_W + off;
            let rect = Rectangle::new(Point::new(x, 2.0), Size::new(TAB_W, STRIP_H - 4.0));
            let hovered = hover_i == Some(i) && self.app.tab_drag.is_none();
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    rect.position(),
                    rect.size(),
                    iced::border::Radius::from(6.0),
                ),
                if active {
                    theme::PANEL
                } else if hovered {
                    Color::from_rgba(0.11, 0.129, 0.157, 0.5)
                } else {
                    Color::TRANSPARENT
                },
            );
            if active {
                frame.stroke(
                    &canvas::Path::rounded_rectangle(
                        rect.position(),
                        rect.size(),
                        iced::border::Radius::from(6.0),
                    ),
                    canvas::Stroke {
                        style: theme::BORDER.into(),
                        width: 1.0,
                        ..Default::default()
                    },
                );
            }
            let mut title = tab.title();
            if tab.is_modified() {
                title = format!("\u{2022} {title}");
            }
            frame.fill_text(canvas::Text {
                content: truncate_title(&title, 22),
                position: Point::new(x + 10.0, rect.center_y()),
                color: if active { theme::TEXT } else { theme::DIM },
                size: 12.0.into(),
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
            frame.fill_text(canvas::Text {
                content: "\u{00d7}".to_string(),
                position: Point::new(x + TAB_W - 16.0, rect.center_y()),
                color: theme::DIM,
                size: 11.0.into(),
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        // the + button
        let px = STRIP_LEFT + self.app.tabs.len() as f32 * TAB_SLOT_W + 4.0;
        frame.fill_text(canvas::Text {
            content: "+".to_string(),
            position: Point::new(px + 10.0, STRIP_H * 0.5),
            color: theme::DIM,
            size: 13.0.into(),
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
                        let x_zone = STRIP_LEFT + i as f32 * TAB_SLOT_W + TAB_W - 18.0;
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
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        use iced::mouse::Interaction;
        if self.app.tab_drag.is_some() {
            return Interaction::Grabbing;
        }
        if let Some(p) = cursor.position_over(bounds) {
            let p = Point::new(p.x - bounds.x, p.y - bounds.y);
            if self.tab_at(p.x, p.y).is_some() || self.plus_at(p.x, p.y) {
                return Interaction::Pointer;
            }
        }
        Interaction::default()
    }
}

/// Clip a tab title to `max` chars, ellipsis at the end.
fn truncate_title(title: &str, max: usize) -> String {
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
    const MENU_H: f32 = 130.0;
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
            item("Format", Message::EditorMenuFormat, true),
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

/// Small toolbar above the editor: Save, Save As, Format, fmt-on-save
/// switch.
fn editor_toolbar(app: &App) -> Element<'static, Message> {
    use iced::widget::button;
    let btn = |label: &'static str, msg: Message, accent: bool| {
        let mut b = button(text(label).size(11.0).color(theme::TEXT))
            .padding([3, 8])
            .style(move |_t, _s| tool_button_style());
        if accent {
            b = b.style(move |_t, _s| {
                let mut st = tool_button_style();
                st.border = st.border.color(theme::FK);
                st
            });
        }
        b.on_press_maybe(Some(msg))
    };
    let fmt_switch = button(
        text(if app.config.fmt_on_save {
            "Fmt-on-Save ✓"
        } else {
            "Fmt-on-Save ✗"
        })
        .size(11.0)
        .color(if app.config.fmt_on_save {
            theme::PK
        } else {
            theme::DIM
        }),
    )
    .padding([3, 8])
    .style(move |_t, _s| tool_button_style())
    .on_press(Message::ToggleFmtOnSave);
    row![
        btn(
            "Save",
            Message::KeyPressed(KeyCode::Char('s'), KeyModifiers::CONTROL),
            true
        ),
        btn(
            "Save As",
            Message::KeyPressed(KeyCode::Char('n'), KeyModifiers::CONTROL),
            false
        ),
        btn("Format", Message::FormatNow, false),
        fmt_switch,
        horizontal_space(),
    ]
    .padding([0, 2])
    .spacing(4)
    .into()
}

/// The no-document start screen: Open file / New diagram. Shown in place
/// of the editor pane; this state is not collapsible.
fn start_pane(app: &App) -> Element<'static, Message> {
    let open = button(text("Open file…".to_string()).size(14.0).color(theme::TEXT))
        .padding([8, 16])
        .width(200)
        .style(move |_t, _s| tool_button_style())
        .on_press(Message::OpenFilePicker);
    let new = button(
        text("New diagram".to_string())
            .size(14.0)
            .color(theme::TEXT),
    )
    .padding([8, 16])
    .width(200)
    .style(move |_t, _s| tool_button_style())
    .on_press(Message::NewDiagram);
    let mut list = column![
        text("dbml-ui".to_string()).size(20.0).color(theme::TEXT),
        text("DBML diagram editor".to_string())
            .size(12.0)
            .color(theme::DIM),
        open,
        new,
    ]
    .spacing(12)
    .align_x(iced::alignment::Horizontal::Center);
    if !app.config.recent.is_empty() {
        list = list.push(text("Recent".to_string()).size(12.0).color(theme::DIM));
        for path in app.config.recent.iter().take(5) {
            let label = match path.rsplit('/').next() {
                Some(base) if !base.is_empty() => base.to_string(),
                _ => path.clone(),
            };
            list = list.push(
                button(text(label).size(12.0).color(theme::TEXT))
                    .padding([4, 10])
                    .width(200)
                    .style(move |_t, _s| tool_button_style())
                    .on_press(Message::OpenRecent(path.clone())),
            );
        }
    }
    container(list)
        .align_x(iced::alignment::Horizontal::Center)
        .width(Fill)
        .height(Fill)
        .padding(1.0)
        .style(move |_t| panel_style(false))
        .into()
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

/// dbdiagram-style edge handle over the pane seam: a small chevron pill
/// straddling the flush pane borders (click = collapse/expand the editor,
/// pointing the way it will move), on an invisible full-height strip that
/// starts the split-resize drag (tracked window-wide via the event
/// subscription). One canvas draws and hit-tests both, so the pill sits
/// exactly on the seam however the panes are split.
fn edge_overlay(app: &App) -> Element<'_, Message> {
    // below the tab strip: strip 30 + strip spacing 4 + base padding 8
    const PANES_TOP: f32 = 42.0;
    container(
        canvas(EdgeHandle {
            collapsed: app.editor_collapsed,
        })
        .width(Length::Fixed(EDGE_STRIP_W))
        .height(Fill),
    )
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
/// each pane: the visible edge stays the panes' 1px borders).
const EDGE_STRIP_W: f32 = 15.0;
/// The collapse chevron pill, straddling the seam.
const PILL_W: f32 = 14.0;
const PILL_H: f32 = 30.0;
/// Pill center, strip-local from the top — the strip starts 42px down, so
/// the pill rides the seam in the panes' upper area, like dbdiagram's.
const PILL_CY: f32 = 54.0;

struct EdgeHandle {
    /// Editor pane is collapsed: the chevron points the other way.
    collapsed: bool,
}

impl EdgeHandle {
    /// The chevron pill in window coordinates (cursor positions from
    /// `position_over` are window-relative).
    fn pill(&self, bounds: iced::Rectangle) -> iced::Rectangle {
        iced::Rectangle::new(
            iced::Point::new(
                bounds.x + (bounds.width - PILL_W) / 2.0,
                bounds.y + PILL_CY - PILL_H / 2.0,
            ),
            iced::Size::new(PILL_W, PILL_H),
        )
    }

    /// The chevron: ‹ collapses (the pane will slide left), › expands.
    fn glyph(&self) -> &'static str {
        if self.collapsed {
            "\u{203a}"
        } else {
            "\u{2039}"
        }
    }
}

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
            let msg = if self.pill(bounds).contains(p) {
                Message::ToggleEditor
            } else {
                Message::DividerPressed
            };
            return (canvas::event::Status::Captured, Some(msg));
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // the pill, centered on the seam (strip-local coords)
        let pill = iced::Rectangle::new(
            iced::Point::new((bounds.width - PILL_W) / 2.0, PILL_CY - PILL_H / 2.0),
            iced::Size::new(PILL_W, PILL_H),
        );
        let hovered = cursor
            .position()
            .map(|p| pill.contains(iced::Point::new(p.x - bounds.x, p.y - bounds.y)))
            .unwrap_or(false);
        let path = canvas::Path::rounded_rectangle(
            pill.position(),
            pill.size(),
            iced::border::Radius::from(PILL_W / 2.0),
        );
        frame.fill(&path, if hovered { theme::TOOLTIP } else { theme::PANEL });
        frame.stroke(
            &path,
            canvas::Stroke {
                style: theme::BORDER.into(),
                width: 1.0,
                ..Default::default()
            },
        );
        frame.fill_text(canvas::Text {
            content: self.glyph().to_string(),
            position: iced::Point::new(pill.center_x(), pill.center_y()),
            color: theme::TEXT,
            size: 13.0.into(),
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
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
        if self.pill(bounds).contains(p) {
            Interaction::Pointer
        } else {
            Interaction::ResizingHorizontally
        }
    }
}

fn help_overlay() -> Element<'static, Message> {
    let line = |key: &str, desc: &str| -> Element<'static, Message> {
        row![
            text(key.to_string())
                .size(13.0)
                .color(theme::FK)
                .width(Length::Fixed(210.0)),
            text(desc.to_string()).size(13.0).color(theme::TEXT),
        ]
        .spacing(8)
        .into()
    };
    let help = column![
        text("dbml-ui — keys").size(16.0).color(theme::TEXT),
        line("i a o O / Esc", "enter / leave insert mode"),
        line(
            "hjkl w b e 0 $ ^ gg G { }",
            "motions (counts work: 3w, 2gg, ...)"
        ),
        line("x dd dw cw cc d$ c0 yy p P diw ciw", "operators"),
        line(
            ":export sql [postgres|mysql|mssql|oracle|sqlite]",
            "write DDL for a dialect"
        ),
        line("open file.sql", "import a SQL dump as DBML"),
        line("r{c}", "replace char (with [count])"),
        line(
            "v + motions, y / d / u / U / o",
            "visual: yank/delete/lower/upper/swap"
        ),
        line("u Ctrl-r / pattern n N", "undo redo / search next prev"),
        line(
            "Ctrl-d Ctrl-u PgUp PgDn",
            "scroll half/full page (animated)"
        ),
        line("arrows Home End Delete", "work in every mode"),
        line(":w :q :q! :wq :o file :export f.svg|sql", "ex commands"),
        line("Ctrl-s / Ctrl-c", "save / quit"),
        line("Ctrl+T / Ctrl+W", "new / close tab (right-click a tab: menu)"),
        line(
            "Ctrl-Space / type 2+ letters",
            "completion popup (insert mode)"
        ),
        line("Up Down Enter Tab Esc", "navigate / accept / dismiss popup"),
        line(
            "wheel / drag / drag a table",
            "zoom at cursor / pan / move table (live)"
        ),
        line("click a table", "jump the editor to its definition"),
        line("Tab / ?", "switch pane focus / close this help"),
    ]
    .spacing(6)
    .padding(20);
    container(help)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.11, 0.96,
            ))),
            border: iced::border::rounded(10).color(theme::BORDER),
            ..Default::default()
        })
        .width(Length::Fixed(580.0))
        .height(Length::Shrink)
        .into()
}

fn panel_style(focused: bool) -> container::Style {
    seam_panel_style(focused, Seam::None)
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

fn seam_panel_style(focused: bool, _seam: Seam) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme::PANEL)),
        border: iced::border::Border {
            color: if focused { theme::FK } else { theme::BORDER },
            width: 1.0,
            radius: iced::border::Radius::from(0.0),
        },
        ..Default::default()
    }
}

fn mode_chip(mode: &crate::editor::Mode, linewise: bool) -> (&'static str, Color) {
    match mode {
        crate::editor::Mode::Insert => (" INSERT ", Color::from_rgb(0.30, 0.75, 0.95)),
        crate::editor::Mode::Visual => {
            if linewise {
                (" V-LINE ", Color::from_rgb(0.98, 0.62, 0.80))
            } else {
                (" VISUAL ", Color::from_rgb(0.98, 0.75, 0.30))
            }
        }
        crate::editor::Mode::Normal => (" NORMAL ", Color::from_rgb(0.55, 0.90, 0.55)),
    }
}

fn status_bar(app: &App) -> Element<'_, Message> {
    let tables = app
        .diagram()
        .boxes
        .iter()
        .filter(|b| b.kind == dbml_ui::model::NodeKind::Table)
        .count();
    let zoom = format!("zoom {:.0}%", app.cur().zoom / BASE_ZOOM * 100.0);
    let errors = format!("errors {}", app.cur().parse_errors.len());
    let (crow, ccol) = app.editor().buf.cursor();

    let seg = |s: &str, color: Color| -> Element<'_, Message> {
        text(s.to_string())
            .size(12.0)
            .color(color)
            .width(Length::Shrink)
            .into()
    };

    // leading segment: the cmdline when open, else mode + showcmd + focus
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let Some(cmd) = &app.editor().cmdline {
        items.push(seg(
            &format!(" {}{}▏ ", cmd.kind, cmd.input),
            iced::Color::BLACK,
        ));
    } else {
        let (mode, color) = if app.regular_mode() {
            (" REGULAR ", Color::from_rgb(0.55, 0.75, 0.95))
        } else {
            mode_chip(&app.editor().mode, app.editor().visual_linewise)
        };
        items.push(
            container(seg(mode, Color::BLACK))
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::border::rounded(4),
                    ..Default::default()
                })
                .into(),
        );
        if let Some(sc) = app.editor().showcmd() {
            items.push(seg(&sc, theme::DIM));
        }
        match app.focus {
            Focus::Editor => items.push(seg("EDITOR", theme::TEXT)),
            Focus::Canvas => items.push(seg("CANVAS", theme::TEXT)),
        }
    }
    items.push(seg("·", theme::DIM));
    if let Some(msg) = &app.message {
        items.push(seg(msg, theme::FK));
    } else {
        let name = app
            .file()
            .as_ref()
            .map(|f| f.display().to_string())
            .unwrap_or_else(|| "[No Name]".to_string());
        let modified = if app.is_modified() { " [+]" } else { "" };
        items.push(seg(&format!("{name}{modified}"), theme::TEXT));
    }
    items.push(seg("·", theme::DIM));
    items.push(seg(
        &format!("Ln {}, Col {}", crow + 1, ccol + 1),
        theme::DIM,
    ));
    items.push(seg("·", theme::DIM));
    items.push(seg(&format!("tables {tables}"), theme::DIM));
    items.push(seg("·", theme::DIM));
    items.push(seg(&zoom, theme::DIM));
    items.push(seg("·", theme::DIM));
    items.push(seg(&errors, theme::DIM));

    let mut bar = row![];
    let mut any = false;
    for item in items {
        if any {
            bar = bar.push(seg(" ", theme::DIM));
        }
        bar = bar.push(item);
        any = true;
    }

    container(bar.spacing(6))
        .width(Fill)
        .height(Length::Shrink)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(theme::BG)),
            border: iced::border::rounded(6).color(theme::BORDER),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod edge_handle_tests {
    use super::*;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;
    use iced::{Point, Rectangle, Size};

    /// The resize strip over a seam at x=400: 15px wide, from the panes'
    /// top down the window.
    const STRIP: Rectangle = Rectangle::new(Point::new(400.0, 42.0), Size::new(15.0, 858.0));

    fn press(at: Point) -> (canvas::Event, Cursor) {
        (
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Cursor::Available(at),
        )
    }

    #[test]
    fn press_on_the_chevron_toggles_the_editor() {
        let h = EdgeHandle { collapsed: false };
        let at = h.pill(STRIP).center();
        let (event, cursor) = press(at);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(matches!(message, Some(Message::ToggleEditor)));
        assert_eq!(status, canvas::event::Status::Captured);
    }

    #[test]
    fn press_on_the_strip_starts_a_resize_drag() {
        let h = EdgeHandle { collapsed: false };
        let at = Point::new(407.0, 300.0); // on the strip, below the pill
        let (event, cursor) = press(at);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(matches!(message, Some(Message::DividerPressed)));
        assert_eq!(status, canvas::event::Status::Captured);
    }

    #[test]
    fn press_anywhere_else_is_ignored() {
        // regression: an ungated seam turned every click in the editor
        // into a pane resize (clicking near the left edge snapped the
        // split to its minimum)
        let h = EdgeHandle { collapsed: false };
        let editor_area = Point::new(60.0, 50.0); // 2nd char of a line
        let (event, cursor) = press(editor_area);
        let (status, message) = h.update(&mut (), event, STRIP, cursor);
        assert!(message.is_none());
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn cursor_shape_pointer_on_the_pill_resize_on_the_strip() {
        // regression: iced does not bounds-gate canvas mouse_interaction,
        // so an unconditional ResizingHorizontally put resize arrows over
        // the whole window
        let h = EdgeHandle { collapsed: false };
        let pill = h.pill(STRIP);
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(pill.center())),
            iced::mouse::Interaction::Pointer
        );
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(Point::new(407.0, 300.0))),
            iced::mouse::Interaction::ResizingHorizontally
        );
        assert_eq!(
            h.mouse_interaction(&(), STRIP, Cursor::Available(Point::new(100.0, 100.0))),
            iced::mouse::Interaction::default()
        );
    }

    #[test]
    fn the_chevron_flips_with_the_collapsed_state() {
        assert_ne!(
            EdgeHandle { collapsed: false }.glyph(),
            EdgeHandle { collapsed: true }.glyph(),
            "expanded points left (collapse), collapsed points right (expand)"
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
