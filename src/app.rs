use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::dbml::parser::{self, ParseError};
use crate::layout::{self, Layout};
use crate::model;
use crate::ui;

/// Startup document for a fresh session.
pub const SAMPLE_DBML: &str = r#"Project blog {
  database_type: 'PostgreSQL'
  Note: 'A small blog schema'
}

Table users as U [note: 'Registered accounts'] {
  id integer [pk, increment]
  username varchar(80) [not null, unique]
  email varchar(255) [not null]
  role varchar [default: 'member']
  created_at timestamp [default: `now()`]

  indexes {
    (username, email) [unique, name: 'idx_users_contact']
  }
}

Table posts {
  id integer [pk]
  user_id integer [not null, ref: > U.id]
  status post_status [not null, default: 'draft']
  title varchar(200) [not null]
  body text
  published_at timestamp
}

Table comments {
  id integer [pk]
  post_id integer [not null, ref: > posts.id]
  author_id integer [not null, ref: > U.id]
  body text [not null]
}

Enum post_status {
  draft
  published
  archived
}

Ref: comments.author_id > users.id
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Editor,
    Canvas,
}

/// A table drag in progress.
#[derive(Debug, Clone)]
pub struct TableDrag {
    pub title: String,
    /// Grab offset in diagram cells (cursor - box origin).
    pub grab: (i32, i32),
    /// Press position, for click-vs-drag discrimination.
    pub origin: (u16, u16),
    /// Live position while dragging (committed on release in image modes).
    pub pending: Option<(i32, i32)>,
}

/// Screen rectangles of the main regions, refreshed each frame. Used for
/// hit-testing mouse events between draws.
#[derive(Debug, Default, Clone, Copy)]
pub struct Panes {
    pub body: Rect,
    pub editor: Rect,
    pub divider: Rect,
    pub canvas: Rect,
}

pub struct App {
    pub should_quit: bool,
    pub focus: Focus,
    pub split: f32,
    pub divider_dragging: bool,
    /// Canvas pan offset in cells; positive shifts diagram content right/down.
    pub pan: (i32, i32),
    pub pan_anchor: Option<(u16, u16)>,
    pub last_mouse: (u16, u16),
    pub panes: Panes,
    pub source: String,
    pub diagram: Option<Layout>,
    pub parse_errors: Vec<ParseError>,
    pub dangling: usize,
    pub editor: crate::editor::Editor,
    /// Text changed since the last re-parse; debounce timer anchor.
    pub dirty: bool,
    pub last_edit: Option<Instant>,
    /// Transient status message (command feedback, errors).
    pub message: Option<String>,
    /// File backing the buffer, if any.
    pub file: Option<std::path::PathBuf>,
    /// Buffer content as of the last load/save, for the modified flag.
    pub saved_text: String,
    // --- image-mode rendering state ---
    pub render_mode: crate::render::Mode,
    pub picker: Option<ratatui_image::picker::Picker>,
    /// Image zoom; 1.0 draws one diagram cell per terminal cell.
    pub zoom: f32,
    /// Viewport offset into the zoomed raster, in raster pixels.
    pub img_pan: (i32, i32),
    pub full_raster: Option<image::RgbaImage>,
    /// Layout generation the raster was built for.
    raster_gen: u64,
    layout_gen: u64,
    pub proto: Option<ratatui_image::protocol::Protocol>,
    /// Crop + protocol rebuild needed (layout/zoom/pan/area changed).
    pub image_due: bool,
    last_tx: Option<Instant>,
    /// Canvas inner size in pixels, tracked to detect resizes.
    pub canvas_px: (u32, u32),
    /// Press position for click-vs-drag discrimination on the canvas.
    click_origin: Option<(u16, u16)>,
    /// Scrollbar drag in flight: Some(true) vertical, Some(false) horizontal.
    sb_drag: Option<bool>,
    /// User-dragged table positions (diagram coords, by table/enum name);
    /// survive re-parses while editing.
    pub table_pos: std::collections::HashMap<String, (i32, i32)>,
    /// Table drag in flight. In image modes the position is only applied on
    /// release (`pending`); per-tick re-transmission would flood the
    /// terminal, so the UI draws a cheap outline instead.
    pub table_drag: Option<TableDrag>,
    /// Help overlay visibility.
    pub show_help: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            ..Self::empty()
        }
    }
}

impl App {
    /// All-zero state except defaults that must not be zero.
    fn empty() -> Self {
        Self {
            should_quit: false,
            // editor/canvas width fraction (f32's derived default would be 0)
            split: 0.4,
            focus: Focus::default(),
            divider_dragging: false,
            pan: (0, 0),
            pan_anchor: None,
            last_mouse: (0, 0),
            panes: Panes::default(),
            source: String::new(),
            diagram: None,
            parse_errors: Vec::new(),
            dangling: 0,
            editor: crate::editor::Editor::default(),
            dirty: false,
            last_edit: None,
            message: None,
            file: None,
            saved_text: String::new(),
            render_mode: crate::render::Mode::Text,
            picker: None,
            zoom: 1.0,
            img_pan: (0, 0),
            full_raster: None,
            raster_gen: 0,
            layout_gen: 0,
            proto: None,
            image_due: false,
            last_tx: None,
            canvas_px: (0, 0),
            click_origin: None,
            sb_drag: None,
            table_pos: std::collections::HashMap::new(),
            table_drag: None,
            show_help: false,
        }
    }
}

const DEBOUNCE: Duration = Duration::from_millis(120);
/// Minimum interval between image crop/transmit rebuilds while panning.
const IMAGE_TX_INTERVAL: Duration = Duration::from_millis(33);

