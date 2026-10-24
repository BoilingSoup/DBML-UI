pub mod kitty;
pub mod raster;
pub mod svg;
pub mod text;

/// Diagram rendering backend. Auto-detection order: kitty, sixel, then the
/// plain text renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Text,
    Kitty,
    Sixel,
}
