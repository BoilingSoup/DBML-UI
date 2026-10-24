//! dbml-ui — desktop DBML diagram editor built on iced.

mod app;
mod canvas;
mod theme;
mod view;

fn main() -> Result<(), iced::Error> {
    iced::application("dbml-ui", app::update, view::root)
        .theme(|_| theme::app())
        .window_size(iced::Size::new(1400.0, 900.0))
        .run()
}
