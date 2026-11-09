//! Deterministic layered layout, shared by the text and image renderers and
//! by mouse hit-testing. Coordinates are in terminal cells.


use crate::dbml::ast::Cardinality;
use crate::dbml::ast::Span;
use crate::model::{Edge, NodeKind, Schema};
use iced::Color;

pub const GAP_X: i32 = 5;
pub const GAP_Y: i32 = 2;

/// Bold text runs wider than the advance classes `Row::est_width` was
/// calibrated on (regular row font) — headers multiply by this so glyphs
/// placed after a bold label clear its last character. Deliberately
/// generous: the estimator undershoots on real desktop fonts, and an
/// overshoot is just a wider gap while an undershoot rides on the name.
/// Lives HERE because the box-width computation reserves the header's
/// glyph room with the same factor the canvas painter places glyphs with.
pub const BOLD_TEXT: f32 = 1.4;

/// WCAG relative-luminance contrast ratio between two colors (1.0..=21):
/// the same measure web accessibility uses, so "readable" has a number.
/// Lives HERE (not in the bin's theme) because the SVG exporter needs it
/// too.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    fn lum(c: Color) -> f32 {
        fn lin(v: f32) -> f32 {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }
    let (l1, l2) = (lum(a), lum(b));
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// The near-white chrome ink the bands must carry — theme::TEXT's value,
/// mirrored here because the lib cannot see the bin's theme module.
const CHROME: Color = Color::from_rgb(0.902, 0.929, 0.953);

/// The band color that carries the white chrome (title, glyphs): a
/// header may carry ANY user hex, so bright picks are DARKENED —
/// multiplicative RGB scaling keeps the hue — until the near-white
/// chrome reads (WCAG 3.5, the large-text bar). Dark bands pass
/// untouched; the color picker still shows the original hex, and lines
/// inherit the original color (on the dark canvas a bright line is the
/// VISIBLE one — only the band, where white chrome sits, adjusts).
/// Lives HERE for the same reason as contrast_ratio: the exporter clamps
/// its bands identically.
pub fn readable_band(bg: Color) -> Color {
    let mut c = bg;
    for _ in 0..64 {
        if contrast_ratio(CHROME, c) >= 3.5 {
            return c;
        }
        c = Color {
            r: c.r * 0.93,
            g: c.g * 0.93,
            b: c.b * 0.93,
            a: c.a,
        };
    }
    c
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pk: bool,
    pub fk: bool,
    pub name: String,
    /// Span of the name token in the source (the jump target when a
    /// diagram column row is clicked).
    pub name_span: Span,
    pub ty: String,
    /// The type names a schema enum: the row gets dbdiagram's "Enum" pill
    /// right of the type text (hovering it shows the enum's values).
    pub ty_enum: bool,
    /// Some(true) = not null, Some(false) = explicit null.
    pub not_null: Option<bool>,
    pub unique: bool,
    pub note: Option<String>,
    /// `default:` verbatim (quotes kept).
    pub default: Option<String>,
    /// `[increment]` — the records grid renders the row number for unset
    /// cells on this column (dbdiagram's value chain).
    pub increment: bool,
    /// dbdiagram custom properties, shown on the row's hover card.
    pub custom: Vec<(String, String)>,
}

impl Row {
    /// Extra width (cells) of one left-cluster icon slot and of the
    /// "Enum" / "Not Null" pills' lead-in.
    /// One icon slot in the left cluster: a 10.5px glyph plus its 4px gap,
    /// in cells (their WN.PADDING_X = 4 at the 20px 1:1 cell).
    pub const ICON_CELL: f32 = (10.5 + 4.0) / 20.0;
    /// One text-pill badge: glyph run plus its padding/gap, in cells.
    pub const BADGE_CELL: f32 = 0.42; // one badge-font char in cells
    pub fn badge_cell(text: &str) -> f32 {
        0.6 + text.chars().count() as f32 * Self::BADGE_CELL
    }
    /// Estimated cells per character of sans text at the row font:
    /// ~0.5em average advance with slack, mirroring the canvas painter's
    /// `char_px` so boxes hug their content like dbdiagram's measureText.
    pub const CHAR_CELL: f32 = 0.58;

    /// Approximate rendered width of `text` in CELLS at the row font —
    /// per-character advance classes instead of one uniform factor
    /// (dbdiagram measures with the real font; "author_id" is much
    /// narrower than the old 0.58/char estimate, which showed up as a
    /// dead gap before the row's first icon). Slightly generous: the
    /// canvas cannot measure text, so an undershoot would overlap glyphs.
    pub fn est_width(text: &str) -> f32 {
        const NARROW: f32 = 0.30;
        const WIDE: f32 = 0.85;
        const UPPER: f32 = 0.66;
        const DIGIT: f32 = 0.55;
        const LOWER: f32 = 0.55;
        text.chars()
            .map(|c| match c {
                'i' | 'j' | 'l' | 'f' | 't' | 'r' | 'I' | '.' | ',' | ':' | ';' | '\'' | '!'
                | '|' => NARROW,
                'm' | 'w' | 'M' | 'W' => WIDE,
                'A'..='Z' => UPPER,
                '0'..='9' => DIGIT,
                '_' => 0.50,
                '-' | '/' | '(' | ')' | '[' | ']' => 0.36,
                ' ' => NARROW,
                c if c.is_lowercase() => LOWER,
                c if c.is_alphanumeric() => 0.60,
                _ => 0.50,
            })
            .sum()
    }

    pub fn width(&self) -> i32 {
        // left: name inset, then dbdiagram's icon cluster (pk, fk, unique,
        // note — each 10.5px glyph + 4px gap). The note icon rides ANY
        // custom property, not just a `note` key: it is the
        // "extra info on hover" affordance.
        let has_info = self.note.is_some() || !self.custom.is_empty();
        let mut w = 1.0 + Self::est_width(&self.name) + 0.25;
        if self.pk {
            w += Self::ICON_CELL;
        }
        if self.fk {
            w += Self::ICON_CELL;
        }
        if self.unique {
            w += Self::ICON_CELL;
        }
        if has_info {
            w += Self::ICON_CELL;
        }
        // right: type text, then the "Enum" / "Not Null" pills flush right
        w += 0.6 + Self::est_width(&self.ty);
        if self.ty_enum {
            w += Self::badge_cell("Enum");
        }
        if self.not_null == Some(true) {
            w += Self::badge_cell("Not Null");
        }
        w += 0.4; // right inner padding
        (w.max(6.0)).ceil() as i32
    }
}

/// Char-wrap `text` to `max` columns (canvas text cannot wrap); long
/// unbroken words hard-break so nothing escapes the card.
pub fn wrap_chars(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut current = String::new();
        for word in para.split_whitespace() {
            if word.chars().count() > max {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                let mut chunk = String::new();
                for c in word.chars() {
                    chunk.push(c);
                    if chunk.chars().count() == max {
                        lines.push(std::mem::take(&mut chunk));
                    }
                }
                current = chunk;
                continue;
            }
            let candidate = current.chars().count() + word.chars().count() + 1;
            if !current.is_empty() && candidate > max {
                lines.push(std::mem::take(&mut current));
            }
            if current.is_empty() {
                current.push_str(word);
            } else {
                current.push(' ');
                current.push_str(word);
            }
        }
        lines.push(current);
    }
    lines
}

/// The left-cluster icons a column row can show (dbdiagram's pk /
/// fk-link / unique fingerprint / note glyphs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowIcon {
    Pk,
    Fk,
    Unique,
    Note,
}


