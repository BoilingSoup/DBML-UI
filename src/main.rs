use tui_db::app::App;

fn main() -> anyhow::Result<()> {
    App::with_sample_source().run()
}
