//! Deterministic layered layout, shared by the text and image renderers and
//! by mouse hit-testing. Coordinates are in terminal cells.

use unicode_width::UnicodeWidthStr;

use crate::dbml::ast::Cardinality;
use crate::dbml::ast::Span;
use crate::model::{Edge, NodeKind, Schema};

pub const GAP_X: i32 = 5;
pub const GAP_Y: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pk: bool,
    pub fk: bool,
    pub name: String,
    pub ty: String,
    /// Some(true) = not null, Some(false) = explicit null.
    pub not_null: Option<bool>,
    pub unique: bool,
    pub note: Option<String>,
}

impl Row {
    /// Extra width (cells) of the "Not Null" / "Unique" pills and the
    /// trailing note icon.
    pub const BADGE_CELL: f32 = 0.456; // one badge-font char in cells

    pub fn badges(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.not_null == Some(true) {
            v.push("Not Null");
        }
        if self.unique {
            v.push("Unique");
        }
        v
    }

    pub fn width(&self) -> i32 {
        let mut w = 2.0 + self.name.width() as f32 + 1.9 + 0.5; // pad + name + icon band + gap
        for b in self.badges() {
            w += 0.8 + b.chars().count() as f32 * Self::BADGE_CELL;
        }
        w += 1.0 + self.ty.width() as f32; // gap + type
        if self.note.is_some() {
            w += 1.6; // note icon zone at the right edge
        }
        w.ceil() as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBox {
    /// Index into `Schema::nodes`.
    pub node: usize,
    pub kind: NodeKind,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
    /// `Table users as U` — the alias, for hover-docs and lookups.
    pub alias: Option<String>,
    pub header_color: Option<String>,
    pub rows: Vec<Row>,
    /// Span of the table/enum name in the source (click-to-jump target).
    pub name_span: Option<Span>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRoute {
    /// Index into `Schema::edges`.
    pub edge: usize,
    pub pts: Vec<(i32, i32)>,
    /// Cardinality, for the crow's-foot / tick end markers.
    pub card: Cardinality,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub boxes: Vec<TableBox>,
    pub edges: Vec<EdgeRoute>,
    /// (start_x, end_x) of each layer column, for gap routing.
    pub cols: Vec<(i32, i32)>,
    /// Diagram content extents (max right, max bottom).
    pub extent: (i32, i32),
}

/// Longest-path layering over ref edges (cycles tolerated: Kahn order, with
/// cycle members appended after the last layer), then column packing.
pub fn layout(schema: &Schema) -> Layout {
    layout_with_overrides(schema, &std::collections::HashMap::new())
}

/// Layout with user-dragged table positions (diagram coordinates, keyed by
/// table/enum name). Overridden boxes keep their dragged position; column
/// extents, content bounds, and edge routes are computed from the final
/// positions. New/renamed tables fall back to the automatic placement.
pub fn layout_with_overrides(
    schema: &Schema,
    overrides: &std::collections::HashMap<String, (i32, i32)>,
) -> Layout {
    let n = schema.nodes.len();

    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    // Layering puts the referenced table first and FK tables after it, so
    // diagrams read left-to-right like dbdiagram's.
    for e in &schema.edges {
        let (fk, rf) = fk_ref(e);
        if !adj[rf].contains(&fk) {
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

    // Boxes, sized from their content.
    let boxes: Vec<TableBox> = schema
        .nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| {
            let title = node.name.clone();
            let rows = node
                .columns
                .iter()
                .map(|c| Row {
                    pk: c.pk,
                    fk: c.fk,
                    name: c.name.clone(),
                    ty: c.ty.clone(),
                    not_null: c.not_null,
                    unique: c.unique,
                    note: c.note.clone(),
                })
                .collect::<Vec<_>>();
            let content = rows
                .iter()
                .map(|r| r.width())
                .chain(std::iter::once(title.width() as i32 + 2))
                .max()
                .unwrap_or(6);
            TableBox {
                node: idx,
                kind: node.kind,
                x: 0,
                y: 0,
                w: content + 4,
                h: rows.len() as i32 + 2,
                title,
                alias: node.alias.clone(),
                header_color: node.header_color.clone(),
                rows,
                name_span: node.name_span.clone(),
            }
        })
        .collect();

    // dbdiagram-style uniform width: every box as wide as the widest one
    // (with a floor), so columns of tables read as tidy equal blocks.
    let uniform_w = boxes.iter().map(|b| b.w).max().unwrap_or(6).max(16);
    let mut boxes = boxes;
    for b in &mut boxes {
        b.w = uniform_w;
    }

    // Column packing: one column of tables per layer, doc order within.
    // All boxes in a column are left-aligned; `cols` records each column's
    // extent so edge routing can use the gaps between columns.
    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut placed = Vec::new();
    let mut cols: Vec<(i32, i32)> = Vec::new();
    let mut x = 0i32;
    let mut max_right = 0i32;
    let mut max_bottom = 0i32;
    for l in 0..=max_layer {
        let mut y = 0i32;
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

    let channel_base = max_bottom + 1;
    let ctx = RouteCtx {
        cols: cols.clone(),
        layer: layer.clone(),
        channel_base,
    };
    let edges = schema
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| EdgeRoute {
            edge: ei,
            pts: route(&placed, e, ei, &ctx),
            card: e.card,
        })
        .collect();

    Layout {
        boxes: placed,
        edges,
        cols,
        extent: (max_right, channel_base + 2),
    }
}

struct RouteCtx {
    cols: Vec<(i32, i32)>,
    layer: Vec<usize>,
    channel_base: i32,
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

/// Orthogonal routes that never cross a table box:
/// - adjacent columns: short jog through the shared gap
/// - anything else: bus route down a clear channel below all content,
///   with vertical trunks in the column gaps
fn route(boxes: &[TableBox], e: &Edge, index: usize, ctx: &RouteCtx) -> Vec<(i32, i32)> {
    let (fk_idx, rf_idx) = fk_ref(e);
    let (fk_col_idx, rf_col_idx) = match e.card {
        Cardinality::OneToMany => (e.to_col, e.from_col),
        _ => (e.from_col, e.to_col),
    };
    let fk = find_box(boxes, fk_idx);
    let rf = find_box(boxes, rf_idx);
    let fy = fk.col_row_y(fk_col_idx);
    let ry = rf.col_row_y(rf_col_idx);
    let lane = (index as i32 % 3) - 1;
    let channel_y = ctx.channel_base + (index as i32 % 3);

    let (fk_col, rf_col) = (ctx.layer[fk_idx], ctx.layer[rf_idx]);
    if fk_col == rf_col + 1 {
        // short jog through the gap between the two columns
        let sx = fk.x - 1;
        let ex = rf.right();
        let mid = ex + (sx - ex) / 2 + lane;
        vec![(sx, fy), (mid, fy), (mid, ry), (ex, ry)]
    } else if fk_col > rf_col {
        // FK column right of referenced: exit FK's left gap, approach the
        // referenced table's right side
        let (fk_start, _) = ctx.cols[fk_col];
        let (_, rf_end) = ctx.cols[rf_col];
        let trunk = fk_start - 2 - lane;
        let approach = rf_end + 2 + lane;
        vec![
            (fk.x - 1, fy),
            (trunk, fy),
            (trunk, channel_y),
            (approach, channel_y),
            (approach, ry),
            (rf.right(), ry),
        ]
    } else {
        // same column (cycles) or anomalous layout: exit right, approach left
        let (_, fk_end) = ctx.cols[fk_col];
        let (rf_start, _) = ctx.cols[rf_col];
        let trunk = fk_end + 2 + lane;
        let approach = rf_start - 2 - lane;
        vec![
            (fk.right(), fy),
            (trunk, fy),
            (trunk, channel_y),
            (approach, channel_y),
            (approach, ry),
            (rf.x - 1, ry),
        ]
    }
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
Table a { id integer [pk] a_id integer }
Table b { id integer [pk] b_ref integer }
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
        // every edge route has 4 points inside extent
        for r in &l.edges {
            assert_eq!(r.pts.len(), 4);
            for (x, y) in &r.pts {
                assert!(*x >= 0 && *y >= 0 && *x <= l.extent.0 && *y <= l.extent.1);
            }
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let src = "
Table a { id integer [pk] }
Table b { id integer [pk] b_ref integer }
Ref: b.b_ref > a.id
";
        let l1 = layout_of(src);
        let l2 = layout_of(src);
        assert_eq!(l1.boxes, l2.boxes);
        assert_eq!(l1.edges, l2.edges);
    }

    #[test]
    fn same_column_refs_route_below() {
        // cycle: neither table can be layered before the other
        let src = "
Table a { id integer [pk] rb integer }
Table b { id integer [pk] ra integer }
Ref: a.rb > b.id
Ref: b.ra > a.id
";
        let l = layout_of(src);
        // both are cycle members -> same layer -> vertical stack
        assert_eq!(l.boxes[0].x, l.boxes[1].x);
        let r = &l.edges[0];
        // bus route dips below all content: channel y past both bottoms
        let channel_y = r.pts[2].1;
        assert!(
            channel_y > l.boxes[0].bottom() && channel_y > l.boxes[1].bottom(),
            "route {r:?} boxes {:?}",
            l.boxes
        );
    }

    #[test]
    fn overrides_move_boxes_and_recompute_extent() {
        let src = "
Table a { id integer [pk] }
Table b { id integer [pk] b_ref integer }
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
    fn enums_lay_out_as_boxes() {
        let src = "Enum st { a b }\nTable t { s st }\n";
        let l = layout_of(src);
        assert_eq!(l.boxes.len(), 2);
        let st = l.boxes.iter().find(|b| b.title == "st").unwrap();
        assert_eq!(st.h, 4); // 2 values + header + bottom border
    }

    #[test]
    fn boxes_share_a_uniform_width() {
        // dbdiagram renders all tables at one equal width (the widest one)
        let src = "
Table short { id integer }
Table much_longer_name { id integer [pk] very_long_column integer }
";
        let l = layout_of(src);
        let ws: Vec<i32> = l.boxes.iter().map(|b| b.w).collect();
        assert!(ws.windows(2).all(|w| w[0] == w[1]), "{ws:?}");
        // the short box alone needs only 16 cells; uniform width must have
        // been stretched past its own content by the wide sibling
        let short_row = 2 + 2 + 1 + 7; // "id integer"
        assert_eq!(short_row, 12);
        assert!(ws[0] > short_row + 4, "stretched: {ws:?}");
        assert!(ws[0] >= 16, "minimum width floor");
    }

    #[test]
    fn edge_routes_carry_cardinality() {
        let l = layout_of("Table a { id integer }\nTable b { a_id integer }\nRef: b.a_id > a.id\n");
        assert_eq!(l.edges.len(), 1);
        assert_eq!(l.edges[0].card, Cardinality::ManyToOne);
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
}