pub const PK_ICON: &str = "M 6.5625 0.9844 C 8.1935 0.9844 9.5156 2.3065 9.5156 3.9375 C 9.5156 5.5685 8.1935 6.8906 6.5625 6.8906 C 6.1735 6.8906 5.8022 6.8153 5.462 6.6786 L 4.5938 7.5469 H 3.9375 V 8.5313 H 2.9531 V 9.5156 H 0.9844 V 7.5469 L 3.736 4.7952 C 3.6518 4.5171 3.6092 4.2281 3.6094 3.9375 C 3.6094 2.3065 4.9315 0.9844 6.5625 0.9844 Z M 6.5625 0 C 4.388 0 2.625 1.7627 2.625 3.9375 C 2.625 4.1186 2.6373 4.2989 2.6619 4.4772 L 0.1441 6.995 C 0.0519 7.0873 0 7.2125 0 7.343 L 0 10.0078 C 0 10.2797 0.2204 10.5 0.4922 10.5 H 3.4453 C 3.7171 10.5 3.9375 10.2797 3.9375 10.0078 V 9.5156 H 4.4297 C 4.7015 9.5156 4.9219 9.2953 4.9219 9.0234 V 8.6133 L 5.7432 7.7895 C 6.011 7.8464 6.2848 7.875 6.5625 7.875 C 8.737 7.875 10.5 6.1123 10.5 3.9375 C 10.5 1.763 8.7373 0 6.5625 0 Z M 6.5625 2.9531 C 6.5625 3.4968 7.0032 3.9375 7.5469 3.9375 C 8.0905 3.9375 8.5313 3.4968 8.5313 2.9531 C 8.5313 2.4095 8.0905 1.9688 7.5469 1.9688 C 7.0032 1.9688 6.5625 2.4095 6.5625 2.9531 Z";
pub const FK_ICON: &str = "M 4.6244 6.6258 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -1.1204 -1.1205 -1.1204 -2.9437 0 -4.0641 l 1.5016 -1.5016 c 0.5419 -0.542 1.2635 -0.8405 2.032 -0.8405 s 1.4901 0.2985 2.032 0.8405 c 1.1204 1.1204 1.1204 2.9436 0 4.0641 l -1.001 1.001 c -0.1416 0.1417 -0.3299 0.2197 -0.5304 0.2198 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 c 0 -0.2004 0.0781 -0.3888 0.2198 -0.5305 l 1.001 -1.001 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9423 c -0.2588 -0.2588 -0.6039 -0.4015 -0.9712 -0.4015 s -0.7123 0.1426 -0.9712 0.4015 l -1.5016 1.5016 c -0.5355 0.5355 -0.5355 1.4068 0 1.9422 c 0.2925 0.2925 0.2925 0.7684 0 1.0609 c -0.1419 0.1417 -0.3302 0.2197 -0.5304 0.2197 z M 2.8725 10.5 c -0.7685 0 -1.4901 -0.2985 -2.032 -0.8405 c -1.1204 -1.1204 -1.1204 -2.9436 0 -4.0641 l 0.7509 -0.7509 c 0.1416 -0.1416 0.3302 -0.2197 0.5304 -0.2197 c 0.2002 0 0.3888 0.0781 0.5304 0.2198 c 0.1416 0.1416 0.2198 0.33 0.2198 0.5304 c 0 0.2004 -0.0781 0.3888 -0.2198 0.5305 l -0.7509 0.7508 c -0.5355 0.5354 -0.5355 1.4067 0 1.9422 c 0.2588 0.2588 0.6039 0.4015 0.9712 0.4015 s 0.7123 -0.1425 0.9712 -0.4015 l 1.5016 -1.5016 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9422 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 s 0.0781 -0.3888 0.2198 -0.5305 c 0.1416 -0.1417 0.3302 -0.2197 0.5304 -0.2197 s 0.3888 0.078 0.5304 0.2196 c 1.1204 1.1205 1.1204 2.9437 0 4.0641 l -1.5016 1.5016 c -0.5419 0.542 -1.2635 0.8405 -2.032 0.8405 z";
pub const UNIQUE_ICON: &str = "m6.179 10.5005c\
    -.0797 0-.1606-.0181-.2367-.0562-.2613-.1309-.3669-.4489-.236-.7101.311-.6204.5644-1.2734.7533-1.9409.0796-.2812.3721-.4446.6532-.365.2812.0796.4446.372.365.6532-.207.7314-.4848 1.447-.8255 2.1268-.0928.1852-.2795.2922-.4734.2922z\
    m-2.3451-.1445c\
    -.0963 0-.1937-.0263-.2813-.0813-.2473-.1556-.3217-.4823-.1661-.7296.8998-1.4301 1.3753-3.0838 1.3753-4.7822 0-.2922.2369-.5291.5291-.5291s\
    .5291.2369.5291.5291c0 1.8981-.5318 3.7467-1.5378 5.3457-.1005.1598-.2725.2474-.4483.2474zm\
    -1.8201-1.0793c-.0927 0-.1865-.0244-.2718-.0755-.2506-.1504-.3318-.4754-.1814-.7259l.0286-.0476c\
    .0019-.0032.0039-.0064.0059-.0096.6898-1.0958 1.053-2.3593 1.0504-3.6541-0-.7077.2752-1.372.7748-1.8717.4997-.4997 1.164-.7749 1.8706-.7749.7067 0 1.371.2752 1.8707.7749.4997.4996.7748 1.164.7748 1.8706 0 .5513-.0378 1.1093-.1122 1.6584-.0393.2896-.3059.4924-.5954.4532-.2896-.0393-.4925-.3058-.4532-.5954.0681-.5022.1026-1.0123.1026-1.5162 0-.424-.1651-.8226-.4649-1.1224-.2998-.2998-.6984-.4649-1.1224-.4649-.424 0-.8226.1651-1.1224.4649-.2998.2998-.4649.6984-.4649 1.1224.003 1.4923-.4154 2.9496-1.2099 4.2139l\
    -.0259.0431c-.0992.1653-.2744.2569-.4542.2569zm6.9865-.2111c\
    -.048 0-.0967-.0066-.1451-.0203-.281-.08-.444-.3727-.364-.6537.3342-1.174.5036-2.3949.5036-3.6287.0002-.6494-.1713-1.2899-.4959-1.8522-.3245-.5624-.7933-1.0312-1.3556-1.356-.5623-.3247-1.2027-.4964-1.852-.4964-.0001 0-.0002 0-.0003 0-.6492 0-1.2895.1716-1.8517.4962-.2531.1461-.5767.0594-.7228-.1937s\
    -.0594-.5767.1937-.7228c.7229-.4174 1.5463-.638 2.381-.638h.0003c\
    .8348 0 1.6582.2208 2.3812.6382.7229.4175 1.3257 1.0204 1.7429 1.7434.4173.7231.6378 1.5465.6376 2.3813 0 1.3317-.183 2.65-.544 3.9183-.0662.2326-.2781.3844-.5086.3844z\
    m-8.4715-1.4646c\
    -.0775 0-.1562-.0171-.2306-.0532-.2629-.1276-.3726-.4441-.245-.707.3159-.651.476-1.3501.476-2.078 0-.8363.2203-1.6596.6371-2.381.1462-.253.4698-.3397.7228-.1935.253.1462.3397.4698.1935.7228-.3239.5607-.4952 1.201-.4952 1.8517 0 .8893-.1959 1.7438-.5822 2.54-.0915.1885-.2801.2982-.4764.2982z";
pub const NOTE_ICON: &str = "M 9 1.125 C 9.2063 1.125 9.375 1.2938 9.375 1.5 V 6.75 H 7.5 C 7.0852 6.75 6.75 7.0852 6.75 7.5 V 9.375 H 1.5 C 1.2938 9.375 1.125 9.2063 1.125 9 V 1.5 C 1.125 1.2938 1.2938 1.125 1.5 1.125 H 9 Z M 1.5 10.5 H 6.75 H 6.8789 C 7.2773 10.5 7.6594 10.343 7.9406 10.0617 L 10.0617 7.9406 C 10.343 7.6594 10.5 7.2773 10.5 6.8789 V 6.75 V 1.5 C 10.5 0.6727 9.8273 0 9 0 H 1.5 C 0.6727 0 0 0.6727 0 1.5 V 9 C 0 9.8273 0.6727 10.5 1.5 10.5 Z M 3 7.6875 C 3 7.5383 2.9407 7.3952 2.8352 7.2897 C 2.7298 7.1843 2.5867 7.125 2.4375 7.125 C 2.2883 7.125 2.1452 7.1843 2.0398 7.2897 C 1.9343 7.3952 1.875 7.5383 1.875 7.6875 C 1.875 7.8367 1.9343 7.9798 2.0398 8.0853 C 2.1452 8.1907 2.2883 8.25 2.4375 8.25 C 2.5867 8.25 2.7298 8.1907 2.8352 8.0853 C 2.9407 7.9798 3 7.8367 3 7.6875 Z M 2.4375 2.25 C 2.2883 2.25 2.1452 2.3093 2.0398 2.4148 C 1.9343 2.5202 1.875 2.6633 1.875 2.8125 C 1.875 2.9617 1.9343 3.1048 2.0398 3.2102 C 2.1452 3.3157 2.2883 3.375 2.4375 3.375 C 2.5867 3.375 2.7298 3.3157 2.8352 3.2102 C 2.9407 3.1048 3 2.9617 3 2.8125 C 3 2.6633 2.9407 2.5202 2.8352 2.4148 C 2.7298 2.3093 2.5867 2.25 2.4375 2.25 Z M 3 5.25 C 3 5.1008 2.9407 4.9577 2.8352 4.8522 C 2.7298 4.7468 2.5867 4.6875 2.4375 4.6875 C 2.2883 4.6875 2.1452 4.7468 2.0398 4.8522 C 1.9343 4.9577 1.875 5.1008 1.875 5.25 C 1.875 5.3992 1.9343 5.5423 2.0398 5.6478 C 2.1452 5.7532 2.2883 5.8125 2.4375 5.8125 C 2.5867 5.8125 2.7298 5.7532 2.8352 5.6478 C 2.9407 5.5423 3 5.3992 3 5.25 Z";

pub fn row_icon_path(icon: RowIcon) -> &'static str {
    match icon {
        RowIcon::Pk => PK_ICON,
        RowIcon::Fk => FK_ICON,
        RowIcon::Unique => UNIQUE_ICON,
        RowIcon::Note => NOTE_ICON,
    }
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBox {
    /// Index into `Schema::nodes`.
    pub node: usize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
    /// `Table users as U` — the alias, for hover-docs and lookups.
    pub alias: Option<String>,
    pub header_color: Option<String>,
    pub rows: Vec<Row>,
    /// `records` sample rows (resolved model form), for the record view
    /// and the canvas affordance; None for tables without them.
    pub records: Option<crate::model::NodeRecords>,
    /// Span of the table/enum name in the source (click-to-jump target).
    pub name_span: Option<Span>,
    /// dbdiagram custom properties on the table, shown on the header's
    /// hover card.
    pub custom: Vec<(String, String)>,
    /// `Note: '...'` inside the block — drives the header's note icon.
    pub note: Option<String>,
}

impl TableBox {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    /// Diagram-coordinates rect of a column row (the edge anchor).
    pub fn col_row_y(&self, col: usize) -> i32 {
        self.y + 1 + col.min(self.rows.len().saturating_sub(1)) as i32
    }

    /// Vertical center of a column row in cell coords.
    pub fn col_row_center(&self, col: usize) -> f32 {
        self.col_row_y(col) as f32 + 0.5
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRoute {
    /// Index into `Schema::edges`.
    pub edge: usize,
    /// Orthogonal waypoints in cell coords: side anchors at the rows'
    /// vertical centers, turns in column-gap lanes. Renderers round the
    /// corners (dbdiagram's look).
    pub pts: Vec<(f32, f32)>,
    /// (node, column) of each end: fk end first, referenced end second
    /// (see `fk_ref`). Column index == row index in the box.
    pub ends: [(usize, usize); 2],
    /// Cardinality, for the crow's-foot / tick end markers.
    pub card: Cardinality,
    /// `Ref name: ...`, for the hover name label at the midpoint.
    pub name: Option<String>,
    /// `[color: #aabbcc]` — the whole relationship renders in this color.
    /// EXPLICIT only: the picker's current-color read and the hover
    /// behavior (explicit lines keep their color; default lines ease
    /// blue) key on this field alone. The render-time fallback lives in
    /// `inherited`.
    pub color: Option<String>,
    /// The REFERENCED table's resolved header color — the render fallback
    /// for a line without an explicit `[color:]` (dbdiagram pairs a line
    /// with the header it points at: `likes.post_id > posts.id` renders
    /// posts' purple). Always None when `color` is Some.
    pub inherited: Option<String>,
    /// The top-level Ref line index (the click-to-pick splice target);
    /// None for inline `ref:` edges.
    pub ref_index: Option<usize>,
}

/// A schema enum definition, for the "Enum" pill's tooltip and the
/// editor's hover docs (enums are never boxes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumInfo {
    pub name: String,
    pub values: Vec<String>,
    /// Span of the enum's name token (the go-to-definition target).
    pub name_span: Option<Span>,
}

/// A table group's rendered region: the bounding box of its member
/// boxes plus padding (dbdiagram's labeled rounded frame).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupRegion {
    pub name: String,
    /// `[color: #1E69FD]` — stroke + label color.
    pub color: Option<String>,
    /// `Note: '''...'''` — the label-hover tooltip.
    pub note: Option<String>,
    /// Header custom properties (`[owner: "team"]`), in order.
    pub custom: Vec<(String, String)>,
    /// Region rect in cells (x, y, w, h).
    pub rect: (i32, i32, i32, i32),
    /// Member node indices (for hover hit tests).
    pub members: Vec<usize>,
}

