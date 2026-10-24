mod app;
mod ui;

fn main() -> anyhow::Result<()> {
    let mut app = app::App::default();
    app.run()
}
