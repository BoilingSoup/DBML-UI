use clap::Parser;
use tui_db::app::App;

/// Terminal DBML diagram editor: vim editing on the left, live diagram on
/// the right.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// DBML file to open
    file: Option<std::path::PathBuf>,
    /// Diagram renderer: auto detects kitty -> sixel -> text fallback
    #[arg(long, value_enum, default_value_t = RenderOpt::Auto)]
    render: RenderOpt,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
enum RenderOpt {
    Auto,
    Kitty,
    Sixel,
    Text,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut app = match &args.file {
        Some(path) => {
            let mut app = App::default();
            let path = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))?;
            app.open_file(path)
                .map_err(|e| anyhow::anyhow!("cannot open {path}: {e}"))?;
            app
        }
        None => App::with_sample_source(),
    };
    app.render_pref = match args.render {
        RenderOpt::Auto => None,
        RenderOpt::Kitty => Some(tui_db::render::Mode::Kitty),
        RenderOpt::Sixel => Some(tui_db::render::Mode::Sixel),
        RenderOpt::Text => Some(tui_db::render::Mode::Text),
    };
    app.run()
}