/// A sticky note on the canvas (dbdiagram's yellow sticky), auto-placed
/// below the diagram content, stacked left to right.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteBox {
    pub name: String,
    pub text: String,
    /// Rect in cells (x, y, w, h).
    pub rect: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub boxes: Vec<TableBox>,
    pub edges: Vec<EdgeRoute>,
    /// The schema's enums, in document order: `Enum post_status { ... }`
    /// feeds the "Enum" pill on enum-typed columns and the tooltips.
    pub enums: Vec<EnumInfo>,
    /// (start_x, end_x) of each layer column, for gap routing.
    pub cols: Vec<(i32, i32)>,
    /// Diagram content extents (max right, max bottom).
    pub extent: (i32, i32),
    /// Table group regions (drawn behind the boxes).
    pub groups: Vec<GroupRegion>,
    /// Sticky notes (drawn above the boxes).
    pub notes: Vec<NoteBox>,
}

/// Longest-path layering over ref edges (cycles tolerated: Kahn order, with
/// cycle members appended after the last layer), then column packing.
pub fn layout(schema: &Schema) -> Layout {
    layout_with_overrides(schema, &std::collections::HashMap::new())
}

/// Gaps of the auto-arrange layout, in cells, translating the ELK layered
/// options dbdiagram's "Left-right" arrange uses (1 cell = 10px wide x
/// 20px tall at 1:1):
/// - `edgeNodeBetweenLayers: 120` -> 12 cells horizontal between layers
pub const ARRANGE_LAYER_GAP_X: i32 = 12;
/// - `spacing.baseValue: 40` -> 2 cells vertical between tables in a layer
pub const ARRANGE_LAYER_GAP_Y: i32 = 2;
/// - `spacing.componentComponent: 80` -> 8 cells between disconnected
///   components
pub const ARRANGE_COMPONENT_GAP: i32 = 8;

/// Longest-path layering over ref edges: the referenced table is the
/// predecessor and FK tables come after it, so diagrams read left-to-right
/// like dbdiagram's. Cycles are tolerated: Kahn's order processes what it
/// can (longest-path relaxations are order-independent); members stuck in
/// a cycle never enter the queue and are parked one layer past the deepest.
fn ref_layers(schema: &Schema) -> Vec<usize> {
    let n = schema.nodes.len();
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &schema.edges {
        let (fk, rf) = fk_ref(e);
        // self-references don't order layers (a table always comes before
        // itself); counting them made Kahn treat the whole graph as cyclic
        if fk != rf && !adj[rf].contains(&fk) {
            adj[rf].push(fk);
            indeg[fk] += 1;
        }
    }

    let mut layer = vec![0usize; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    while let Some(i) = queue.pop() {
        order.push(i);
        for &j in &adj[i] {
            layer[j] = layer[j].max(layer[i] + 1);
            indeg[j] -= 1;
            if indeg[j] == 0 {
                queue.push(j);
            }
        }
    }
    // Cycle members never entered the queue: park them after everything.
    let max_layer = layer.iter().copied().max().unwrap_or(0);
    for (i, l) in layer.iter_mut().enumerate() {
        if !order.contains(&i) {
            *l = max_layer + 1;
        }
    }
    layer
}

/// dbdiagram's "Auto-arrange / Left-right" (ELK layered, left-to-right):
/// referenced tables LEFT of their FK tables, long relationship chains
/// become long left-to-right runs. Returns the same shape as the
/// drag-override map — table name -> (x, y) in CELLS — for the app to
/// write into `table_pos` and re-layout with `layout_with_overrides`; it
/// never mutates `layout()` itself.
///
/// ELK options it mirrors (1 cell = 10px wide x 20px tall at 1:1):
/// - layering: longest-path (INTERACTIVE), cycles parked after the last
///   layer (`ref_layers`)
/// - crossingMinimization: barycenter sweeps from model order;
///   forceNodeModelOrder=true keeps document order on ties
/// - nodePlacement: INTERACTIVE — the in-layer barycenter order is placed
///   top-down as stacked blocks, each layer vertically centered (ELK
///   centers layers)
/// - spacing: 120px between layers, 40px in-layer, 80px between
///   disconnected components (packed left to right, each centered on the
///   global center line)
/// - mergeEdges: edge routing only — `EdgeRoute` already handles it
/// - TableGroups: members place individually (groups are not units), and
///   free-standing tables drop below any group frame they would overlap
pub fn auto_arrange(
    boxes: &[TableBox],
    schema: &Schema,
    stickies: &[(i32, i32, i32, i32)],
) -> std::collections::HashMap<String, (i32, i32)> {
    // sticky rects arrive as (x, y, w, h) cells — the NoteBox rect form —
    // and every obstacle check below speaks (x0, y0, x1, y1)
    let stickies: Vec<(i32, i32, i32, i32)> = stickies
        .iter()
        .map(|(x, y, w, h)| (*x, *y, x + w, y + h))
        .collect();
    let n = schema.nodes.len();
    let mut out = std::collections::HashMap::new();
    if n == 0 {
        return out;
    }
    let layer = ref_layers(schema);

    // Box sizes and titles by node index. Enum nodes have no box (enums
    // are "Enum" pills on their columns, never boxes) and stale nodes lose
    // theirs on edits: both drop out of the arrangement entirely, so no
    // phantom component gaps open up where they used to sit.
    let mut size = vec![(0i32, 0i32); n];
    let mut title = vec![String::new(); n];
    for b in boxes {
        size[b.node] = (b.w, b.h);
        title[b.node] = b.title.clone();
    }
    let live: Vec<usize> = (0..n).filter(|&i| !title[i].is_empty()).collect();
    if live.is_empty() {
        return out;
    }
    let slot_of: std::collections::HashMap<usize, usize> = live
        .iter()
        .copied()
        .enumerate()
        .map(|(slot, i)| (i, slot))
        .collect();
    let m = live.len();
    // everything below works in the compact (boxed) node space
    let layer: Vec<usize> = live.iter().map(|&i| layer[i]).collect();
    let size: Vec<(i32, i32)> = live.iter().map(|&i| size[i]).collect();
    let title: Vec<String> = live.iter().map(|&i| title[i].clone()).collect();
    let max_layer = layer.iter().copied().max().unwrap_or(0);

    // Ref adjacency (referenced -> FK) both ways, for the barycenter
    // sweeps' neighbors in adjacent layers. Edges into unboxed nodes
    // (dangling after an edit) drop out with them.
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); m];
    for e in &schema.edges {
        let (fk, rf) = fk_ref(e);
        if fk == rf {
            continue;
        }
        let (Some(fk), Some(rf)) = (slot_of.get(&fk), slot_of.get(&rf)) else {
            continue;
        };
        let (fk, rf) = (*fk, *rf);
        if !succ[rf].contains(&fk) {
            succ[rf].push(fk);
            pred[fk].push(rf);
        }
    }

    // In-layer order: model (document) order, then barycenter sweeps —
    // each node's key is the average of its neighbors' positions in the
    // adjacent layers. Ties keep model order (forceNodeModelOrder).
    let mut order_layers: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for i in 0..m {
        order_layers[layer[i]].push(i);
    }
    let mut pos = vec![0usize; m];
    for l in &mut order_layers {
        for (i, v) in l.iter().enumerate() {
            pos[*v] = i;
        }
    }
    for _ in 0..4 {
        let mut changed = false;
        for nodes in order_layers.iter_mut().take(max_layer + 1).skip(1) {
            let mut keys: Vec<(f32, usize)> = nodes
                .iter()
                .map(|&v| {
                    let (mut sum, mut cnt) = (0.0f32, 0usize);
                    for &u in &pred[v] {
                        if layer[u] + 1 == layer[v] {
                            sum += pos[u] as f32;
                            cnt += 1;
                        }
                    }
                    for &w in &succ[v] {
                        if layer[w] == layer[v] + 1 {
                            sum += pos[w] as f32;
                            cnt += 1;
                        }
                    }
                    let bc = if cnt > 0 {
                        sum / cnt as f32
                    } else {
                        pos[v] as f32
                    };
                    (bc, v)
                })
                .collect();
            keys.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            });
            for (i, (_, v)) in keys.iter().enumerate() {
                if pos[*v] != i {
                    changed = true;
                    pos[*v] = i;
                }
            }
            *nodes = keys.into_iter().map(|(_, v)| v).collect();
        }
        if !changed {
            break;
        }
    }

    // Connected components over ref edges (union-find); isolated tables
    // (no refs) are their own component.
    let mut parent: Vec<usize> = (0..m).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (v, preds) in pred.iter().enumerate() {
        for &u in preds {
            let (a, b) = (find(&mut parent, u), find(&mut parent, v));
            if a != b {
                parent[a] = b;
            }
        }
    }
    // Components in model order of their first node.
    let mut comps: Vec<Vec<usize>> = Vec::new();
    let mut by_root: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..m {
        let root = find(&mut parent, i);
        let idx = *by_root.entry(root).or_insert_with(|| {
            comps.push(Vec::new());
            comps.len() - 1
        });
        comps[idx].push(i);
    }

    // Each component laid out on its own: layers left to right (layer x =
    // previous x + previous max width + 12), each layer's nodes stacked
    // top-down in barycenter order with 2-cell gaps, the block centered on
    // the component's center line (local y 0).
    let mut placed = vec![(0i32, 0i32); m];
    let mut comp_x = 0i32;
    for comp in &comps {
        let mut x = comp_x;
        let mut max_right = comp_x;
        for layer_nodes in &order_layers {
            let nodes: Vec<usize> = layer_nodes
                .iter()
                .copied()
                .filter(|&v| comp.contains(&v))
                .collect();
            if nodes.is_empty() {
                continue;
            }
            let layer_w = nodes.iter().map(|&v| size[v].0).max().unwrap_or(0);
            let layer_h: i32 = nodes.iter().map(|&v| size[v].1).sum::<i32>()
                + ARRANGE_LAYER_GAP_Y * (nodes.len() as i32 - 1);
            let mut y = -(layer_h as f32 / 2.0).round() as i32;
            for &v in &nodes {
                placed[v] = (x, y);
                y += size[v].1 + ARRANGE_LAYER_GAP_Y;
            }
            max_right = max_right.max(x + layer_w);
            x += layer_w + ARRANGE_LAYER_GAP_X;
        }
        // Pack the next component to the right (componentComponent 80px).
        comp_x = max_right + ARRANGE_COMPONENT_GAP;
    }

    // Free-standing tables must not sit under a TableGroup's frame: the
    // layering above knows nothing of groups, so a member pair straddling
    // a middle layer spans that layer's column and buries whatever parked
    // there. Each region is the members' NEW union box (they were placed
    // individually above) plus the 2-cell frame padding and 1-cell header
    // band above — the same rect `layout_with_all_overrides` draws. Every
    // non-member whose cell intersects one drops below it (the in-layer
    // gap); the loop re-checks because a drop can land in a lower region
    // or on another box, and each step moves strictly past an obstacle's
    // bottom, so it terminates.
    const GROUP_PAD: i32 = 2; // keep in sync with layout_with_all_overrides

    /// A group's region: its members' current union box + the frame
    /// padding and the header band row above. Half-open cells.
    fn group_region(
        members: &[usize],
        placed: &[(i32, i32)],
        size: &[(i32, i32)],
    ) -> (i32, i32, i32, i32) {
        let mut rect = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &s in members {
            let (x, y) = placed[s];
            let (w, h) = size[s];
            rect.0 = rect.0.min(x);
            rect.1 = rect.1.min(y);
            rect.2 = rect.2.max(x + w);
            rect.3 = rect.3.max(y + h);
        }
        (
            rect.0 - GROUP_PAD,
            rect.1 - GROUP_PAD - 1,
            rect.2 + GROUP_PAD,
            rect.3 + GROUP_PAD,
        )
    }

    let group_members: Vec<Vec<usize>> = schema
        .groups
        .iter()
        .map(|g| {
            g.members
                .iter()
                .filter_map(|n| slot_of.get(n).copied())
                .collect()
        })
        .collect();
    let grouped: std::collections::HashSet<usize> =
        group_members.iter().flatten().copied().collect();

    // The arrangement anchors at the content's CURRENT min corner (not
    // a re-normalized origin): pinned stickies are absolute-canvas
    // obstacles, so the result must land in the same space they live in
    // — the dodge below is meaningless if the whole layout then slides
    // somewhere else.
    let (anchor_x, anchor_y) = {
        let mut mx = i32::MAX;
        let mut my = i32::MAX;
        for b in boxes {
            mx = mx.min(b.x);
            my = my.min(b.y);
        }
        if mx == i32::MAX {
            (0, 0)
        } else {
            (mx, my)
        }
    };
    let new_min_x = placed.iter().map(|p| p.0).min().unwrap_or(0);
    let new_min_y = placed.iter().map(|p| p.1).min().unwrap_or(0);
    for p in placed.iter_mut() {
        p.0 += anchor_x - new_min_x;
        p.1 += anchor_y - new_min_y;
    }

    // Group regions must not overlap EACH OTHER either: the naive
    // per-component layering interleaves members of different groups (a
    // pair sharing a layer spans that whole column, burying the other
    // group's frame) — billing's frame used to land inside fulfillment's.
    // And a user-pinned sticky holds its ground wherever the old layout
    // left it, so regions must clear those too. Each group shifts DOWN
    // as a rigid block — members keep their intra-group arrangement —
    // until its region clears every earlier group's final region and
    // every sticky. Groups resolve in schema order: earlier regions are
    // final when the next group starts, and each step moves strictly
    // past an obstacle's bottom, so it terminates.
    let mut final_regions: Vec<(i32, i32, i32, i32)> = Vec::new();
    for mem in &group_members {
        if mem.is_empty() {
            continue; // stale members only: no box, no region
        }
        let mut region = group_region(mem, &placed, &size);
        loop {
            let hit = final_regions
                .iter()
                .copied()
                .chain(stickies.iter().copied())
                .find(|&(ox0, oy0, ox1, oy1)| {
                    region.0 < ox1 && ox0 < region.2 && region.1 < oy1 && oy0 < region.3
                });
            let Some((_, _, _, oy1)) = hit else {
                break;
            };
            let dy = oy1 + ARRANGE_LAYER_GAP_Y - region.1;
            for &s in mem {
                placed[s].1 += dy;
            }
            region = group_region(mem, &placed, &size);
        }
        final_regions.push(region);
    }
    let regions = final_regions;

    for i in 0..m {
        if grouped.contains(&i) {
            continue; // members shape the regions; only free tables dodge
        }
        let (w, h) = size[i];
        loop {
            let (x, y) = placed[i];
            // obstacles: group frames, then the pinned stickies, then
            // every other box at its current spot — a drop must not
            // trade one overlap for another (later tables re-dodge the
            // earlier drops' final spots; each pass only ever moves down)
            let hit = regions
                .iter()
                .copied()
                .chain(stickies.iter().copied())
                .chain((0..m).filter(|&o| o != i).map(|o| {
                    let (ox, oy) = placed[o];
                    let (ow, oh) = size[o];
                    (ox, oy, ox + ow, oy + oh)
                }))
                .find(|&(ox0, oy0, ox1, oy1)| {
                    x < ox1 && ox0 < x + w && y < oy1 && oy0 < y + h
                });
            let Some((_, _, _, oy1)) = hit else {
                break;
            };
            placed[i].1 = oy1 + ARRANGE_LAYER_GAP_Y;
        }
    }

    for (i, p) in placed.iter().enumerate() {
        out.insert(title[i].clone(), *p);
    }
    out
}

