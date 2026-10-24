//! Window layout: editor pane | diagram pane, status bar underneath.

use crate::app::{App, Focus, Message};
use crate::canvas::diagram::Diagram;
use crate::canvas::editor::EditorView;
use crate::theme;
use iced::widget::{canvas, column, container, row, text};
use iced::{Color, Element, Fill, Length};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

pub fn root(app: &App) -> Element<'_, Message> {
    let editor_pane = container(canvas(EditorView { app }).width(Fill).height(Fill))
        .padding(1.0)
        .width(Length::FillPortion(2))
        .height(Fill)
        .style(move |_t| panel_style(app.focus == Focus::Editor));

    let diagram_pane = container(
        canvas(Diagram {
            layout: &app.diagram,
            zoom: app.zoom,
            pan: app.pan,
            hover: if app.drag == crate::app::Drag::None {
                app.canvas_cursor
            } else {
                None
            },
            dragging: app.drag != crate::app::Drag::None,
        })
        .width(Fill)
        .height(Fill),
    )
    .padding(1.0)
    .width(Length::FillPortion(3))
    .height(Fill)
    .style(move |_t| panel_style(app.focus == Focus::Canvas));

    column![
        row![editor_pane, diagram_pane].spacing(8).height(Fill),
        status_bar(app),
    ]
    .spacing(8)
    .padding(8)
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

    // leading segment: the cmdline when open, else mode + showcmd
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
    items.push(seg("sample", theme::DIM));
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
