//! Application state and the iced update loop.

use crate::theme;
use dbml_ui::layout::{self, Layout};
use dbml_ui::{dbml, model};

#[derive(Debug, Clone)]
#[allow(dead_code)] // consumed by the interaction milestone
pub enum Message {
    /// Keep the last cursor position for cursor-anchored wheel zoom.
    MouseMoved(iced::Point),
}

pub struct App {
    /// Raw DBML source shown in the editor.
    pub source: String,
    /// Parsed + laid out diagram for the current source.
    pub diagram: Layout,
    #[allow(dead_code)] // consumed by the editor milestone
    pub parse_errors: Vec<dbml::parser::ParseError>,
    /// User-dragged table positions (diagram cells, by table/enum name);
    /// survive re-parses while editing.
    #[allow(dead_code)] // consumed by the interaction milestone
    pub table_pos: std::collections::HashMap<String, (i32, i32)>,
    /// Diagram zoom: pixels per diagram cell unit.
    #[allow(dead_code)] // consumed by the canvas milestone
    pub zoom: f32,
    /// Viewport offset into the zoomed diagram, in pixels.
    #[allow(dead_code)] // consumed by the canvas milestone
    pub pan: iced::Vector,
    /// Last known cursor position within the window (for anchored zoom).
    #[allow(dead_code)] // consumed by the interaction milestone
    pub cursor: iced::Point,
}

impl Default for App {
    fn default() -> Self {
        Self::with_source(dbml_ui::SAMPLE_DBML.to_string())
    }
}

impl App {
    pub fn with_source(source: String) -> Self {
        let mut app = Self {
            source: String::new(),
            diagram: layout::layout(&model::build(&dbml::parser::parse("").doc)),
            parse_errors: Vec::new(),
            table_pos: Default::default(),
            zoom: 20.0,
            pan: iced::Vector::default(),
            cursor: iced::Point::default(),
        };
        app.set_source(&source);
        app
    }

    /// Re-parse + re-layout (keeps dragged table positions).
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        let out = dbml::parser::parse(src);
        let schema = model::build(&out.doc);
        self.diagram = layout::layout_with_overrides(&schema, &self.table_pos);
        self.parse_errors = out.errors;
    }
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::MouseMoved(p) => app.cursor = p,
    }
}

pub fn view(app: &App) -> iced::Element<'_, Message> {
    let _ = app;
    iced::widget::container(iced::widget::text("dbml-ui").color(theme::TEXT))
        .width(iced::Fill)
        .height(iced::Fill)
        .style(|_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme::BG)),
            ..Default::default()
        })
        .into()
}
