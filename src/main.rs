//! dbml-ui — desktop DBML diagram editor built on iced.

mod app;
mod canvas;
mod config;
mod editor;
mod theme;
mod view;

fn main() -> Result<(), iced::Error> {
    // dbml-ui [FILE]
    let mut app = match std::env::args().nth(1) {
        Some(path) => {
            let mut app = app::App::default();
            if let Err(e) = app.open_file(&path) {
                eprintln!("cannot open {path}: {e}");
                app::App::default()
            } else {
                app
            }
        }
        None => app::App::blank_start(),
    };
    // persisted preferences (vim/Regular) + recent files
    app.config = config::Config::load();
    let _ = &mut app;
    iced::application("dbml-ui", app::update, view::root)
        .theme(|_| theme::app())
        .subscription(app::subscription)
        .window_size(iced::Size::new(1400.0, 900.0))
        .run_with(|| (app, iced::Task::none()))
}