impl App {
    pub fn is_modified(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    /// Raster pixels per terminal cell for the image pipeline. Kitty/sixel
    /// get real font pixels. The halfblocks encoder (text-mode zoom) decimates
    /// everything to 1x2 pseudo-pixels per cell, so the raster is built
    /// directly at that resolution — vector-crisp edges instead of a blurry
    /// downsample of a font-sized raster.
    pub fn font_px(&self) -> (u32, u32) {
        if self.render_mode == crate::render::Mode::Text {
            (1, 2)
        } else {
            self.picker
                .as_ref()
                .map(|p| (p.font_size().0 as u32, p.font_size().1 as u32))
                .unwrap_or((10, 20))
        }
    }

    /// Canvas inner rect (inside the block border).
    /// Diagram viewport: canvas inner minus the block border AND the
    /// scrollbar gutters (right column + bottom row).
    pub fn view_rect(&self) -> Rect {
        let c = self.panes.canvas;
        Rect {
            x: c.x.saturating_add(1),
            y: c.y.saturating_add(1),
            width: c.width.saturating_sub(3),
            height: c.height.saturating_sub(3),
        }
    }

    /// Vertical scrollbar geometry: (content, viewport, offset). Content and
    /// viewport in diagram cells (text mode) or raster pixels (image mode).
    pub fn scroll_v(&self) -> (u32, u32, i32) {
        match (&self.diagram, self.is_image_mode()) {
            (Some(l), false) => (
                (l.extent.1 + 1) as u32,
                self.view_rect().height as u32,
                self.pan.1,
            ),
            _ => (
                self.full_raster.as_ref().map_or(1, |i| i.height()),
                self.canvas_px.1,
                self.img_pan.1,
            ),
        }
    }

    /// Horizontal scrollbar geometry; see `scroll_v`.
    pub fn scroll_h(&self) -> (u32, u32, i32) {
        match (&self.diagram, self.is_image_mode()) {
            (Some(l), false) => (
                (l.extent.0 + 1) as u32,
                self.view_rect().width as u32,
                self.pan.0,
            ),
            _ => (
                self.full_raster.as_ref().map_or(1, |i| i.width()),
                self.canvas_px.0,
                self.img_pan.0,
            ),
        }
    }

    /// Rebuild the full raster (if stale) and re-crop/re-transmit the
    /// viewport. Called from the event loop, throttled.
    pub fn refresh_image(&mut self) {
        use crate::render::{raster, svg};
        let Some(picker) = &self.picker else { return };
        let Some(layout) = &self.diagram else { return };
        let inner = self.view_rect();
        let (fw, fh) = self.font_px();
        let aw = (inner.width as u32).saturating_mul(fw).max(1);
        let ah = (inner.height as u32).saturating_mul(fh).max(1);
        self.canvas_px = (aw, ah);

        let want_w =
            (((layout.extent.0 as f32 + 2.0 * svg::MARGIN_CELLS) * fw as f32) * self.zoom) as u32;
        let want_h =
            (((layout.extent.1 as f32 + 2.0 * svg::MARGIN_CELLS) * fh as f32) * self.zoom) as u32;
        let want_w = want_w.max(aw);
        let want_h = want_h.max(ah);
        let stale = self.raster_gen != self.layout_gen
            || self
                .full_raster
                .as_ref()
                .is_none_or(|i| i.dimensions() != (want_w, want_h));
        if stale {
            // Text-mode zoom renders a label-free outline: at 1x2
            // pseudo-pixels per cell, labels are unreadable noise anyway.
            let doc = svg::build_svg_with(layout, self.render_mode != crate::render::Mode::Text);
            let (sw, sh) = svg::svg_size(layout);
            match raster::rasterize(&doc, want_w, want_h, want_w as f32 / sw, want_h as f32 / sh) {
                Ok(img) => {
                    self.full_raster = Some(img);
                    self.raster_gen = self.layout_gen;
                }
                Err(e) => {
                    self.message = Some(format!("image render failed: {e}"));
                    return;
                }
            }
        }

        let Some(full) = &self.full_raster else {
            return;
        };
        // clamp pan to the raster
        let max_x = (full.width().saturating_sub(aw)) as i32;
        let max_y = (full.height().saturating_sub(ah)) as i32;
        self.img_pan.0 = self.img_pan.0.clamp(0, max_x.max(0));
        self.img_pan.1 = self.img_pan.1.clamp(0, max_y.max(0));

        // crop the viewport and pad to the exact area size so the protocol
        // maps 1 image px row per cell row without resampling; the pad is
        // opaque because the halfblocks encoder drops alpha
        let fx = (self.img_pan.0.max(0) as u32).min(full.width());
        let fy = (self.img_pan.1.max(0) as u32).min(full.height());
        let cw = (full.width() - fx).min(aw);
        let ch = (full.height() - fy).min(ah);
        let bg = svg::BG_RGBA;
        let mut view = image::RgbaImage::from_pixel(aw, ah, image::Rgba([bg.0, bg.1, bg.2, bg.3]));
        let crop = image::imageops::crop_imm(full, fx, fy, cw, ch);
        image::imageops::overlay(&mut view, &crop.to_image(), 0, 0);

        let proto = if self.render_mode == crate::render::Mode::Text {
            // The halfblocks raster is in 1x2-per-cell units, not font
            // pixels; `new_protocol` + `Resize::Fit` would place it at
            // image_px / font_px (a tiny patch). Encode directly at the
            // full view area instead — the image is already exactly
            // area.width x area.height*2, so the internal resize is a
            // no-op and edges stay vector-crisp.
            ratatui_image::protocol::halfblocks::Halfblocks::new(
                image::DynamicImage::ImageRgba8(view),
                inner,
            )
            .map(ratatui_image::protocol::Protocol::Halfblocks)
        } else {
            picker.new_protocol(
                image::DynamicImage::ImageRgba8(view),
                inner,
                ratatui_image::Resize::Fit(None),
            )
        };
        match proto {
            Ok(proto) => self.proto = Some(proto),
            Err(e) => self.message = Some(format!("image protocol failed: {e}")),
        }
    }

    /// Zoom by one step, keeping the point under the cursor stationary.
    /// Text mode zooms out only (halfblocks minimap): the cell renderer is
    /// already at the terminal's native resolution, so magnifying it through
    /// a 1x2-pseudo-pixel raster can only degrade it.
    pub fn zoom_at(&mut self, col: u16, row: u16, dir: i32) {
        let old = self.zoom;
        let max = if self.render_mode == crate::render::Mode::Text {
            1.0
        } else {
            8.0
        };
        let new = (old * 1.25f32.powi(dir)).clamp(0.25, max);
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        let inner = self.view_rect();
        let (fw, fh) = self.font_px();
        let cx = col.saturating_sub(inner.x) as f32 * fw as f32;
        let cy = row.saturating_sub(inner.y) as f32 * fh as f32;
        let s = new / old;
        self.img_pan.0 = ((self.img_pan.0 as f32 + cx) * s - cx) as i32;
        self.img_pan.1 = ((self.img_pan.1 as f32 + cy) * s - cy) as i32;
        self.zoom = new;
        // Landing back on exactly 1.0 in text mode: the halfblocks raster is
        // 1 px per cell there, so carry the pan over to the cell renderer.
        if self.render_mode == crate::render::Mode::Text && (new - 1.0).abs() < f32::EPSILON {
            self.pan = (self.img_pan.0, self.img_pan.1);
        }
        self.image_due = true;
    }

    /// Re-run the pipeline for the current source (keeps dragged positions).
    pub fn relayout(&mut self) {
        let src = self.source.clone();
        self.set_source(&src);
    }

    /// Diagram coordinates under a screen position (inverse of the
    /// renderers' transforms).
    fn cursor_diagram_pos(&self, col: u16, row: u16) -> (i32, i32) {
        let view = self.view_rect();
        match self.is_image_mode() {
            false => (
                col as i32 - view.x as i32 - self.pan.0,
                row as i32 - view.y as i32 - self.pan.1,
            ),
            _ => {
                let (fw, fh) = self.font_px();
                let m = crate::render::svg::MARGIN_CELLS;
                let fx = (col as i32 - view.x as i32) as f32 * fw as f32 + self.img_pan.0 as f32;
                let fy = (row as i32 - view.y as i32) as f32 * fh as f32 + self.img_pan.1 as f32;
                (
                    (fx / (fw as f32 * self.zoom)).round() as i32 - m as i32,
                    (fy / (fh as f32 * self.zoom)).round() as i32 - m as i32,
                )
            }
        }
    }

    /// Screen position of a diagram coordinate (forward of the renderers'
    /// transforms). Inverse of `cursor_diagram_pos`.
    pub fn diagram_to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        let view = self.view_rect();
        match self.is_image_mode() {
            false => (
                view.x as i32 + x + self.pan.0,
                view.y as i32 + y + self.pan.1,
            ),
            _ => {
                let (fw, fh) = self.font_px();
                let m = crate::render::svg::MARGIN_CELLS as i32;
                let px = (x + m) as f32 * fw as f32 * self.zoom - self.img_pan.0 as f32;
                let py = (y + m) as f32 * fh as f32 * self.zoom - self.img_pan.1 as f32;
                (
                    view.x as i32 + (px / fw as f32) as i32,
                    view.y as i32 + (py / fh as f32) as i32,
                )
            }
        }
    }

