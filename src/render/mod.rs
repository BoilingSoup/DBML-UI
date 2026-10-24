pub mod text;

/// Diagram rendering backend. Auto-detection order: kitty, sixel, then the
/// plain text renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Kitty,
    Sixel,
    Text,
}
