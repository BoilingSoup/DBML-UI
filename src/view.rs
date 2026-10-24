//! Window layout: editor pane | diagram pane, status bar underneath.

use crate::app::{App, Message};
use crate::canvas::diagram::Diagram;
use crate::theme;
use iced::widget::{canvas, column, container, row, text};
use iced::{Color, Element, Fill, Length};

/// Zoom at which one diagram cell maps 1:1 to a text line (~20px).
pub const BASE_ZOOM: f32 = 20.0;

pub fn root(app: &App) -> Element<'_, Message> {
    let editor_pane = container(
        text("vim editor — coming in M4")
            .color(theme::DIM)
            .size(14.0),
    )
    .padding(12.0)
    .width(Length::FillPortion(2))
    .height(Fill)
    .style(|_t| panel_style());

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
    .style(|_t| panel_style());

    column![
        row![editor_pane, diagram_pane].spacing(8).height(Fill),
        status_bar(app),
    ]
    .spacing(8)
    .padding(8)
    .into()
}

fn panel_style() -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme::PANEL)),
        border: iced::border::rounded(8).color(theme::BORDER),
        ..Default::default()
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

    let seg = |s: &str, color: Color| -> Element<'_, Message> {
        text(s.to_string())
            .size(12.0)
            .color(color)
            .width(Length::Shrink)
            .into()
    };

    container(
        row![
            seg("dbml-ui", theme::TEXT),
            seg("·", theme::DIM),
            seg("sample", theme::DIM),
            seg("·", theme::DIM),
            seg(&format!("tables {tables}"), theme::DIM),
            seg("·", theme::DIM),
            seg(&zoom, theme::DIM),
            seg("·", theme::DIM),
            seg(&errors, theme::DIM),
        ]
        .spacing(8),
    )
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
