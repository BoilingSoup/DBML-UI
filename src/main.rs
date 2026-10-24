use clap::Parser;
use tui_db::app::App;
use tui_db::render::Mode;

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

    setup_rendering(&mut app, args.render);
    app.run()
}

/// Detect terminal graphics support (kitty -> sixel -> text) or apply the
/// CLI override. Must run before entering the alternate screen.
fn setup_rendering(app: &mut App, pref: RenderOpt) {
    use ratatui_image::picker::{Picker, ProtocolType};

    let queried = Picker::from_query_stdio().ok();
    let mode = match pref {
        RenderOpt::Auto => match queried.as_ref().map(|p| p.protocol_type()) {
            Some(ProtocolType::Kitty) => Mode::Kitty,
            Some(ProtocolType::Sixel) => Mode::Sixel,
            _ => Mode::Text,
        },
        RenderOpt::Kitty => Mode::Kitty,
        RenderOpt::Sixel => Mode::Sixel,
        RenderOpt::Text => Mode::Text,
    };
    app.render_mode = mode;

    // Text mode keeps the crisp cell renderer at zoom 1.0 but zooms via
    // halfblocks through the raster pipeline; prefer a queried picker for
    // real font metrics, else guess a typical cell size.
    let mut picker = queried.unwrap_or_else(|| Picker::from_fontsize((10, 20)));
    picker.set_protocol_type(match mode {
        Mode::Kitty => ProtocolType::Kitty,
        Mode::Sixel => ProtocolType::Sixel,
        Mode::Text => ProtocolType::Halfblocks,
    });
    app.picker = Some(picker);
    if mode != Mode::Text {
        app.image_due = true;
    }
}