    /// If the press lands on a scrollbar track, jump the view to that
    /// position and begin a thumb drag. Returns true when consumed.
    fn try_scrollbar_press(&mut self, col: u16, row: u16) -> bool {
        let c = self.panes.canvas;
        let inner = Rect {
            x: c.x.saturating_add(1),
            y: c.y.saturating_add(1),
            width: c.width.saturating_sub(2),
            height: c.height.saturating_sub(2),
        };
        let view = self.view_rect();
        let bars = crate::ui::scrollbar_rects(self, inner, view);
        let vertical = match (bars.v, bars.h) {
            (Some((vx, _, _)), _) if vx == col && row < view.bottom() => Some(true),
            (_, Some((hy, _, _))) if hy == row && col < view.right() => Some(false),
            _ => None,
        };
        match vertical {
            Some(v) => {
                self.sb_drag = Some(v);
                let frac = self.track_frac(v, if v { row } else { col });
                self.scroll_to(v, frac);
                true
            }
            None => false,
        }
    }

    /// Track fraction (0..=1) for a position along a scrollbar track. The
    /// first and last track cells map exactly to 0.0 and 1.0.
    fn track_frac(&self, vertical: bool, pos: u16) -> f32 {
        let c = self.panes.canvas;
        let (start, len) = if vertical {
            (c.y.saturating_add(1), c.height.saturating_sub(3).max(1))
        } else {
            (c.x.saturating_add(1), c.width.saturating_sub(3).max(1))
        };
        let span = (len - 1).max(1) as f32;
        ((pos.saturating_sub(start)) as f32 / span).clamp(0.0, 1.0)
    }

