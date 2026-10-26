//! Window layout: tab strip on top, editor pane | divider | diagram pane
//! below, status bar at the bottom; help overlay and the tab context menu
//! stack above everything.

use crate::app::{App, Drag, Focus, Message, TabMenu, STRIP_LEFT, TAB_SLOT_W};
use crate::canvas::diagram::Diagram;
use crate::canvas::editor::{EditorOverlay, EditorView};
use crate::editor::{KeyCode, KeyModifiers};
use crate::theme;
use iced::widget::{
    button, canvas, column, container, horizontal_space, mouse_area, row, stack, text,
    vertical_space,
};
use iced::{Color, Element, Fill, Length};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

/// Vertical chrome around the editor canvas: every pixel of the root
/// layout above and below it, which `App::page_lines` must subtract from
/// the window height to get the true visible-line count.
///
/// Term by term (iced line height is 1.3x the text size):
/// above the canvas — 23.6 tab strip (text 12 + tab padding 8) + 4.0
/// strip spacing + 8.0 base padding + 23.6 toolbar row (text 12 + button
/// padding 8) + 4.0 body spacing + 1.0 pane padding + 20.3 editor
/// toolbar (text 11 + button padding 6) + 4.0 editor spacing = 88.5;
/// below the canvas — 1.0 pane padding + 8.0 base spacing + 27.6 status
/// bar (text 12 + its padding 12) + 8.0 base padding = 44.6.
///
/// Verified against a real iced 0.13.1 layout pass: at a 900px window the
/// canvas measures 766.9px tall (900 - 133.1). Keep this in sync with
/// `root`; if the layout chrome changes, re-measure and update both.
pub const EDITOR_CHROME: f32 = 133.1;

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
        .style(move |_t| panel_style(app.focus == Focus::Editor));

    let diagram_pane = container(
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
    )
    .padding(1.0)
    .width(Fill)
    .height(Fill)
    .style(move |_t| panel_style(app.focus == Focus::Canvas));

    let editor = container(editor_pane)
        .width(Length::FillPortion((app.split * 1000.0) as u16))
        .height(Fill);
    let diagram = container(diagram_pane)
        .width(Length::FillPortion(((1.0 - app.split) * 1000.0) as u16))
        .height(Fill);

    let panes = if !app.doc_open() {
        // start screen: the side pane is not collapsible, no divider
        let start = container(start_pane(app))
            .width(Length::FillPortion((app.split * 1000.0) as u16))
            .height(Fill);
        row![start, diagram].spacing(4).height(Fill)
    } else if app.editor_collapsed {
        row![diagram].spacing(4).height(Fill)
    } else {
        row![editor, divider(), diagram].spacing(4).height(Fill)
    };

    // toolbar with the collapse toggle (only once a document is open)
    let body = if app.doc_open() {
        let toggle = button(
            text(if app.editor_collapsed {
                "Show editor ▸"
            } else {
                "◂ Hide editor"
            })
            .size(12.0)
            .color(theme::TEXT),
        )
        .padding([4, 8])
        .style(move |_t, _s| tool_button_style())
        .on_press(Message::ToggleEditor);
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
        column![
            row![mode_btn, horizontal_space(), toggle].padding([0, 4]),
            panes
        ]
        .spacing(4)
        .height(Fill)
    } else {
        column![panes].spacing(4).height(Fill)
    };

    // the tab strip runs full-bleed across the top; the padded content
    // sits below it
    let base = column![
        tab_strip(app),
        column![body, status_bar(app)].spacing(8).padding(8)
    ]
    .spacing(4);

    let base: Element<'_, Message> = if let Some(menu) = &app.tab_menu {
        stack![base, tab_menu_overlay(app, menu)].into()
    } else {
        base.into()
    };
    if app.show_help {
        stack![base, help_overlay()].into()
    } else {
        base
    }
}

/// Chrome-like tab strip: one tab per open document (or start screen),
/// each with a close X, then a + that opens a fresh start-screen tab.
/// Click activates, click & drag reorders (the math lives in
/// `App::tab_drag_moved`), right-click opens the context menu.
fn tab_strip(app: &App) -> Element<'static, Message> {
    let mut tabs = row![].spacing(8);
    for i in 0..app.tabs.len() {
        tabs = tabs.push(tab_button(app, i));
    }
    let plus = button(text("+").size(13.0).color(theme::DIM))
        .padding([2, 8])
        .style(move |_t, _s| tool_button_style())
        .on_press(Message::NewTab);
    container(tabs.push(plus))
        .padding([0.0, STRIP_LEFT])
        .width(Fill)
        .style(|_t| container::Style {
            background: Some(iced::Background::Color(theme::BG)),
            ..Default::default()
        })
        .into()
}

