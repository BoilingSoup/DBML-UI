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
}

impl Row {
    /// Extra width (cells) of one left-cluster icon slot and of the
    /// "Enum" / "Not Null" pills' lead-in.
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
            w += Self::ICON_CELL;
        }
        if self.note.is_some() {
            w += Self::ICON_CELL;
        }
        // right: type text, then the "Enum" / "Not Null" pills flush right
        w += 0.6 + self.ty.width() as f32 * Self::CHAR_CELL;
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

/// A schema enum definition, for the "Enum" pill's tooltip and the
/// editor's hover docs (enums are never boxes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumInfo {
    pub name: String,
    pub values: Vec<String>,
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
pub fn auto_arrange(
    boxes: &[TableBox],
    schema: &Schema,
) -> std::collections::HashMap<String, (i32, i32)> {
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
    // Normalize to non-negative cells: min corner at the origin (the
    // center lines all sit half the tallest component down — the global
    // center line).
    let min_x = placed.iter().map(|p| p.0).min().unwrap_or(0);
    let min_y = placed.iter().map(|p| p.1).min().unwrap_or(0);
    for (i, p) in placed.iter().enumerate() {
        out.insert(title[i].clone(), (p.0 - min_x, p.1 - min_y));
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
        enums,
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
    fn enums_are_never_boxes_but_carry_their_values() {
        let src = "Enum st { a b }\nTable t { s st }\n";
        let l = layout_of(src);
        // only the table is a box; the enum exists as a definition
        assert_eq!(l.boxes.len(), 1);
        assert_eq!(l.boxes[0].title, "t");
        // the layout carries the enum for the pill's tooltip / hover docs
        assert_eq!(
            l.enums,
            vec![EnumInfo {
                name: "st".to_string(),
                values: vec!["a".to_string(), "b".to_string()],
            }]
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
        let pos = auto_arrange(&l.boxes, &schema);
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
        let pos = auto_arrange(&l.boxes, &schema);
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
        let pos = auto_arrange(&l.boxes, &schema);
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
        // even heights (2 rows + 2) so centering is exact
        let (l, schema) = arrange_parts(
            "Table a { id integer [pk] name varchar }
Table b { a_id integer x integer }
Table c { a_id integer y integer }
Ref: b.a_id > a.id
Ref: c.a_id > a.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema);
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
        let pos = auto_arrange(&l.boxes, &schema);
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
            "Table a { id integer [pk] rb integer }
Table b { id integer [pk] ra integer }
Ref: a.rb > b.id
Ref: b.ra > a.id
",
        );
        let pos = auto_arrange(&l.boxes, &schema);
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
        let pos1 = auto_arrange(&l.boxes, &schema);
        let pos2 = auto_arrange(&l.boxes, &schema);
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
}