    /// Scroll the canvas so the thumb sits at `frac` along the track.
    fn scroll_to(&mut self, vertical: bool, frac: f32) {
        let (content, viewport, _) = if vertical {
            self.scroll_v()
        } else {
            self.scroll_h()
        };
        let off = crate::ui::offset_at(content, viewport, frac);
        if self.is_image_mode() {
            if vertical {
                self.img_pan.1 = off;
            } else {
                self.img_pan.0 = off;
            }
            self.image_due = true;
        } else if vertical {
            self.pan.1 = off;
        } else {
            self.pan.0 = off;
        }
    }

    /// Find the table box under a screen position, if any.
    pub fn table_at(&self, col: u16, row: u16) -> Option<&crate::layout::TableBox> {
        let layout = self.diagram.as_ref()?;
        let inner = self.view_rect();
        let (cx, cy) = (col as i32, row as i32);
        // reverse: later boxes are drawn last, so the topmost wins overlaps
        for b in layout.boxes.iter().rev() {
            let (bx, by, bw, bh) = match if self.is_image_mode() {
                crate::render::Mode::Kitty // any non-cell pipeline
            } else {
                crate::render::Mode::Text
            } {
                crate::render::Mode::Text => (
                    inner.x as i32 + b.x + self.pan.0,
                    inner.y as i32 + b.y + self.pan.1,
                    b.w,
                    b.h,
                ),
                _ => {
                    let (fw, fh) = self.font_px();
                    let m = crate::render::svg::MARGIN_CELLS;
                    let px = ((b.x as f32 + m) * fw as f32 * self.zoom) as i32 - self.img_pan.0;
                    let py = ((b.y as f32 + m) * fh as f32 * self.zoom) as i32 - self.img_pan.1;
                    let sx = inner.x as i32 + (px as f32 / fw as f32) as i32;
                    let sy = inner.y as i32 + (py as f32 / fh as f32) as i32;
                    (
                        sx,
                        sy,
                        (b.w as f32 * self.zoom) as i32,
                        (b.h as f32 * self.zoom) as i32,
                    )
                }
            };
            if cx >= bx && cx < bx + bw && cy >= by && cy < by + bh {
                return Some(b);
            }
        }
        None
    }

    /// Click-to-jump: move the editor cursor to a table's definition.
    fn jump_to_table_at(&mut self, col: u16, row: u16) {
        let Some((title, span)) = self
            .table_at(col, row)
            .and_then(|b| b.name_span.clone().map(|span| (b.title.clone(), span)))
        else {
            return;
        };
        let (line, c) = parser::line_col_at(&self.source, span.start);
        self.editor.jump(line, c);
        self.focus = Focus::Editor;
        self.message = Some(format!("jumped to {title}"));
    }