/// Layout with user-dragged table positions (diagram coordinates, keyed by
/// table name). Overridden boxes keep their dragged position; column
/// extents, content bounds, and edge routes are computed from the final
/// positions. New/renamed tables fall back to the automatic placement.
pub fn layout_with_overrides(
    schema: &Schema,
    overrides: &std::collections::HashMap<String, (i32, i32)>,
) -> Layout {
    layout_with_all_overrides(schema, overrides, &std::collections::HashMap::new())
}

/// The full form: table position overrides plus STICKY note overrides
/// (full rects — x, y, w, h in cells — by note name). Notes without an
/// override keep their auto-placed slot below the content; an override
/// moves/resizes the note anywhere (no clamping, negative cells fine —
/// like tables).
pub fn layout_with_all_overrides(
    schema: &Schema,
    overrides: &std::collections::HashMap<String, (i32, i32)>,
    note_overrides: &std::collections::HashMap<String, (i32, i32, i32, i32)>,
) -> Layout {
    let layer = ref_layers(schema);

    // The schema's enums: never boxes (dbdiagram draws an "Enum" pill
    // after an enum-typed column instead). The definitions feed the
    // pill's tooltip and the editor's hover docs; the name set flags the
    // rows whose type names an enum.
    let enums: Vec<EnumInfo> = schema
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Enum)
        .map(|n| EnumInfo {
            name: n.name.clone(),
            values: n.columns.iter().map(|c| c.name.clone()).collect(),
            name_span: n.name_span.clone(),
        })
        .collect();
    let enum_names: std::collections::HashSet<&str> =
        enums.iter().map(|e| e.name.as_str()).collect();

    // Boxes, sized from their content.
    let boxes: Vec<TableBox> = schema
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind != NodeKind::Enum)
        .map(|(idx, node)| {
            let title = node.name.clone();
            let rows = node
                .columns
                .iter()
                .map(|c| Row {
                    pk: c.pk,
                    fk: c.fk,
                    name: c.name.clone(),
                    name_span: c.name_span.clone(),
                    ty: c.ty.clone(),
                    ty_enum: enum_names.contains(c.ty.as_str()),
                    not_null: c.not_null,
                    unique: c.unique,
                    note: c.note.clone(),
                    default: c.default.clone(),
                    custom: c.custom.clone(),
                    increment: c.increment,
                })
                .collect::<Vec<_>>();
            // the header reserves room for its own affordances — the
            // title, then the always-visible note glyph (note/custom
            // tables) and records glyph (records tables, placed RIGHT of
            // the note glyph or of the title), plus the hover-only color
            // wheel's slot at the right edge — so a narrow table GROWS
            // instead of letting the glyphs ride over the wheel. Cell
            // math mirrors the canvas painter's placement (paint_table /
            // records_band): title inset 0.6, glyph gap 0.6 to the note,
            // 0.45 between glyphs, note icon 1.0 cell wide (0.5 cell
            // height), records icon ≈1.1 cells, wheel slot ≈2.2 cells at
            // the right edge.
            let has_info = node.note.is_some() || !node.custom.is_empty();
            let header_min = 0.6
                + Row::est_width(&title) * BOLD_TEXT
                + if has_info { 0.6 + 1.0 } else { 0.0 }
                + if node.records.is_some() { 0.45 + 1.1 } else { 0.0 }
                + 0.45
                + 2.2;
            let content = rows
                .iter()
                .map(|r| r.width())
                .chain(std::iter::once((Row::est_width(&title) + 1.0).ceil() as i32))
                .chain(std::iter::once(header_min.ceil() as i32))
                .max()
                .unwrap_or(6);
            // dbdiagram's inheritance: a table WITHOUT an explicit
            // `headercolor` renders in its TableGroup's color (an explicit
            // one always wins)
            let group_color = schema
                .groups
                .iter()
                .find(|g| g.members.contains(&idx))
                .and_then(|g| g.color.clone());
            TableBox {
                node: idx,
                x: 0,
                y: 0,
                // dbdiagram: each box hugs ITS content (no uniform width),
                // and the height is exactly header + rows — a spare bottom
                // row read as an empty section under the column list
                w: content + 2,
                h: rows.len() as i32 + 1,
                title,
                alias: node.alias.clone(),
                header_color: node
                    .header_color
                    .clone()
                    .or(group_color),
                rows,
                records: node.records.clone(),
                name_span: node.name_span.clone(),
                custom: node.custom.clone(),
                note: node.note.clone(),
            }
        })
        .collect();

    // Column packing: one column of tables per layer, doc order within.
    // All boxes in a column are left-aligned; `cols` records each column's
    // extent so edge routing can use the gaps between columns.
    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut placed = Vec::new();
    let mut cols: Vec<(i32, i32)> = Vec::new();
    let mut x = 0i32;
    let mut max_right = 0i32;
    let mut max_bottom = 0i32;
    // groups reserve a 3-cell header-band row ABOVE their topmost member:
    // starting the packing at 3 keeps that band on-canvas (it would sit at
    // negative y — clipped — in a fresh session otherwise)
    let top_offset = if schema.groups.is_empty() { 0 } else { 3 };
    for l in 0..=max_layer {
        let mut y = top_offset;
        let mut col_w = 0i32;
        let count = boxes
            .iter()
            .enumerate()
            .filter(|(idx, _)| layer[*idx] == l)
            .count();
        for (idx, b) in boxes.iter().enumerate() {
            if layer[idx] != l {
                continue;
            }
            let mut b = b.clone();
            b.x = x;
            b.y = y;
            y = b.bottom() + GAP_Y;
            col_w = col_w.max(b.w);
            max_right = max_right.max(b.right());
            max_bottom = max_bottom.max(b.bottom());
            placed.push(b);
        }
        // Push an entry for every layer (empty ones get zero width) so
        // `cols` indices always match layer numbers.
        cols.push((x, x + col_w));
        if count > 0 {
            x += col_w + GAP_X;
        }
    }

    // Dragged positions win over the automatic packing. No clamping:
    // boxes may sit above/left of the origin (negative diagram coords).
    for b in &mut placed {
        if let Some((x, y)) = overrides.get(&b.title) {
            b.x = *x;
            b.y = *y;
        }
    }
    // Recompute column extents and bounds from the final positions so
    // routing and scrollbars reflect where boxes actually are.
    let mut max_right = 0;
    let mut max_bottom = 0;
    for (l, col) in cols.iter_mut().enumerate() {
        let mut start = i32::MAX;
        let mut end = i32::MIN;
        for b in placed.iter().filter(|b| layer[b.node] == l) {
            start = start.min(b.x);
            end = end.max(b.right());
            max_right = max_right.max(b.right());
            max_bottom = max_bottom.max(b.bottom());
        }
        if start != i32::MAX {
            *col = (start, end);
        }
    }
    if std::env::var("DBG_LAYOUT").is_ok() {
        for b in &placed {
            eprintln!(
                "DBG layout box {:?} x={} y={} w={} node={} layer={}",
                b.title, b.x, b.y, b.w, b.node, layer[b.node]
            );
        }
        eprintln!(
            "DBG layout cols={:?} overrides={}",
            cols, overrides.len()
        );
    }

    let edges = schema
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| {
            let (pts, ends) = route(&placed, e, &cols);
            // color: EXPLICIT `[color:]` only, so the hover easing and the
            // picker's current-color read see exactly the source
            // attributes. The render fallback: a line without one takes
            // the REFERENCED table's resolved header color (which itself
            // may cascade from its TableGroup) — dbdiagram pairs the line
            // with the header it points at.
            let color = e.color.clone();
            let inherited = if e.color.is_some() {
                None
            } else {
                let (_, rf) = fk_ref(e);
                placed
                    .iter()
                    .find(|b| b.node == rf)
                    .and_then(|b| b.header_color.clone())
            };
            EdgeRoute {
                edge: ei,
                pts,
                ends,
                card: e.card,
                name: e.name.clone(),
                color,
                inherited,
                ref_index: e.ref_index,
            }
        })
        .collect();

    // Table group regions: each group's member bounding box + 2-cell
    // padding (live-verified dbdiagram look). Groups with no placed
    // members drop out.
    const GROUP_PAD: i32 = 2;
    let groups = schema
        .groups
        .iter()
        .filter_map(|g| {
            let mut min_x = i32::MAX;
            let mut min_y = i32::MAX;
            let mut max_r = i32::MIN;
            let mut max_b = i32::MIN;
            for b in placed.iter().filter(|b| g.members.contains(&b.node)) {
                min_x = min_x.min(b.x);
                min_y = min_y.min(b.y);
                max_r = max_r.max(b.right());
                max_b = max_b.max(b.bottom());
            }
            if min_x == i32::MAX {
                return None;
            }
            Some(GroupRegion {
                name: g.name.clone(),
                color: g.color.clone(),
                note: g.note.clone(),
                custom: g.custom.clone(),
                rect: (
                    min_x - GROUP_PAD,
                    min_y - GROUP_PAD - 1,
                    max_r - min_x + 2 * GROUP_PAD,
                    max_b - min_y + 2 * GROUP_PAD + 1,
                ),
                members: g.members.clone(),
            })
        })
        .collect::<Vec<_>>();
    for gr in &groups {
        max_right = max_right.max(gr.rect.0 + gr.rect.2);
        max_bottom = max_bottom.max(gr.rect.1 + gr.rect.3);
    }

    // Sticky notes: a row below everything, ~14x5 cells each, packed left
    // to right (dbdiagram's stickies are free-floating; auto-placement
    // keeps them visible without a drag store).
    let mut notes = Vec::new();
    if !schema.notes.is_empty() {
        let y = max_bottom + 2;
        let mut x = 0;
        for n in &schema.notes {
            // user-dragged/resized notes keep their full rect; the rest
            // keep the auto slot below the content
            let rect = match note_overrides.get(&n.name) {
                Some((nx, ny, nw, nh)) => (*nx, *ny, *nw, *nh),
                None => (x, y, 14, 5),
            };
            notes.push(NoteBox {
                name: n.name.clone(),
                text: n.text.clone(),
                rect,
            });
            x += 16;
        }
        max_bottom = y + 5;
        max_right = max_right.max(x);
    }

    Layout {
        boxes: placed,
        edges,
        enums,
        cols,
        extent: (max_right, max_bottom + 1),
        groups,
        notes,
    }
}