/// One tab: title (with a • while it has unsaved changes) plus a close X,
/// wrapped in a mouse_area for activate / right-click menu / middle-click
/// close. The X is a real button inside it: it captures its own press, so
/// closing never activates the tab (Chrome semantics). Width is
/// `TAB_SLOT_W` minus the strip's 8px tab gap so drag-drop slots line up.
fn tab_button(app: &App, i: usize) -> Element<'static, Message> {
    let tab = &app.tabs[i];
    let active = i == app.active;
    let mut title = tab.title();
    if tab.is_modified() {
        title = format!("• {title}");
    }
    let label = text(truncate_title(&title, 22))
        .size(12.0)
        .color(if active { theme::TEXT } else { theme::DIM })
        .width(Fill);
    let close = button(text("×").size(10.0).color(theme::DIM))
        .padding([1, 3])
        .style(move |_t, _s| close_button_style())
        .on_press(Message::CloseTab(i));
    let content = row![label, close]
        .spacing(4)
        .align_y(iced::alignment::Vertical::Center);
    mouse_area(
        container(content)
            .padding([4, 8])
            .width(Length::Fixed(TAB_SLOT_W - 8.0))
            .style(move |_t| tab_style(active)),
    )
    .on_press(Message::TabPressed(i))
    .on_right_press(Message::TabMenu(i))
    .on_middle_press(Message::CloseTab(i))
    .into()
}

/// Clip a tab title to `max` chars, ellipsis at the end.
fn truncate_title(title: &str, max: usize) -> String {
    let count = title.chars().count();
    if count <= max {
        return title.to_string();
    }
    format!(
        "{}…",
        title.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

/// Chrome-like tab shape: the active tab is a PANEL card with rounded top
/// corners sitting on the strip; inactive tabs melt into it.
fn tab_style(active: bool) -> container::Style {
    container::Style {
        background: active.then_some(iced::Background::Color(theme::PANEL)),
        border: iced::border::Border {
            radius: iced::border::Radius {
                top_left: 6.0,
                top_right: 6.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            color: if active {
                theme::BORDER
            } else {
                Color::TRANSPARENT
            },
            width: 1.0,
        },
        ..Default::default()
    }
}

/// Quiet close X: no background of its own.
fn close_button_style() -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: None,
        text_color: theme::DIM,
        border: iced::border::rounded(4),
        ..Default::default()
    }
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
    stack![
        backdrop,
        column![
            horizontal_space().width(Length::Fixed(x)),
            row![vertical_space().height(Length::Fixed(y)), panel]
        ]
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

/// The draggable split handle; pressing it starts a resize drag tracked
/// window-wide via the event subscription.
fn divider() -> Element<'static, Message> {
    canvas(DividerHandle)
        .width(Length::Fixed(6.0))
        .height(Fill)
        .into()
}

struct DividerHandle;

impl canvas::Program<Message> for DividerHandle {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        // The canvas widget forwards every window event; only start a
        // divider drag when the press actually lands on the handle.
        // (Ungated, any click in the editor also began a resize, so a
        // click near the window's left edge snapped the split to its
        // minimum.)
        if cursor.position_over(bounds).is_none() {
            return (canvas::event::Status::Ignored, None);
        }
        match event {
            canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => (
                canvas::event::Status::Captured,
                Some(Message::DividerPressed),
            ),
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(
            iced::Point::new(2.0, 0.0),
            iced::Size::new(2.0, bounds.height),
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
        // canvas mouse_interaction is not bounds-gated by iced — without
        // this check the resize cursor leaked over the whole window
        if cursor.position_over(bounds).is_some() {
            iced::mouse::Interaction::ResizingHorizontally
        } else {
            iced::mouse::Interaction::default()
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
    container::Style {
        background: Some(iced::Background::Color(theme::PANEL)),
        border: iced::border::rounded(8).color(if focused { theme::FK } else { theme::BORDER }),
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
        .padding([6.0, 10.0])
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
mod divider_tests {
    use super::*;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;
    use iced::{Point, Rectangle, Size};

    const HANDLE: Rectangle = Rectangle::new(Point::new(200.0, 0.0), Size::new(6.0, 300.0));

    #[test]
    fn press_on_the_handle_starts_a_drag() {
        let h = DividerHandle;
        let inside = Point::new(203.0, 50.0); // window coords, on the handle
        let (status, message) = h.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            HANDLE,
            Cursor::Available(inside),
        );
        assert!(matches!(message, Some(Message::DividerPressed)));
        assert_eq!(status, canvas::event::Status::Captured);
    }

    #[test]
    fn press_anywhere_else_is_ignored() {
        // regression: an ungated divider turned every click in the editor
        // into a pane resize (clicking near the left edge snapped the
        // split to its minimum)
        let h = DividerHandle;
        let editor_area = Point::new(60.0, 50.0); // 2nd char of a line
        let (status, message) = h.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            HANDLE,
            Cursor::Available(editor_area),
        );
        assert!(message.is_none());
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn cursor_shape_only_on_the_handle() {
        // regression: iced does not bounds-gate canvas mouse_interaction,
        // so an unconditional ResizingHorizontally put resize arrows over
        // the whole window
        let h = DividerHandle;
        let inside = Cursor::Available(Point::new(203.0, 100.0));
        assert_eq!(
            h.mouse_interaction(&(), HANDLE, inside),
            iced::mouse::Interaction::ResizingHorizontally
        );
        let outside = Cursor::Available(Point::new(100.0, 100.0));
        assert_eq!(
            h.mouse_interaction(&(), HANDLE, outside),
            iced::mouse::Interaction::default()
        );
    }
}