    /// Re-parse the document and rebuild the diagram preview.
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        let out = parser::parse(src);
        let schema = model::build(&out.doc);
        self.diagram = Some(layout::layout_with_overrides(&schema, &self.table_pos));
        self.parse_errors = out.errors;
        self.dangling = schema.dangling;
        self.layout_gen += 1;
        if self.is_image_mode() {
            self.image_due = true;
        }
    }

    /// Open a file into the editor.
    pub fn open_file(&mut self, path: &str) -> std::io::Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.table_pos.clear();
        self.file = Some(std::path::PathBuf::from(path));
        self.saved_text = content.clone();
        self.editor.set_text(&content);
        self.set_source(&content);
        self.message = Some(format!(
            "\"{path}\" {}L, {}B",
            content.lines().count(),
            content.len()
        ));
        Ok(())
    }

    /// Write the buffer to disk.
    pub fn save_file(&mut self, path: Option<&str>) -> std::io::Result<()> {
        let target = match path {
            Some(p) => {
                self.file = Some(std::path::PathBuf::from(p));
                std::path::PathBuf::from(p)
            }
            None => match self.file.clone() {
                Some(f) => f,
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "no file name (use :w <file>)",
                    ))
                }
            },
        };
        let text = self.editor.text();
        std::fs::write(&target, &text)?;
        self.saved_text = text;
        self.message = Some(format!(
            "\"{}\" written, {}B",
            target.display(),
            self.saved_text.len()
        ));
        Ok(())
    }

    /// Export the current diagram to `.svg` or `.png` (2x scale).
    pub fn export_diagram(&self, path: &str) -> anyhow::Result<String> {
        use crate::render::{raster, svg};
        let layout = self
            .diagram
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("nothing to export"))?;
        if path.ends_with(".png") {
            let doc = svg::build_svg(layout);
            let (sw, sh) = svg::svg_size(layout);
            let scale = 2.0;
            let img =
                raster::rasterize(&doc, (sw * scale) as u32, (sh * scale) as u32, scale, scale)?;
            img.save(path)?;
        } else {
            std::fs::write(path, svg::build_svg(layout))?;
        }
        Ok(format!("exported {path}"))
    }

    /// Execute an ex command (already stripped of ':').
    pub fn handle_command(&mut self, input: &str) {
        use crate::commands::ExCommand;
        match crate::commands::parse_ex(input) {
            ExCommand::Write(path) => {
                if let Err(e) = self.save_file(path.as_deref()) {
                    self.message = Some(format!(":w failed: {e}"));
                }
            }
            ExCommand::Quit { force } => {
                if self.is_modified() && !force {
                    self.message = Some("No write since last change (use :w or :q!)".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::WriteQuit => {
                if self.file.is_none() {
                    self.message = Some("no file name (use :w <file>)".to_string());
                } else if let Err(e) = self.save_file(None) {
                    self.message = Some(format!(":wq failed: {e}"));
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::Open { path, force } => {
                if self.is_modified() && !force {
                    self.message =
                        Some("No write since last change (use :e! or :w first)".to_string());
                } else if let Err(e) = self.open_file(&path) {
                    self.message = Some(format!("cannot open \"{path}\": {e}"));
                }
            }
            ExCommand::Export(arg) => {
                let path = if arg.is_empty() {
                    "diagram.svg".to_string()
                } else {
                    arg
                };
                match self.export_diagram(&path) {
                    Ok(msg) => self.message = Some(msg),
                    Err(e) => self.message = Some(format!("export failed: {e}")),
                }
            }
            ExCommand::Unknown(why) => self.message = Some(why),
        }
    }

    pub fn with_sample_source() -> Self {
        let mut app = Self {
            editor: crate::editor::Editor::new(SAMPLE_DBML),
            ..Self::default()
        };
        app.set_source(SAMPLE_DBML);
        app
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> anyhow::Result<()> {
        while !self.should_quit {
            terminal.draw(|f| ui::draw(f, self))?;
            let timeout = match (self.dirty, self.last_edit) {
                (true, Some(t)) => DEBOUNCE.saturating_sub(t.elapsed()),
                _ => Duration::from_millis(50),
            };
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                    Event::Mouse(mouse) => self.on_mouse(mouse),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            if self.dirty && self.last_edit.is_none_or(|t| t.elapsed() >= DEBOUNCE) {
                self.set_source(&self.editor.text());
                self.dirty = false;
                self.last_edit = None;
            }
            // While a drag is active, cut emissions to ~10/s: each one is a
            // multi-MB image transmission and terminals fall behind quickly.
            let interval = if self.pan_anchor.is_some() || self.table_drag.is_some() {
                Duration::from_millis(100)
            } else {
                IMAGE_TX_INTERVAL
            };
            let tx_ok = self.last_tx.is_none_or(|t| t.elapsed() >= interval);
            if self.image_due && tx_ok {
                self.refresh_image();
                self.image_due = false;
                self.last_tx = Some(Instant::now());
            }
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('s') => {
                    if let Err(e) = self.save_file(None) {
                        self.message = Some(format!("save failed: {e}"));
                    }
                    return;
                }
                _ => {}
            }
        }
        // The command-line prompt always receives keys, regardless of focus.
        if self.editor.cmdline.is_some() {
            self.dispatch_editor_key(key);
            return;
        }
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                Focus::Editor => Focus::Canvas,
                Focus::Canvas => Focus::Editor,
            };
            return;
        }
        let typing = self.focus == Focus::Editor && self.editor.mode == crate::editor::Mode::Insert;
        if key.code == KeyCode::Char('?') && !typing {
            self.show_help = !self.show_help;
            return;
        }
        if self.show_help && key.code != KeyCode::Tab {
            // any other key closes the overlay first
            self.show_help = false;
            return;
        }
        match self.focus {
            Focus::Editor => self.dispatch_editor_key(key),
            Focus::Canvas => self.on_canvas_key(key),
        }
    }

    fn dispatch_editor_key(&mut self, key: KeyEvent) {
        match self.editor.handle_key(key) {
            crate::editor::Action::Changed => {
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            crate::editor::Action::Command(cmd) => self.handle_command(&cmd),
            crate::editor::Action::None => {}
        }
    }

    /// True when the diagram renders through the raster pipeline: kitty,
    /// sixel, or text mode zoomed away from 1.0 (halfblocks).
    pub fn is_image_mode(&self) -> bool {
        self.render_mode != crate::render::Mode::Text || (self.zoom - 1.0).abs() > f32::EPSILON
    }

    fn on_canvas_key(&mut self, key: KeyEvent) {
        // the command line is reachable without switching focus back
        if matches!(key.code, KeyCode::Char(':') | KeyCode::Char('/'))
            && self.editor.mode == crate::editor::Mode::Normal
        {
            self.dispatch_editor_key(key);
            return;
        }
        if self.is_image_mode() {
            let (fw, fh) = self.font_px();
            let (dx, dy) = match key.code {
                KeyCode::Char('h') | KeyCode::Left => (-1, 0),
                KeyCode::Char('l') | KeyCode::Right => (1, 0),
                KeyCode::Char('j') | KeyCode::Down => (0, 1),
                KeyCode::Char('k') | KeyCode::Up => (0, -1),
                KeyCode::Char('H') => (-8, 0),
                KeyCode::Char('L') => (8, 0),
                KeyCode::Char('0') => {
                    self.img_pan = (0, 0);
                    self.image_due = true;
                    return;
                }
                _ => return,
            };
            self.img_pan.0 -= dx * fw as i32;
            self.img_pan.1 -= dy * fh as i32;
            self.image_due = true;
            return;
        }
        let (dx, dy) = match key.code {
            KeyCode::Char('h') | KeyCode::Left => (-4, 0),
            KeyCode::Char('l') | KeyCode::Right => (4, 0),
            KeyCode::Char('j') | KeyCode::Down => (0, 1),
            KeyCode::Char('k') | KeyCode::Up => (0, -1),
            KeyCode::Char('H') => (-16, 0),
            KeyCode::Char('L') => (16, 0),
            KeyCode::Char('0') => {
                self.pan = (0, 0);
                return;
            }
            _ => return,
        };
        self.pan.0 += dx;
        self.pan.1 += dy;
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let (col, row) = (mouse.column, mouse.row);
        self.last_mouse = (col, row);
        let panes = self.panes;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if col >= panes.divider.left().saturating_sub(0)
                    && col <= panes.divider.right().saturating_add(0)
                    && row >= panes.body.top()
                {
                    self.divider_dragging = true;
                } else if rect_contains(panes.canvas, col, row)
                    && !self.try_scrollbar_press(col, row)
                {
                    self.focus = Focus::Canvas;
                    // pressing a table box drags the table; empty space pans
                    let hit = self.table_at(col, row).map(|b| (b.title.clone(), b.x, b.y));
                    if let Some((title, bx, by)) = hit {
                        let (dx, dy) = self.cursor_diagram_pos(col, row);
                        self.table_drag = Some(TableDrag {
                            title,
                            grab: (dx - bx, dy - by),
                            origin: (col, row),
                            pending: None,
                        });
                    } else {
                        self.pan_anchor = Some((col, row));
                    }
                    self.click_origin = Some((col, row));
                } else if rect_contains(panes.editor, col, row) {
                    self.focus = Focus::Editor;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = self.table_drag.clone() {
                    let moved = (col as i32 - drag.origin.0 as i32).abs() > 0
                        || (row as i32 - drag.origin.1 as i32).abs() > 0;
                    if moved {
                        let (dx, dy) = self.cursor_diagram_pos(col, row);
                        let pos = ((dx - drag.grab.0).max(0), (dy - drag.grab.1).max(0));
                        if self.is_image_mode() {
                            // re-rasterizing per tick would flood the terminal
                            // with multi-MB transmissions; commit on release
                            self.table_drag = Some(TableDrag {
                                pending: Some(pos),
                                ..drag
                            });
                        } else {
                            self.table_pos.insert(drag.title, pos);
                            self.relayout();
                        }
                    }
                } else if let Some(vertical) = self.sb_drag {
                    let frac = self.track_frac(vertical, if vertical { row } else { col });
                    self.scroll_to(vertical, frac);
                } else if self.divider_dragging {
                    if panes.body.width > 0 {
                        let rel = (col.saturating_sub(panes.body.left())) as f32
                            / panes.body.width as f32;
                        self.split = rel.clamp(0.15, 0.85);
                    }
                } else if let Some((last_col, last_row)) = self.pan_anchor {
                    if self.is_image_mode() {
                        let (fw, fh) = self.font_px();
                        self.img_pan.0 -= (col as i32 - last_col as i32) * fw as i32;
                        self.img_pan.1 -= (row as i32 - last_row as i32) * fh as i32;
                        self.image_due = true;
                    } else {
                        self.pan.0 += col as i32 - last_col as i32;
                        self.pan.1 += row as i32 - last_row as i32;
                    }
                    self.pan_anchor = Some((col, row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.divider_dragging = false;
                self.sb_drag = None;
                if let Some(drag) = self.table_drag.take() {
                    if let Some(pos) = drag.pending {
                        self.table_pos.insert(drag.title, pos);
                        self.relayout();
                    }
                }
                // click (no drag) on a table box jumps the editor to it
                if let Some((dc, dr)) = self.click_origin {
                    if (col as i32 - dc as i32).abs() <= 1 && (row as i32 - dr as i32).abs() <= 1 {
                        self.jump_to_table_at(col, row);
                    }
                }
                self.pan_anchor = None;
                self.click_origin = None;
            }
            MouseEventKind::ScrollUp => {
                if rect_contains(panes.canvas, col, row) {
                    self.zoom_at(col, row, 1);
                } else if rect_contains(panes.editor, col, row) {
                    self.editor.scroll(-2);
                }
            }
            MouseEventKind::ScrollDown => {
                if rect_contains(panes.canvas, col, row) {
                    self.zoom_at(col, row, -1);
                } else if rect_contains(panes.editor, col, row) {
                    self.editor.scroll(2);
                }
            }
            _ => {}
        }
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.left() && col < rect.right() && row >= rect.top() && row < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Action;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    #[test]
    fn typed_table_flows_to_diagram() {
        let mut app = App::with_sample_source();
        assert_eq!(app.diagram.as_ref().unwrap().boxes.len(), 4);

        // G o  Table tmp { id integer [pk] }  Esc
        assert_eq!(app.editor.handle_key(key('G')), Action::None);
        assert_eq!(app.editor.handle_key(key('o')), Action::Changed);
        for c in "Table tmp { id integer [pk] }".chars() {
            assert_eq!(app.editor.handle_key(key(c)), Action::Changed);
        }
        assert_eq!(app.editor.handle_key(esc()), Action::None);

        // what the debounce handler runs:
        app.set_source(&app.editor.text());
        assert!(app.parse_errors.is_empty(), "{:?}", app.parse_errors);
        assert_eq!(app.diagram.as_ref().unwrap().boxes.len(), 5);
        assert!(app
            .diagram
            .as_ref()
            .unwrap()
            .boxes
            .iter()
            .any(|b| b.title == "tmp"));
    }

    #[test]
    fn image_pipeline_builds_raster_and_protocol() {
        use ratatui_image::picker::{Picker, ProtocolType};
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        app.picker = Some(picker);
        app.render_mode = crate::render::Mode::Kitty;

        app.refresh_image();
        assert!(app.full_raster.is_some(), "raster built");
        let (w, h) = app.full_raster.as_ref().unwrap().dimensions();
        assert!(
            w >= 57 * 10 && h >= 35 * 20,
            "raster covers viewport: {w}x{h}"
        );
        assert!(app.proto.is_some(), "protocol created");
        assert_eq!(app.canvas_px, (57 * 10, 35 * 20));

        // zoom keeps the cursor point stationary and requests a refresh
        app.zoom_at(50, 10, 1);
        assert!(
            (app.zoom - 1.25).abs() < 1e-6,
            "zoom={} due={}",
            app.zoom,
            app.image_due
        );
        assert!(app.image_due);

        // pan stays clamped to the (now larger, zoomed) raster after refresh
        app.img_pan = (i32::MAX, i32::MAX);
        app.refresh_image();
        let (w2, h2) = app.full_raster.as_ref().unwrap().dimensions();
        assert!(
            w2 > w && h2 >= h,
            "zoom enlarged raster: {w}x{h} -> {w2}x{h2}"
        );
        assert!(
            app.img_pan.0 <= w2 as i32 - 57 * 10 && app.img_pan.1 <= h2 as i32 - 35 * 20,
            "clamped: pan={:?} raster={w2}x{h2}",
            app.img_pan
        );
    }

    #[test]
    fn click_jumps_to_table_definition() {
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        // "users" is the first box at diagram (0,0)
        let inner = app.view_rect();
        let hit = app.table_at(inner.x, inner.y);
        assert!(hit.is_some(), "box at canvas origin");
        assert_eq!(hit.unwrap().title, "users");

        let before = app.editor.cursor();
        app.jump_to_table_at(inner.x, inner.y);
        let after = app.editor.cursor();
        assert_ne!(before, after);
        let (line, col) = after;
        assert_eq!(app.editor.ta.lines()[line].get(col..col + 5), Some("users"));
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn export_writes_svg_and_png() {
        let dir = std::env::temp_dir().join(format!("tui-db-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = App::with_sample_source();
        let svg_path = dir.join("d.svg");
        app.export_diagram(svg_path.to_str().unwrap()).unwrap();
        let svg = std::fs::read_to_string(&svg_path).unwrap();
        assert!(svg.starts_with("<svg"));

        let png_path = dir.join("d.png");
        app.export_diagram(png_path.to_str().unwrap()).unwrap();
        let png = std::fs::read(&png_path).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(png.len() > 10_000, "non-trivial png: {} bytes", png.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scrollbar_press_and_drag_pans() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::with_sample_source();
        // small canvas so the diagram overflows vertically
        app.panes.canvas = Rect::new(40, 0, 30, 12); // inner 28x10, view 27x9
        let view = app.view_rect();
        assert!(view.height == 9 && view.width == 27);
        let vx = view.right(); // vertical track column

        let down = |c, r| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let drag = |c, r| MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let up = |c, r| MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };

        let (_, content, viewport) = app.scroll_v();
        assert!(
            content > viewport as u32,
            "diagram must overflow the small canvas"
        );
        let max_off = content - viewport as u32;

        // press mid-track: view jumps halfway
        let mid_row = view.y + view.height / 2;
        app.on_mouse(down(vx, mid_row));
        assert_eq!(app.sb_drag, Some(true));
        let half = (max_off as f32 / 2.0).round() as i32;
        assert!(
            (app.pan.1 - half).abs() <= 1,
            "pan {} ~ {}",
            app.pan.1,
            half
        );

        // drag to the track end: max offset
        let end_row = view.bottom() - 1;
        app.on_mouse(drag(vx, end_row));
        assert_eq!(app.pan.1, max_off as i32);

        // release clears the drag mode
        app.on_mouse(up(vx, end_row));
        assert_eq!(app.sb_drag, None);

        // dragging on the canvas body afterwards still pans normally
        app.on_mouse(down(view.x, view.y));
        app.on_mouse(drag(view.x + 3, view.y + 2));
        assert_eq!(app.sb_drag, None);
        assert!(app.pan.0 >= 3);
    }

    #[test]
    fn drag_table_moves_it_and_survives_reparse() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let view = app.view_rect();
        let down = |c, r| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let drag = |c, r| MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let up = |c, r| MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };

        // users box sits at the view origin; grab and drag it +6/+3
        let users_before = app
            .diagram
            .as_ref()
            .unwrap()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap()
            .clone();
        app.on_mouse(down(view.x + 2, view.y + 2));
        assert!(app.table_drag.is_some(), "press on a table starts a drag");
        assert!(app.pan_anchor.is_none(), "no pan while on a table");
        app.on_mouse(drag(view.x + 8, view.y + 5));
        app.on_mouse(up(view.x + 8, view.y + 5));
        assert!(app.table_drag.is_none());

        let users_after = app
            .diagram
            .as_ref()
            .unwrap()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap()
            .clone();
        assert_eq!(
            (
                users_after.x - users_before.x,
                users_after.y - users_before.y
            ),
            (6, 3)
        );

        // a stationary click on the moved box still jumps to the definition
        let before = app.editor.cursor();
        app.on_mouse(down(view.x + 9, view.y + 4));
        app.on_mouse(up(view.x + 9, view.y + 4));
        assert_ne!(app.editor.cursor(), before);

        // positions survive a re-parse (simulated edit)
        let pos = *app.table_pos.get("users").unwrap();
        app.relayout();
        let users = app
            .diagram
            .as_ref()
            .unwrap()
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        assert_eq!((users.x, users.y), pos);

        // empty-space drag still pans and does not touch tables
        let pan_before = app.pan;
        app.on_mouse(down(view.x + 40, view.y + 30));
        app.on_mouse(drag(view.x + 37, view.y + 28));
        app.on_mouse(up(view.x + 37, view.y + 28));
        assert!(app.pan != pan_before, "empty canvas drag pans");
    }

    #[test]
    fn image_mode_table_at_hits_box() {
        use ratatui_image::picker::{Picker, ProtocolType};
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        app.picker = Some(picker);
        app.render_mode = crate::render::Mode::Kitty;
        app.refresh_image();

        let view = app.view_rect();
        let (sx, sy) = app.diagram_to_screen(0, 0);
        assert!(
            sx >= view.x as i32 && sx < view.right() as i32,
            "users origin on screen at {sx},{sy} view {view:?}"
        );
        let hit = app.table_at((sx + 2) as u16, (sy + 1) as u16);
        assert!(hit.is_some(), "hit at users box origin");
        assert_eq!(hit.unwrap().title, "users");
    }

    #[test]
    fn image_mode_table_drag_defers_to_release() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui_image::picker::{Picker, ProtocolType};
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        app.picker = Some(picker);
        app.render_mode = crate::render::Mode::Kitty;
        app.refresh_image();
        assert!(!app.image_due, "settled after initial refresh");

        let view = app.view_rect();
        let down = |c, r| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let drag = |c, r| MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };
        let up = |c, r| MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: c,
            row: r,
            modifiers: KeyModifiers::NONE,
        };

        let before = app.diagram.as_ref().unwrap().boxes[0].clone();
        app.on_mouse(down(view.x + 2, view.y + 2));
        app.on_mouse(drag(view.x + 8, view.y + 5));
        // mid-drag: pending position tracked, no re-layout, no emission
        assert_eq!(app.table_drag.as_ref().unwrap().pending, Some((6, 3)));
        assert_eq!(
            app.diagram.as_ref().unwrap().boxes[0].x,
            before.x,
            "no re-layout mid-drag"
        );
        assert!(!app.image_due, "no image re-transmission mid-drag");

        // release commits exactly once
        app.on_mouse(up(view.x + 8, view.y + 5));
        assert!(app.table_drag.is_none());
        assert_eq!(
            app.diagram.as_ref().unwrap().boxes[0].x,
            before.x + 6,
            "committed on release"
        );
        assert!(app.image_due, "single re-render scheduled");
    }

    #[test]
    fn text_mode_zooms_through_halfblocks() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        use ratatui_image::picker::{Picker, ProtocolType};
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let mut picker = Picker::from_fontsize((10, 20));
        picker.set_protocol_type(ProtocolType::Halfblocks);
        app.picker = Some(picker);
        assert!(!app.is_image_mode(), "cell renderer at zoom 1.0");
        assert_eq!(app.font_px(), (1, 2), "halfblocks raster units");

        // zoom-in is disabled in text mode: the cell renderer is already at
        // native terminal resolution, magnifying through halfblocks only
        // degrades it
        let wheel = |kind: MouseEventKind| MouseEvent {
            kind,
            column: 50,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert!(
            (app.zoom - 1.0).abs() < f32::EPSILON,
            "no zoom-in in text mode"
        );
        assert!(!app.is_image_mode());

        // wheel down zooms OUT into the halfblocks minimap range
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert!(
            (app.zoom - 0.8).abs() < 1e-6,
            "first step is 80%, got {}",
            app.zoom
        );
        assert!(app.is_image_mode(), "zoomed text uses the raster pipeline");
        assert!(app.image_due);

        app.refresh_image();
        assert!(app.full_raster.is_some(), "raster built for halfblocks");
        assert!(app.proto.is_some(), "halfblocks protocol created");

        for _ in 0..8 {
            app.on_mouse(wheel(MouseEventKind::ScrollDown));
        }
        assert!(
            (app.zoom - 0.25).abs() < 1e-6 || app.zoom < 1.0,
            "zoom out allowed below 1.0, got {}",
            app.zoom
        );

        // wheel back up lands on exactly 1.0: crisp cell renderer again
        for _ in 0..10 {
            app.on_mouse(wheel(MouseEventKind::ScrollUp));
        }
        assert!(
            (app.zoom - 1.0).abs() < f32::EPSILON,
            "clamped at 1.0, got {}",
            app.zoom
        );
        assert!(!app.is_image_mode());
    }

    #[test]
    fn editor_sample_matches_app_source() {
        let app = App::with_sample_source();
        assert_eq!(app.editor.text(), app.source);
    }

    #[test]
    fn file_round_trip_is_byte_identical() {
        let dir = std::env::temp_dir().join(format!("tui-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("a.dbml");
        let dst_path = dir.join("b.dbml");
        std::fs::write(&src_path, "Table t {\n  id integer [pk]\n}\n").unwrap();

        let mut app = App::default();
        app.open_file(src_path.to_str().unwrap()).unwrap();
        assert!(!app.is_modified());

        // modify, then save-as and confirm the modified content landed
        app.editor.handle_key(key('G'));
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        for c in "Table extra { id integer }".chars() {
            app.editor.handle_key(key(c));
        }
        app.editor.handle_key(esc());
        assert!(app.is_modified());
        app.save_file(Some(dst_path.to_str().unwrap())).unwrap();
        assert!(!app.is_modified());
        assert_eq!(
            std::fs::read_to_string(&dst_path).unwrap(),
            "Table t {\n  id integer [pk]\n}\nTable extra { id integer }\n"
        );

        // :q refuses while modified, :q! quits
        app.editor.handle_key(key('A'));
        app.editor.handle_key(key('z'));
        app.editor.handle_key(esc());
        app.handle_command("q");
        assert!(!app.should_quit, ":q must refuse when modified");
        app.handle_command("q!");
        assert!(app.should_quit);

        std::fs::remove_dir_all(&dir).ok();
    }
}