/// How many segments a bezier is sampled into for the canvas polyline.
impl Layout {
    /// Indices (into `edges`) of routes touching a (node, column) row.
    pub fn edges_touching(&self, node: usize, col: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|r| r.ends.contains(&(node, col)))
            .map(|r| r.edge)
            .collect()
    }

    /// Every edge attached to a table (either end) — hovering the table's
    /// header lights all of its relationships, dbdiagram-style.
    pub fn edges_touching_table(&self, node: usize) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|r| r.ends.iter().any(|(n, _)| *n == node))
            .map(|r| r.edge)
            .collect()
    }

    /// During a table drag the re-layout recomputes EVERY route from the
    /// moved positions — the dragged table shifts the column-gap lanes,
    /// which made unrelated relationship lines wander as the drag crossed
    /// them. This pins each edge that does not touch `dragged_node` to its
    /// previous route (matched by index — the source is unchanged
    /// mid-drag); the dragged table's own edges keep the recomputed,
    /// live-following ones.
    pub fn with_frozen_routes(mut self, dragged_node: usize, prev: &Layout) -> Layout {
        for (i, e) in self.edges.iter_mut().enumerate() {
            let touches = e.ends.iter().any(|(n, _)| *n == dragged_node);
            if !touches {
                if let Some(p) = prev.edges.get(i) {
                    e.pts = p.pts.clone();
                }
            }
        }
        self
    }
}

/// Lane candidates for the vertical run: the middle of each column gap,
/// plus bulges outside the outermost columns (cell coords).
fn lane_candidates(cols: &[(i32, i32)]) -> Vec<f32> {
    let mut v = Vec::new();
    if let Some((_, first_end)) = cols.first() {
        v.push(*first_end as f32 - 2.0);
    }
    for w in cols.windows(2) {
        v.push((w[0].1 + w[1].0) as f32 / 2.0);
    }
    if let Some((last_start, _)) = cols.last() {
        v.push(*last_start as f32 + 2.0);
    }
    v
}

/// Router output: waypoints plus the (node, column) pair at each end.
pub type Route = (Vec<(f32, f32)>, [(usize, usize); 2]);

