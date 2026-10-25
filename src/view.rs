//! Window layout: editor pane | divider | diagram pane, status bar below,
//! help overlay on top.

use crate::app::{App, Drag, Focus, Message};
use crate::canvas::diagram::Diagram;
use crate::canvas::editor::EditorView;
use crate::theme;
use iced::widget::{canvas, column, container, row, stack, text};
use iced::{Color, Element, Fill, Length};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

pub fn root(app: &App) -> Element<'_, Message> {
    let editor_pane = container(canvas(EditorView { app }).width(Fill).height(Fill))
        .padding(1.0)
        .width(Fill)
        .height(Fill)
        .style(move |_t| panel_style(app.focus == Focus::Editor));

    let diagram_pane = container(
        canvas(Diagram {
            layout: &app.diagram,
            zoom: app.zoom,
            pan: app.pan,
            hover: if app.drag == Drag::None {
                app.canvas_cursor
            } else {
                None
            },
            dragging: app.drag != Drag::None,
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

    let panes = row![editor, divider(), diagram].spacing(4).height(Fill);

    let base = column![panes, status_bar(app)].spacing(8).padding(8);

    if app.show_help {
        stack![base, help_overlay()].into()
    } else {
        base.into()
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
        _bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        iced::mouse::Interaction::ResizingHorizontally
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
        line(":w :q :q! :wq :o file :export f.svg", "ex commands"),
        line("Ctrl-s / Ctrl-c", "save / quit"),
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

fn mode_chip(mode: &crate::editor::Mode) -> (&'static str, Color) {
    match mode {
        crate::editor::Mode::Insert => (" INSERT ", Color::from_rgb(0.30, 0.75, 0.95)),
        crate::editor::Mode::Visual => (" VISUAL ", Color::from_rgb(0.98, 0.75, 0.30)),
        crate::editor::Mode::Normal => (" NORMAL ", Color::from_rgb(0.55, 0.90, 0.55)),
    }
}

fn status_bar(app: &App) -> Element<'_, Message> {
    let tables = app
        .diagram
        .boxes
        .iter()
        .filter(|b| b.kind == dbml_ui::model::NodeKind::Table)
        .count();
    let zoom = format!("zoom {:.0}%", app.zoom / BASE_ZOOM * 100.0);
    let errors = format!("errors {}", app.parse_errors.len());
    let (crow, ccol) = app.editor.buf.cursor();

    let seg = |s: &str, color: Color| -> Element<'_, Message> {
        text(s.to_string())
            .size(12.0)
            .color(color)
            .width(Length::Shrink)
            .into()
    };

    // leading segment: the cmdline when open, else mode + showcmd + focus
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let Some(cmd) = &app.editor.cmdline {
        items.push(seg(
            &format!(" {}{}▏ ", cmd.kind, cmd.input),
            iced::Color::BLACK,
        ));
    } else {
        let (mode, color) = mode_chip(&app.editor.mode);
        items.push(
            container(seg(mode, Color::BLACK))
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(color)),
                    border: iced::border::rounded(4),
                    ..Default::default()
                })
                .into(),
        );
        if let Some(sc) = app.editor.showcmd() {
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
            .file
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
}
