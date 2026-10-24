//! dbml-ui — desktop DBML diagram editor built on iced.

mod app;
mod theme;

fn main() -> Result<(), iced::Error> {
    iced::application("dbml-ui", app::update, app::view)
        .theme(|_| theme::app())
        .run()
}