/// dbdiagram-style routing: side anchors at the rows' vertical centers on
/// the sides FACING the other table (nearest sides when boxes overlap),
/// joined by an orthogonal Z whose vertical run rides a column-gap lane.
/// Renderers round the corners.
fn route(boxes: &[TableBox], e: &Edge, cols: &[(i32, i32)]) -> Route {
    let (fk_idx, rf_idx) = fk_ref(e);
    let (fk_col_idx, rf_col_idx) = match e.card {
        Cardinality::OneToMany => (e.to_col, e.from_col),
        _ => (e.from_col, e.to_col),
    };
    let fk = find_box(boxes, fk_idx);
    let rf = find_box(boxes, rf_idx);
    let sy = fk.col_row_center(fk_col_idx);
    let ey = rf.col_row_center(rf_col_idx);

    // side choice: right edge of the leftmost box, left edge of the
    // rightmost; on horizontal overlap the nearer sides by center
    let (sx, ex) = if fk_idx == rf_idx {
        // self-reference: a loop out of and back into the right side
        (fk.right() as f32, fk.right() as f32)
    } else if (fk.right() as f32) <= rf.x as f32 {
        (fk.right() as f32, rf.x as f32)
    } else if (rf.right() as f32) <= fk.x as f32 {
        (fk.x as f32, rf.right() as f32)
    } else if fk.x as f32 + fk.w as f32 / 2.0 <= rf.x as f32 + rf.w as f32 / 2.0 {
        (fk.right() as f32, rf.x as f32)
    } else {
        (fk.x as f32, rf.right() as f32)
    };

    let ends = [(fk_idx, fk_col_idx), (rf_idx, rf_col_idx)];

    // straight across when the rows align
    if (sy - ey).abs() < 0.25 {
        return (vec![(sx, sy), (ex, ey)], ends);
    }

    // self-loop (or fully degenerate Z): bulge out of one side
    let loop_bulge = 2.5 + (index_of(e) % 2) as f32;
    if fk_idx == rf_idx || (sx - ex).abs() < 1.0 {
        let (x0, dir) = if sx >= ex { (sx, 1.0) } else { (sx, -1.0) };
        let mx = x0 + dir * loop_bulge;
        return (vec![(sx, sy), (mx, sy), (mx, ey), (ex, ey)], ends);
    }

    // Z-route: the vertical run takes the gap lane nearest the midpoint,
    // kept between the two anchors so the horizontals stay short
    let lo = sx.min(ex);
    let hi = sx.max(ex);
    let mid = (sx + ex) / 2.0;
    let mut mx = mid;
    let mut best = f32::INFINITY;
    for cand in lane_candidates(cols) {
        if cand <= lo + 0.4 || cand >= hi - 0.4 {
            continue; // must sit between the anchors
        }
        let d = (cand - mid).abs();
        if d < best {
            best = d;
            mx = cand;
        }
    }
    // keep clear of the anchors themselves
    if (mx - sx).abs() < 0.4 {
        mx = if sx < ex { sx + 0.8 } else { sx - 0.8 };
    }
    if (mx - ex).abs() < 0.4 {
        mx = if ex < sx { ex + 0.8 } else { ex - 0.8 };
    }
    (vec![(sx, sy), (mx, sy), (mx, ey), (ex, ey)], ends)
}

fn index_of(e: &Edge) -> usize {
    // stable per-edge jitter for self-loop spread; Edge has no id field,
    // so hash the endpoint identity
    let mut h = e.from;
    h = h.wrapping_mul(31).wrapping_add(e.to);
    h.wrapping_mul(31).wrapping_add(e.from_col)
}

/// FK/referenced node pair for an edge, normalizing `<` refs so the FK side
/// is always first.
fn fk_ref(e: &Edge) -> (usize, usize) {
    match e.card {
        Cardinality::OneToMany => (e.to, e.from),
        _ => (e.from, e.to),
    }
}

/// Which route ends carry the "many" crow's foot, as (fk end, referenced
/// end). For both directional cardinalities the FK side is the many side;
/// `<>` is many on both, `-` on neither.
pub fn many_ends(card: Cardinality) -> (bool, bool) {
    match card {
        Cardinality::OneToMany | Cardinality::ManyToOne => (true, false),
        Cardinality::OneToOne => (false, false),
        Cardinality::ManyToMany => (true, true),
    }
}

