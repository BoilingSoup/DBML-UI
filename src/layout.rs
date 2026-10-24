//! Deterministic layered layout, shared by the text and image renderers and
//! by mouse hit-testing. Coordinates are in terminal cells.

use unicode_width::UnicodeWidthStr;

use crate::dbml::ast::Cardinality;
use crate::model::{Edge, NodeKind, Schema};

pub const GAP_X: i32 = 5;
pub const GAP_Y: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pk: bool,
    pub fk: bool,
    pub name: String,
    pub ty: String,
}

impl Row {
    /// "◆ name type" width in cells.
    pub fn width(&self) -> i32 {
        2 + self.name.width() as i32 + 1 + self.ty.width() as i32
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
    pub header_color: Option<String>,
    pub rows: Vec<Row>,
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
    for i in 0..n {
        if !order.contains(&i) {
            layer[i] = max_layer + 1;
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
                header_color: node.header_color.clone(),
                rows,
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

fn find_box<'a>(boxes: &'a [TableBox], node: usize) -> &'a TableBox {
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
    fn enums_lay_out_as_boxes() {
        let src = "Enum st { a b }\nTable t { s st }\n";
        let l = layout_of(src);
        assert_eq!(l.boxes.len(), 2);
        let st = l.boxes.iter().find(|b| b.title == "st").unwrap();
        assert_eq!(st.h, 4); // 2 values + header + bottom border
    }
}
