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
    /// One icon slot in the left cluster: a 10.5px glyph plus its 4px gap,
    /// in cells (their WN.PADDING_X = 4 at the 20px 1:1 cell).
    pub const ICON_CELL: f32 = (10.5 + 4.0) / 20.0;
    /// One text-pill badge: glyph run plus its padding/gap, in cells.
    pub const BADGE_CELL: f32 = 0.456; // one badge-font char in cells
    pub fn badge_cell(text: &str) -> f32 {
        0.8 + text.chars().count() as f32 * Self::BADGE_CELL
    }
    /// Estimated cells per character of sans text at the row font:
    /// ~0.5em average advance with slack, mirroring the canvas painter's
    /// `char_px` so boxes hug their content like dbdiagram's measureText.
    pub const CHAR_CELL: f32 = 0.58;

    pub fn width(&self) -> i32 {
        // left: name inset, then dbdiagram's icon cluster (pk, fk, unique,
        // note — each 10.5px glyph + 4px gap)
        let mut w = 1.0 + self.name.width() as f32 * Self::CHAR_CELL + 0.25;
        if self.pk {
            w += Self::ICON_CELL;
        }
        if self.fk {
            w += Self::ICON_CELL;
        }
        if self.unique {
            w += Self::badge_cell("Unique"); // text pill in the cluster slot
        }
        if self.note.is_some() {
            w += Self::ICON_CELL;
        }
        // right: type text, then the "Not Null" pill flush right
        w += 0.6 + self.ty.width() as f32 * Self::CHAR_CELL;
        if self.not_null == Some(true) {
            w += Self::badge_cell("Not Null");
        }
        w += 0.4; // right inner padding
        (w.max(6.0)).ceil() as i32
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
    /// `records` sample rows (resolved model form), for the record view
    /// and the canvas affordance; None for tables without them.
    pub records: Option<crate::model::NodeRecords>,
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
}

#[derive(Debug, Clone, PartialEq, Default)]
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

    // Boxes, sized from their content.
    let boxes: Vec<TableBox> = schema
        .nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| {
            let title = node.name.clone();
            // enum type names, for the right-side enum pill flag
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
                records: node.records.clone(),
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

    let edges = schema
        .edges
        .iter()
        .enumerate()
        .map(|(ei, e)| {
            let (pts, ends) = route(&placed, e, &cols);
            EdgeRoute {
                edge: ei,
                pts,
                ends,
                card: e.card,
            }
        })
        .collect();

    Layout {
        boxes: placed,
        edges,
        cols,
        extent: (max_right, max_bottom + 1),
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
Table b { id integer [pk] b_ref integer }
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
Table a { id integer [pk] rb integer }
Table b { id integer [pk] ra integer }
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
            "Table u { id integer [pk] referrer_id integer }\nRef: u.referrer_id > u.id\n",
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
}