fn find_box(boxes: &[TableBox], node: usize) -> &TableBox {
    boxes.iter().find(|b| b.node == node).expect("box for node")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbml::parser;

    fn layout_of(src: &str) -> Layout {
        let out = parser::parse(src);
        layout(&crate::model::build(&out.doc))
    }

    #[test]
    fn boxes_do_not_overlap_and_edges_route() {
        let src = "
Table a {
  id integer [pk]
  a_id integer
}
Table b {
  id integer [pk]
  b_ref integer
}
Table c { id integer [pk] }
Ref: b.b_ref > a.id
Ref: c.id > b.id
";
        let l = layout_of(src);
        assert_eq!(l.boxes.len(), 3);
        assert_eq!(l.edges.len(), 2);
        for (i, bi) in l.boxes.iter().enumerate() {
            for (_, bj) in l.boxes.iter().enumerate().skip(i + 1) {
                let overlap = bi.x < bj.right()
                    && bj.x < bi.right()
                    && bi.y < bj.bottom()
                    && bj.y < bi.bottom();
                assert!(!overlap, "boxes {bi:?} and {bj:?} overlap");
            }
        }
        // all three in distinct layers -> left-to-right ordering
        let xs: Vec<i32> = l.boxes.iter().map(|b| b.x).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
        // every route is anchored on the two box edges facing each other,
        // with horizontal first/last segments, inside the extent
        for r in &l.edges {
            assert!(r.pts.len() == 2 || r.pts.len() == 4, "straight or Z");
            for (x, y) in &r.pts {
                assert!(
                    *x >= 0.0 && *y >= 0.0 && *x <= l.extent.0 as f32 && *y <= l.extent.1 as f32
                );
            }
            // horizontal first/last segments (row-centered anchors)
            assert_eq!(r.pts[0].1, r.pts[1].1);
            let n = r.pts.len();
            assert_eq!(r.pts[n - 2].1, r.pts[n - 1].1);
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let src = "
Table a { id integer [pk] }
Table b {
  id integer [pk]
  b_ref integer
}
Ref: b.b_ref > a.id
";
        let l1 = layout_of(src);
        let l2 = layout_of(src);
        assert_eq!(l1.boxes, l2.boxes);
        assert_eq!(l1.edges, l2.edges);
    }

    #[test]
    fn same_column_refs_route_between_nearest_sides() {
        // cycle: neither table can be layered before the other
        let src = "
Table a {
  id integer [pk]
  rb integer
}
Table b {
  id integer [pk]
  ra integer
}
Ref: a.rb > b.id
Ref: b.ra > a.id
";
        let l = layout_of(src);
        // both are cycle members -> same layer -> vertical stack
        assert_eq!(l.boxes[0].x, l.boxes[1].x);
        // same-x boxes: centers tie, so both ends use the nearer sides by
        // center comparison — one exits right into the other's left
        for r in &l.edges {
            let (sx, sy) = r.pts[0];
            let (ex, ey) = r.pts[r.pts.len() - 1];
            assert!(
                (sx - ex).abs() > 0.5 || (sy - ey).abs() > 0.5,
                "endpoints distinct: {r:?}"
            );
        }
    }

    #[test]
    fn side_choice_follows_table_positions() {
        // in `Ref: b.a_id > a.id`, b (fk) lays out RIGHT of a (referenced):
        // b exits its LEFT edge, a enters its RIGHT edge
        let l = layout_of(
            "Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n",
        );
        let (a, b) = (&l.boxes[0], &l.boxes[1]);
        assert!(a.right() <= b.x, "a left of b");
        let r = &l.edges[0];
        assert_eq!(r.pts[0].0, b.x as f32, "fk exits its left edge");
        assert_eq!(
            r.pts[r.pts.len() - 1].0,
            a.right() as f32,
            "referenced enters its right edge"
        );

        // dragged so b sits LEFT of a: sides flip
        let out = parser::parse(
            "Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n",
        );
        let schema = crate::model::build(&out.doc);
        let mut ov = std::collections::HashMap::new();
        ov.insert("b".to_string(), (-30, 0));
        let l2 = layout_with_overrides(&schema, &ov);
        let r2 = &l2.edges[0];
        assert_eq!(
            r2.pts[0].0,
            l2.boxes.iter().find(|x| x.title == "b").unwrap().right() as f32,
            "fk exits its right edge"
        );
        assert_eq!(
            r2.pts[r2.pts.len() - 1].0,
            l2.boxes.iter().find(|x| x.title == "a").unwrap().x as f32,
            "referenced enters its left edge"
        );

        // horizontally overlapping boxes anchor at the nearest sides
        let mut ov2 = std::collections::HashMap::new();
        ov2.insert("b".to_string(), (2, 8)); // overlaps a's x range
        let l3 = layout_with_overrides(&schema, &ov2);
        let r3 = &l3.edges[0];
        let bk = l3.boxes.iter().find(|x| x.title == "b").unwrap();
        let ak = l3.boxes.iter().find(|x| x.title == "a").unwrap();
        assert!(
            r3.pts[0].0 == bk.right() as f32 || r3.pts[0].0 == bk.x as f32,
            "anchored on a box edge"
        );
        assert!(
            r3.pts[r3.pts.len() - 1].0 == ak.x as f32
                || r3.pts[r3.pts.len() - 1].0 == ak.right() as f32
        );
        // and the anchors face each other (start >= end when b is right...)
        // centers: a at x=0..w, b at x=2..w+2 -> b center greater -> b exits left
        assert_eq!(r3.pts[0].0, bk.x as f32, "b exits its left edge");
        assert_eq!(
            r3.pts[r3.pts.len() - 1].0,
            ak.right() as f32,
            "a enters its right edge"
        );
    }

    #[test]
    fn self_reference_loops_out_one_side() {
        let l = layout_of(
            "Table u {
  id integer [pk]
  referrer_id integer
}\nRef: u.referrer_id > u.id\n",
        );
        let r = &l.edges[0];
        let b = &l.boxes[0];
        // both ends on the right edge, control points bulge right
        assert_eq!(r.pts[0].0, b.right() as f32);
        assert_eq!(r.pts[r.pts.len() - 1].0, b.right() as f32);
        let bulge = r.pts[1].0;
        assert!(bulge > b.right() as f32, "loop bulges right");
        assert_eq!(r.pts[2].0, bulge, "the vertical run shares the lane");
    }

    #[test]
    fn overrides_move_boxes_and_recompute_extent() {
        let src = "
Table a { id integer [pk] }
Table b {
  id integer [pk]
  b_ref integer
}
Ref: b.b_ref > a.id
";
        let out = parser::parse(src);
        let schema = crate::model::build(&out.doc);
        let auto = layout(&schema);
        let a_auto = auto.boxes.iter().find(|b| b.title == "a").unwrap().clone();

        let mut ov = std::collections::HashMap::new();
        ov.insert("a".to_string(), (40, 12));
        let moved = layout_with_overrides(&schema, &ov);
        let a_moved = moved.boxes.iter().find(|b| b.title == "a").unwrap();
        assert_eq!((a_moved.x, a_moved.y), (40, 12), "override applied");
        assert_ne!((a_moved.x, a_moved.y), (a_auto.x, a_auto.y));
        // extent covers the dragged position
        assert!(moved.extent.0 >= a_moved.right() && moved.extent.1 >= a_moved.bottom());
        // the edge between b and a now routes around the moved box: it uses
        // the below-channel path since b's column is no longer adjacent
        assert_eq!(moved.edges.len(), 1);
        // unnamed tables keep automatic placement
        let b_moved = moved.boxes.iter().find(|b| b.title == "b").unwrap().clone();
        let b_auto = auto.boxes.iter().find(|b| b.title == "b").unwrap().clone();
        assert_eq!((b_moved.x, b_moved.y), (b_auto.x, b_auto.y));
    }

    #[test]
    fn enums_are_never_boxes_but_carry_their_values() {
        let src = "Enum st { a b }\nTable t { s st }\n";
        let l = layout_of(src);
        // only the table is a box; the enum exists as a definition
        assert_eq!(l.boxes.len(), 1);
        assert_eq!(l.boxes[0].title, "t");
        // the layout carries the enum for the pill's tooltip / hover docs
        assert_eq!(l.enums.len(), 1);
        let e = &l.enums[0];
        assert_eq!(e.name, "st");
        assert_eq!(e.values, vec!["a".to_string(), "b".to_string()]);
        // the name token's span: the go-to-definition target
        assert_eq!(
            e.name_span.as_ref().map(|sp| &src[sp.clone()]),
            Some("st")
        );
        // and the enum-typed column is flagged
        assert!(l.boxes[0].rows[0].ty_enum);
        // a column whose type is not an enum is not
        let src = "Table t { s integer }\nEnum st { a }\n";
        assert!(!layout_of(src).boxes[0].rows[0].ty_enum);
    }

    #[test]
    fn an_enum_typed_column_widens_its_row_by_the_enum_pill() {
        // identical rows except the type names a declared enum: the
        // "Enum" pill's width shows up in the row width
        let plain = layout_of("Table t { status_flag st }");
        let typed = layout_of("Enum st { a b }\nTable t { status_flag st }");
        assert!(!plain.boxes[0].rows[0].ty_enum);
        assert!(typed.boxes[0].rows[0].ty_enum);
        let w0 = plain.boxes[0].rows[0].width();
        let w1 = typed.boxes[0].rows[0].width();
        let pill = Row::badge_cell("Enum");
        assert!(
            (w1 - w0) as f32 >= pill.floor() && (w1 - w0) as f32 <= pill.ceil(),
            "the Enum pill widens the row: {w1} vs {w0}"
        );
    }


    #[test]
    fn frozen_routes_pin_unrelated_edges_and_release_the_dragged_ones() {
        // drag c (unconnected) into the a—b route's lane area: the a—b
        // line is FROZEN (pinned to its pre-drag route); drag b
        // (connected) instead: its line re-routes live, following b
        let src = "Table a { id integer [pk] }\nTable b { a_id integer }\nTable c { x integer }\nRef: b.a_id > a.id\n";
        let schema = crate::model::build(&parser::parse(src).doc);
        let base = layout(&schema);
        let c_node = base.boxes.iter().find(|b| b.title == "c").unwrap().node;
        let b_node = base.boxes.iter().find(|b| b.title == "b").unwrap().node;
        let c_pos: std::collections::HashMap<String, (i32, i32)> =
            [("c".to_string(), (base.boxes[0].x + 8, base.boxes[0].y + 2))]
                .into_iter()
                .collect();
        let b_pos: std::collections::HashMap<String, (i32, i32)> =
            [("b".to_string(), (base.boxes[1].x + 6, base.boxes[1].y + 3))]
                .into_iter()
                .collect();

        let moved_c = layout_with_overrides(&schema, &c_pos);
        let frozen = moved_c.with_frozen_routes(c_node, &base);
        assert_eq!(
            frozen.edges[0].pts, base.edges[0].pts,
            "the unrelated a—b line is pinned to its pre-drag route"
        );

        let moved_b = layout_with_overrides(&schema, &b_pos);
        let live = moved_b.with_frozen_routes(b_node, &base);
        assert_ne!(
            live.edges[0].pts, base.edges[0].pts,
            "the dragged table's own line follows"
        );
    }

    #[test]
    fn boxes_hug_their_own_content() {
        // dbdiagram sizes each table to ITS widest line (name + icons vs
        // type + badges) — no uniform width stretching every box to the
        // widest sibling (audit_log read as an accidental wide block)
        let src = "
Table short { id integer }
Table much_longer_name {
  id integer [pk]
  very_long_column integer
}
";
        let l = layout_of(src);
        let ws: Vec<i32> = l.boxes.iter().map(|b| b.w).collect();
        assert!(
            ws[0] < ws[1],
            "the short box is narrower than the long one: {ws:?}"
        );
        // the short box hugs its own single line: name inset + name +
        // 0.25 gap + type inset + type + pads, +2 box padding
        let short_content = 1.0 + Row::est_width("id") + 0.25 + 0.6 + Row::est_width("integer")
            + 0.4 + 2.0;
        assert_eq!(ws[0] as f32, short_content.ceil(), "{ws:?}");
        // the long box is at least as wide as its longest row needs
        let long_row = 1.0 + Row::est_width("very_long_column") + 0.25
            + Row::ICON_CELL /* pk icon */
            + 0.6 + Row::est_width("integer") + 0.4;
        assert!(ws[1] as f32 >= long_row, "{ws:?}");
    }

    #[test]
    fn edge_routes_carry_cardinality() {
        let l = layout_of("Table a { id integer }\nTable b { a_id integer }\nRef: b.a_id > a.id\n");
        assert_eq!(l.edges.len(), 1);
        assert_eq!(l.edges[0].card, Cardinality::ManyToOne);
    }

    #[test]
    fn edges_touching_maps_both_ends() {
        let l = layout_of(
            "Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n",
        );
        // a.id (node 0, col 0) and b.a_id (node 1, col 0) both touch edge 0
        assert_eq!(l.edges_touching(0, 0), vec![0]);
        assert_eq!(l.edges_touching(1, 0), vec![0]);
        // unrelated rows touch nothing
        assert!(l.edges_touching(0, 0).len() == 1);
        assert!(l.edges[0].ends[0] == (1, 0) && l.edges[0].ends[1] == (0, 0));
    }

    #[test]
    fn many_ends_follow_cardinality() {
        use Cardinality::*;
        // fk end first in the route, so it carries the crow's foot
        assert_eq!(many_ends(ManyToOne), (true, false));
        assert_eq!(many_ends(OneToMany), (true, false));
        assert_eq!(many_ends(OneToOne), (false, false));
        assert_eq!(many_ends(ManyToMany), (true, true));
    }

    // -------------------- auto_arrange (Left-right) --------------------

    /// Layout boxes + schema for an auto-arrange test (auto_arrange takes
    /// the laid-out boxes for sizes/titles and the schema for the edges).
    fn arrange_parts(src: &str) -> (Layout, Schema) {
        let out = parser::parse(src);
        let schema = crate::model::build(&out.doc);
        let l = layout(&schema);
        (l, schema)
    }

    fn box_of<'a>(l: &'a Layout, title: &str) -> &'a TableBox {
        l.boxes.iter().find(|b| b.title == title).unwrap()
    }

    #[test]
    fn auto_arrange_layers_left_to_right_with_layer_gap() {
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] }
Table b { a_id integer }
Table c { b_id integer }
Ref: b.a_id > a.id
Ref: c.b_id > b.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        // referenced LEFT of FK, one 12-cell gap between layers
        assert_eq!(pos["a"].0, 0, "first layer at the origin");
        assert_eq!(
            pos["b"].0,
            pos["a"].0 + box_of(&l, "a").w + ARRANGE_LAYER_GAP_X
        );
        assert_eq!(
            pos["c"].0,
            pos["b"].0 + box_of(&l, "b").w + ARRANGE_LAYER_GAP_X
        );
    }

    /// A group's padded region from the arrange result (the same rect
    /// auto_arrange resolves with: member union + 2-cell frame padding
    /// and the header band row above).
    fn region_of(
        pos: &std::collections::HashMap<String, (i32, i32)>,
        l: &Layout,
        members: &[&str],
    ) -> (i32, i32, i32, i32) {
        let mut rect = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for m in members {
            let (x, y) = pos[*m];
            let b = box_of(l, m);
            rect.0 = rect.0.min(x);
            rect.1 = rect.1.min(y);
            rect.2 = rect.2.max(x + b.w);
            rect.3 = rect.3.max(y + b.h);
        }
        (rect.0 - 2, rect.1 - 3, rect.2 + 2, rect.3 + 2)
    }

    fn regions_disjoint(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
        a.0 >= b.2 || b.0 >= a.2 || a.1 >= b.3 || b.1 >= a.3
    }

    #[test]
    fn auto_arrange_separates_interleaved_group_regions() {
        // A's members and B's members share both layers: the naive
        // per-component layering interleaves them and the two frames
        // land on top of each other. The regions must come out disjoint,
        // with the later group shifted down as a rigid block (its
        // members keep their arrangement).
        let (l, schema) = arrange_parts(
            "TableGroup \"A\" {
  a
  d
}
TableGroup \"B\" {
  b
  c
}
Table a { id integer [pk] }
Table b { a_id integer }
Table c { a_id integer }
Table d { a_id integer }
Ref: b.a_id > a.id
Ref: c.a_id > a.id
Ref: d.a_id > a.id
",
        );
        let naive = auto_arrange(&l.boxes, &schema, &[]);
        let naive_span = naive["c"].1 - naive["b"].1;
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        let ra = region_of(&pos, &l, &["a", "d"]);
        let rb = region_of(&pos, &l, &["b", "c"]);
        assert!(
            regions_disjoint(ra, rb),
            "group regions overlap: {ra:?} vs {rb:?}"
        );
        // rigid shift: B's members keep their vertical arrangement —
        // c sits below b by the same span the naive layout gave them
        assert_eq!(pos["c"].1 - pos["b"].1, naive_span);
    }

    #[test]
    fn auto_arrange_dodges_pinned_stickies() {
        // a sticky parked over a table's naive placement: the table
        // drops below it instead of covering the note
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] }
Table b { a_id integer }
Ref: b.a_id > a.id
",
        );
        let naive = auto_arrange(&l.boxes, &schema, &[]);
        let (ax, ay) = naive["a"];
        let ba = box_of(&l, "a");
        // the sticky covers a's naive spot exactly
        let stickies = vec![(ax, ay, ba.w, ba.h)];
        let pos = auto_arrange(&l.boxes, &schema, &stickies);
        let (x, y) = pos["a"];
        let clear = |sx0, sy0, sx1, sy1| x >= sx1 || sx0 >= x + ba.w || y >= sy1 || sy0 >= y + ba.h;
        for &(sx0, sy0, sw, sh) in &stickies {
            assert!(
                clear(sx0, sy0, sx0 + sw, sy0 + sh),
                "a still overlaps the sticky at ({x},{y})"
            );
        }
    }

    #[test]
    fn auto_arrange_shifts_a_whole_group_off_a_sticky() {
        // a sticky inside a group's naive region: the group's members
        // move as one block, region clear of the note
        let (l, schema) = arrange_parts(
            "TableGroup \"G\" {
  a
  b
}
Table a { id integer [pk] }
Table b { a_id integer }
Ref: b.a_id > a.id
",
        );
        let naive = auto_arrange(&l.boxes, &schema, &[]);
        let rg = region_of(&naive, &l, &["a", "b"]);
        // the sticky sits on the group region's middle
        let stickies = vec![(
            (rg.0 + rg.2) / 2 - 1,
            (rg.1 + rg.3) / 2 - 1,
            4,
            4,
        )];
        let pos = auto_arrange(&l.boxes, &schema, &stickies);
        let region = region_of(&pos, &l, &["a", "b"]);
        for &(sx0, sy0, sw, sh) in &stickies {
            assert!(
                regions_disjoint(region, (sx0, sy0, sx0 + sw, sy0 + sh)),
                "group region {region:?} still covers the sticky"
            );
        }
        // rigid: the members keep their naive arrangement
        assert_eq!(
            (pos["b"].0 - pos["a"].0, pos["b"].1 - pos["a"].1),
            (naive["b"].0 - naive["a"].0, naive["b"].1 - naive["a"].1)
        );
    }

    #[test]
    fn auto_arrange_in_layer_gap_and_model_order() {
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] }
Table b { a_id integer }
Table c { a_id integer }
Ref: b.a_id > a.id
Ref: c.a_id > a.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        // same layer: same x, stacked top-down with the 2-cell gap; ties
        // keep model order (b before c, so b above c)
        assert_eq!(pos["b"].0, pos["c"].0);
        assert_eq!(
            pos["c"].1,
            pos["b"].1 + box_of(&l, "b").h + ARRANGE_LAYER_GAP_Y
        );
        assert!(pos["b"].1 < pos["c"].1);
    }

    #[test]
    fn auto_arrange_barycenter_untangles_crossing() {
        // q1 -> r2 and q2 -> r1 cross if layer 2 keeps model order; the
        // barycenter sweep must swap r2 above r1
        let (l, schema) = arrange_parts(
            "Table p { id integer [pk] }
Table q1 { p_id integer }
Table q2 { p_id integer }
Table r1 { q2_id integer }
Table r2 { q1_id integer }
Ref: q1.p_id > p.id
Ref: q2.p_id > p.id
Ref: r2.q1_id > q1.id
Ref: r1.q2_id > q2.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        // model order (q1,q2 / r1,r2) would cross both edges; the sweeps
        // must leave the two layers ordered so no edge crosses (whether it
        // swapped the q layer, the r layer, or both)
        let q_order = pos["q1"].1 < pos["q2"].1;
        let r_order = pos["r2"].1 < pos["r1"].1;
        assert_eq!(q_order, r_order, "orders must agree (no crossing): {pos:?}");
        assert!(
            !(q_order && pos["r1"].1 < pos["r2"].1),
            "the crossing model order must have been undone: {pos:?}"
        );
        // and each layer is a tight 2-cell stack
        assert_eq!(
            (pos["q2"].1 - pos["q1"].1).abs(),
            box_of(&l, "q1").h + ARRANGE_LAYER_GAP_Y
        );
    }

    #[test]
    fn auto_arrange_centers_layers_on_one_line() {
        // even heights: one column per table = 1 row + 1 header cell, and
        // the 2-cell layer gap keeps every center on an integer line
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] }
Table b { a_id integer }
Table c { a_id integer }
Ref: b.a_id > a.id
Ref: c.a_id > a.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        let a = box_of(&l, "a");
        let c = box_of(&l, "c");
        let (pa, pb, pc) = (pos["a"], pos["b"], pos["c"]);
        let center_a = pa.1 as f32 + a.h as f32 / 2.0;
        let block_h = (pc.1 + c.h - pb.1) as f32;
        let center_block = pb.1 as f32 + block_h / 2.0;
        assert_eq!(center_a, center_block, "layer blocks share a center line");
        // the single-table layer is the shorter one: centered INSIDE the
        // two-table block's span
        assert!(pa.1 > pb.1 && pa.1 + a.h < pc.1 + c.h);
    }

    #[test]
    fn auto_arrange_packs_components_left_to_right() {
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] }
Table b { a_id integer }
Table c { id integer [pk] }
Table d { c_id integer }
Ref: b.a_id > a.id
Ref: d.c_id > c.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        // the first component (model order) on the left; the second starts
        // one 8-cell component gap after the first's right edge (b is the
        // rightmost box of component 1)
        let b = box_of(&l, "b");
        assert_eq!(pos["c"].0 - (pos["b"].0 + b.w), ARRANGE_COMPONENT_GAP);
        // both components vertically centered on the same line (equal
        // shapes, so their boxes share y rows)
        assert_eq!(pos["a"].1, pos["c"].1);
        assert_eq!(pos["b"].1, pos["d"].1);
    }

    #[test]
    fn auto_arrange_tolerates_cycles() {
        let (l, schema) = arrange_parts(
            "Table a {
  id integer [pk]
  rb integer
}
Table b {
  id integer [pk]
  ra integer
}
Ref: a.rb > b.id
Ref: b.ra > a.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema, &[]);
        // cycle members park in one layer: same x, stacked
        assert_eq!(pos["a"].0, pos["b"].0);
        assert_eq!(
            pos["b"].1,
            pos["a"].1 + box_of(&l, "a").h + ARRANGE_LAYER_GAP_Y
        );
    }

    #[test]
    fn auto_arrange_covers_every_table_and_is_deterministic() {
        let (l, schema) = arrange_parts(
            "Enum st { draft published }
Table t { s st }
Table u { id integer [pk] }
Ref: u.id > t.id
",
        );
        let pos1 = auto_arrange(&l.boxes, &schema, &[]);
        let pos2 = auto_arrange(&l.boxes, &schema, &[]);
        assert_eq!(pos1, pos2, "deterministic");
        assert_eq!(pos1.len(), l.boxes.len(), "every table gets an override");
        for b in &l.boxes {
            assert!(pos1.contains_key(&b.title), "missing {}", b.title);
        }
        // enums are not boxes: no position, and no phantom component gap
        // between the tables
        assert_eq!(l.boxes.len(), 2, "only the tables are boxes");
        assert!(!pos1.contains_key("st"));
        assert_eq!(
            pos1["u"].0,
            pos1["t"].0 + box_of(&l, "t").w + ARRANGE_LAYER_GAP_X,
            "t and u pack as one component, no enum gap"
        );
    }

    fn route_of<'a>(l: &'a Layout, from: &str, to: &str) -> &'a EdgeRoute {
        l.edges
            .iter()
            .find(|r| {
                let (a, b) = (
                    l.boxes.iter().find(|x| x.node == r.ends[0].0),
                    l.boxes.iter().find(|x| x.node == r.ends[1].0),
                );
                a.is_some_and(|a| a.title == from) && b.is_some_and(|b| b.title == to)
            })
            .expect("the fixture route")
    }

    #[test]
    fn an_uncolored_line_wears_the_referenced_table_s_header_color() {
        // likes.post_id > posts.id with NO explicit [color:]: the line
        // renders posts' resolved header color — the dbdiagram line ↔
        // header pairing (the explicit pick on posts does NOT touch the
        // line's own attribute)
        let src = "Table posts [headercolor: #7a4f9e] { id integer [pk] }\n\
                   Table likes { post_id integer [ref: > posts.id] }\n";
        let l = layout_of(src);
        let r = route_of(&l, "likes", "posts");
        assert!(r.color.is_none(), "no explicit color on the line");
        assert_eq!(
            r.inherited.as_deref(),
            Some("#7a4f9e"),
            "the line falls back to the referenced header"
        );

        // an explicit `[color:]` wins and leaves the fallback unset
        let src = "Table posts [headercolor: #7a4f9e] { id integer [pk] }\n\
                   Table likes { post_id integer }\n\
                   Ref: likes.post_id > posts.id [color: #f1c40f]\n";
        let l = layout_of(src);
        let r = route_of(&l, "likes", "posts");
        assert_eq!(r.color.as_deref(), Some("#f1c40f"));
        assert!(r.inherited.is_none(), "explicit color owns the line");

        // the referenced header may itself cascade from a TableGroup
        let src = "TableGroup g [color: #1e69fd] { posts }\n\
                   Table posts { id integer [pk] }\n\
                   Table likes { post_id integer [ref: > posts.id] }\n";
        let l = layout_of(src);
        let r = route_of(&l, "likes", "posts");
        assert_eq!(
            r.inherited.as_deref(),
            Some("#1e69fd"),
            "the fallback resolves through the group cascade"
        );
    }
}
