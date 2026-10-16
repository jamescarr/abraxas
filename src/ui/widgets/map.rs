//! The host map: nodes as `[gate2]` labels on a layered grid, joined by box-drawing routes.
//!
//! Pure functions over [`MapNode`]s. [`MapGrid::build`] turns the layout data of
//! `HostSession::map_layout` into a grid of glyph cells; [`MapGrid::render`] blits a clipped,
//! scrolled window of it. Two scales exist: the full `scan` map (column = layer x 14, row =
//! index x 3, nodes as `[id]` labels) and a compact one for the Run screen's side panel (column =
//! layer x 4, row = index x 2, nodes as single glyphs).

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Style};

use crate::game::host::MapNode;
use crate::palette::{Dos, Palette};

/// Columns per BFS layer in the full map.
pub const FULL_COL_STRIDE: usize = 14;
/// Rows per node row in the full map.
pub const FULL_ROW_STRIDE: usize = 3;
/// Columns per BFS layer in the compact map.
pub const MINI_COL_STRIDE: usize = 4;
/// Rows per node row in the compact map.
pub const MINI_ROW_STRIDE: usize = 2;

const N: u8 = 1;
const E: u8 = 2;
const S: u8 = 4;
const W: u8 = 8;

/// Which map to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapScale {
    /// `[gate2]` labels, 14 columns per layer.
    Full,
    /// One glyph per node, 4 columns per layer.
    Mini,
}

impl MapScale {
    fn col_stride(self) -> usize {
        match self {
            MapScale::Full => FULL_COL_STRIDE,
            MapScale::Mini => MINI_COL_STRIDE,
        }
    }

    fn row_stride(self) -> usize {
        match self {
            MapScale::Full => FULL_ROW_STRIDE,
            MapScale::Mini => MINI_ROW_STRIDE,
        }
    }

    /// Edge cells between the bend column and the node it enters.
    fn bend_gap(self) -> usize {
        match self {
            MapScale::Full => 2,
            MapScale::Mini => 2,
        }
    }
}

/// How a node is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLook {
    /// Not visited, nothing guarding it.
    Plain,
    /// Its ICE is down (or it never had any) and the player passed through.
    Cleared,
    /// Guarded by uncleared ICE.
    Ice,
    /// Where the player is.
    Current,
}

/// What a grid cell holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Empty,
    Edge,
    Node(NodeLook),
}

/// One cell of a [`MapGrid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapCell {
    pub ch: char,
    pub kind: CellKind,
}

const EMPTY: MapCell = MapCell {
    ch: ' ',
    kind: CellKind::Empty,
};

/// The look of a node: current wins, then ICE, then cleared (only nodes the player has entered
/// count: the portal and anything it connected to).
pub fn node_look(node: &MapNode) -> NodeLook {
    if node.is_current {
        NodeLook::Current
    } else if node.ice.is_some() && !node.cleared {
        NodeLook::Ice
    } else if node.cleared {
        NodeLook::Cleared
    } else {
        NodeLook::Plain
    }
}

/// Box-drawing glyph for the directions (N, E, S, W bits) a cell connects.
fn edge_glyph(bits: u8) -> char {
    match bits {
        0 => ' ',
        b if b == N | S || b == N || b == S => '│',
        b if b == E | W || b == E || b == W => '─',
        b if b == E | S => '┌',
        b if b == S | W => '┐',
        b if b == N | E => '└',
        b if b == N | W => '┘',
        b if b == N | E | S => '├',
        b if b == N | S | W => '┤',
        b if b == E | S | W => '┬',
        b if b == N | E | W => '┴',
        _ => '┼',
    }
}

/// A laid-out map: glyph cells plus where each node sits.
#[derive(Debug, Clone)]
pub struct MapGrid {
    width: usize,
    height: usize,
    cells: Vec<MapCell>,
    /// `(node index, x, y, width)`.
    rects: Vec<(usize, usize, usize, usize)>,
}

impl MapGrid {
    /// Lay out `nodes` (as returned by `HostSession::map_layout`).
    pub fn build(nodes: &[MapNode], scale: MapScale) -> MapGrid {
        let labels: Vec<String> = nodes
            .iter()
            .map(|n| match scale {
                MapScale::Full => format!("[{}]", n.id),
                MapScale::Mini => mini_glyph(n).to_string(),
            })
            .collect();
        let rects: Vec<(usize, usize, usize, usize)> = nodes
            .iter()
            .zip(&labels)
            .map(|(n, l)| {
                (
                    n.index,
                    n.layer * scale.col_stride(),
                    n.row * scale.row_stride(),
                    l.chars().count(),
                )
            })
            .collect();
        let width = rects.iter().map(|&(_, x, _, w)| x + w).max().unwrap_or(0);
        let height = rects.iter().map(|&(_, _, y, _)| y + 1).max().unwrap_or(0);
        let mut bits = vec![0u8; width * height];

        for node in nodes {
            for &nb in &node.neighbors {
                if node.index >= nb {
                    continue;
                }
                let (Some(a), Some(b)) = (rect_of(&rects, node.index), rect_of(&rects, nb)) else {
                    continue;
                };
                route(&mut bits, width, a, b, scale);
            }
        }

        let mut cells: Vec<MapCell> = bits
            .iter()
            .map(|&b| {
                if b == 0 {
                    EMPTY
                } else {
                    MapCell {
                        ch: edge_glyph(b),
                        kind: CellKind::Edge,
                    }
                }
            })
            .collect();
        for ((node, label), &(_, x, y, _)) in nodes.iter().zip(&labels).zip(&rects) {
            let look = node_look(node);
            for (i, ch) in label.chars().enumerate() {
                cells[y * width + x + i] = MapCell {
                    ch,
                    kind: CellKind::Node(look),
                };
            }
        }
        MapGrid {
            width,
            height,
            cells,
            rects,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// The cell at `(x, y)`; empty outside the grid.
    pub fn cell(&self, x: usize, y: usize) -> MapCell {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x]
        } else {
            EMPTY
        }
    }

    /// Row `y` as text.
    pub fn row_text(&self, y: usize) -> String {
        (0..self.width).map(|x| self.cell(x, y).ch).collect()
    }

    /// `(x, y, width)` of the node with this index.
    pub fn node_rect(&self, index: usize) -> Option<(usize, usize, usize)> {
        rect_of(&self.rects, index)
    }

    /// The scroll offset `(x, y)` that shows node `index` inside a `view_w` x `view_h` window,
    /// leaving the grid's origin in place whenever everything fits.
    pub fn scroll_for(&self, index: usize, view_w: usize, view_h: usize) -> (usize, usize) {
        let Some((x, y, w)) = self.node_rect(index) else {
            return (0, 0);
        };
        let axis = |pos: usize, len: usize, total: usize, view: usize| {
            if total <= view || pos + len <= view {
                0
            } else {
                (pos + len).saturating_sub(view).min(total - view)
            }
        };
        (
            axis(x, w, self.width, view_w),
            axis(y, 1, self.height, view_h),
        )
    }

    /// Clamp a scroll offset so the window never leaves the grid.
    pub fn clamp_scroll(
        &self,
        scroll: (usize, usize),
        view_w: usize,
        view_h: usize,
    ) -> (usize, usize) {
        (
            scroll.0.min(self.width.saturating_sub(view_w)),
            scroll.1.min(self.height.saturating_sub(view_h)),
        )
    }

    /// Draw the window of the grid starting at `scroll` into a `view_w` x `view_h` area whose
    /// top-left is the absolute buffer position `(x0, y0)`. Anything outside the grid or the
    /// buffer is clipped. Cleared nodes use `chrome`, ICE nodes red, the current node is shown
    /// inverse, routes dark gray.
    pub fn render(
        &self,
        buf: &mut Buffer,
        origin: (u16, u16),
        view: (u16, u16),
        scroll: (usize, usize),
        palette: &Palette,
        chrome: Color,
    ) {
        let styles = [
            (CellKind::Edge, palette.style(Dos::DarkGray, Dos::Black)),
            (
                CellKind::Node(NodeLook::Plain),
                palette.style(Dos::LightGray, Dos::Black),
            ),
            (
                CellKind::Node(NodeLook::Cleared),
                Style::new().fg(chrome).bg(palette.color(Dos::Black)),
            ),
            (
                CellKind::Node(NodeLook::Ice),
                palette.style(Dos::LightRed, Dos::Black),
            ),
            (
                CellKind::Node(NodeLook::Current),
                palette.style(Dos::Black, Dos::LightGray),
            ),
        ];
        for vy in 0..usize::from(view.1) {
            for vx in 0..usize::from(view.0) {
                let cell = self.cell(scroll.0 + vx, scroll.1 + vy);
                if cell.kind == CellKind::Empty {
                    continue;
                }
                let style = styles
                    .iter()
                    .find(|(k, _)| *k == cell.kind)
                    .map_or_else(Style::new, |(_, s)| *s);
                // Window sizes are u16, so these casts back to u16 cannot overflow.
                let pos = (origin.0 + vx as u16, origin.1 + vy as u16);
                if let Some(c) = buf.cell_mut(pos) {
                    c.set_char(cell.ch);
                    c.set_style(style);
                }
            }
        }
    }
}

fn rect_of(rects: &[(usize, usize, usize, usize)], index: usize) -> Option<(usize, usize, usize)> {
    rects
        .iter()
        .find(|r| r.0 == index)
        .map(|&(_, x, y, w)| (x, y, w))
}

fn mini_glyph(n: &MapNode) -> char {
    match node_look(n) {
        NodeLook::Current => '☺',
        NodeLook::Ice => 'Θ',
        NodeLook::Cleared => '•',
        NodeLook::Plain => '○',
    }
}

fn mark(bits: &mut [u8], width: usize, x: usize, y: usize, b: u8) {
    if let Some(cell) = bits.get_mut(y * width + x) {
        *cell |= b;
    }
}

/// Draw the L-shaped route between two nodes: out of the left node's right edge along its row,
/// down or up the bend column just before the right node, then into it. Nodes in the same layer
/// are joined by a vertical run below the upper one.
fn route(
    bits: &mut [u8],
    width: usize,
    a: (usize, usize, usize),
    b: (usize, usize, usize),
    scale: MapScale,
) {
    let (a, b) = if (a.0, a.1) <= (b.0, b.1) {
        (a, b)
    } else {
        (b, a)
    };
    let ((xa, ya, wa), (xb, yb, _)) = (a, b);
    if xa == xb {
        let xc = xa + wa / 2;
        for y in ya + 1..yb {
            mark(bits, width, xc, y, N | S);
        }
        return;
    }
    let start = xa + wa;
    let bend = xb.saturating_sub(scale.bend_gap()).max(start);
    if ya == yb {
        for x in start..xb {
            mark(bits, width, x, ya, E | W);
        }
        return;
    }
    let down = yb > ya;
    for x in start..bend {
        mark(bits, width, x, ya, E | W);
    }
    mark(bits, width, bend, ya, W | if down { S } else { N });
    let (lo, hi) = if down { (ya + 1, yb) } else { (yb + 1, ya) };
    for y in lo..hi {
        mark(bits, width, bend, y, N | S);
    }
    mark(bits, width, bend, yb, E | if down { N } else { S });
    for x in bend + 1..xb {
        mark(bits, width, x, yb, E | W);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::host::{IceKindAt, NodeKind};
    use ratatui::layout::Rect;

    fn node(index: usize, id: &str, layer: usize, row: usize, neighbors: &[usize]) -> MapNode {
        MapNode {
            index,
            id: id.to_string(),
            display: id.to_uppercase(),
            kind: NodeKind::Gate,
            ice: None,
            cleared: false,
            is_current: false,
            layer,
            row,
            neighbors: neighbors.to_vec(),
        }
    }

    fn rows(grid: &MapGrid) -> Vec<String> {
        (0..grid.height()).map(|y| grid.row_text(y)).collect()
    }

    #[test]
    fn straight_edge_between_layers_joins_label_to_label() {
        let nodes = vec![node(0, "portal0", 0, 0, &[1]), node(1, "gate1", 1, 0, &[0])];
        let g = MapGrid::build(&nodes, MapScale::Full);
        assert_eq!(g.node_rect(1), Some((14, 0, 7)));
        assert_eq!(g.row_text(0), "[portal0]─────[gate1]");
    }

    #[test]
    fn l_shaped_route_bends_in_the_column_before_the_target() {
        let nodes = vec![
            node(0, "portal0", 0, 0, &[1, 2]),
            node(1, "gate1", 1, 0, &[0]),
            node(2, "gate2", 1, 1, &[0]),
        ];
        let g = MapGrid::build(&nodes, MapScale::Full);
        let r = rows(&g);
        assert_eq!(r.len(), 4);
        // Layer 1 starts at column 14: the bend column is 12, shared by both routes.
        assert_eq!(r[0].chars().nth(12), Some('┬'), "{r:?}");
        assert_eq!(r[1].chars().nth(12), Some('│'));
        assert_eq!(r[2].chars().nth(12), Some('│'));
        assert_eq!(r[3].chars().nth(12), Some('└'));
        assert_eq!(r[3].chars().nth(13), Some('─'));
        assert!(r[3].ends_with("[gate2]"));
        assert!(r[0].starts_with("[portal0]"));
    }

    #[test]
    fn route_going_up_uses_the_mirrored_corners() {
        let nodes = vec![node(0, "a0", 0, 1, &[1]), node(1, "b1", 1, 0, &[0])];
        let g = MapGrid::build(&nodes, MapScale::Full);
        let r = rows(&g);
        assert_eq!(r[3].chars().nth(12), Some('┘'), "{r:?}");
        assert_eq!(r[1].chars().nth(12), Some('│'));
        assert_eq!(r[0].chars().nth(12), Some('┌'));
    }

    #[test]
    fn overlapping_routes_merge_into_junctions() {
        // 1 and 2 (layer 1) both enter 3 (layer 2): their routes share the bend column.
        let nodes = vec![
            node(0, "p0", 0, 0, &[1, 2]),
            node(1, "g1", 1, 0, &[0, 3]),
            node(2, "g2", 1, 1, &[0, 3]),
            node(3, "g3", 2, 0, &[1, 2]),
        ];
        let g = MapGrid::build(&nodes, MapScale::Full);
        let r = rows(&g);
        let bend2 = 26; // 28 - 2: both g1 and g2 route into g3 here
        assert_eq!(r[0].chars().nth(bend2), Some('┬'), "{r:?}");
        assert_eq!(r[1].chars().nth(bend2), Some('│'));
        assert_eq!(r[3].chars().nth(bend2), Some('┘'));
    }

    #[test]
    fn same_layer_nodes_are_joined_vertically_under_the_upper_one() {
        let nodes = vec![
            node(0, "p0", 0, 0, &[]),
            node(1, "g1", 1, 0, &[2]),
            node(2, "g2", 1, 1, &[1]),
        ];
        let g = MapGrid::build(&nodes, MapScale::Full);
        let r = rows(&g);
        // "[g1]" is 4 wide at column 14: the run sits at 14 + 2.
        assert_eq!(r[1].chars().nth(16), Some('│'), "{r:?}");
        assert_eq!(r[3].chars().nth(15), Some('g'));
    }

    #[test]
    fn looks_follow_current_ice_and_cleared() {
        let mut n = node(0, "p0", 0, 0, &[]);
        n.ice = Some(IceKindAt::Breach);
        assert_eq!(node_look(&n), NodeLook::Ice);
        n.cleared = true;
        assert_eq!(node_look(&n), NodeLook::Cleared);
        n.is_current = true;
        assert_eq!(node_look(&n), NodeLook::Current);
        n.is_current = false;
        n.cleared = false;
        n.ice = None;
        assert_eq!(node_look(&n), NodeLook::Plain);
    }

    fn chain(len: usize) -> Vec<MapNode> {
        (0..len)
            .map(|i| {
                let mut nb = Vec::new();
                if i > 0 {
                    nb.push(i - 1);
                }
                if i + 1 < len {
                    nb.push(i + 1);
                }
                node(i, &format!("gate{i}"), i, 0, &nb)
            })
            .collect()
    }

    #[test]
    fn a_long_chain_is_wider_than_the_pane_and_scroll_keeps_the_current_node_visible() {
        let mut nodes = chain(8);
        nodes[7].is_current = true;
        let g = MapGrid::build(&nodes, MapScale::Full);
        assert_eq!(g.width(), 7 * 14 + 7);
        let (sx, sy) = g.scroll_for(7, 54, 20);
        let (x, _, w) = g.node_rect(7).unwrap();
        assert!(x >= sx && x + w <= sx + 54, "scroll {sx} hides node at {x}");
        assert_eq!(sy, 0);
        // An early node needs no scroll.
        assert_eq!(g.scroll_for(1, 54, 20), (0, 0));
    }

    #[test]
    fn render_clips_to_the_window_and_the_buffer() {
        let mut nodes = chain(8);
        nodes[0].is_current = true;
        let g = MapGrid::build(&nodes, MapScale::Full);
        let area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(area);
        let p = Palette::new(false);
        // Window larger than the buffer, offset near the right edge, huge scroll: no panic.
        g.render(&mut buf, (10, 1), (54, 22), (0, 0), &p, Color::Red);
        g.render(&mut buf, (0, 0), (20, 4), (9_999, 9_999), &p, Color::Red);
        // Window at scroll 0 shows the start of the first label.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
        g.render(&mut buf, (0, 0), (20, 2), (0, 0), &p, Color::Red);
        let text: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(text.starts_with("[gate0]"), "{text:?}");
        // The current node is inverse: black on light gray.
        assert_eq!(buf[(1, 0)].bg, p.color(Dos::LightGray));
        assert_eq!(buf[(1, 0)].fg, p.color(Dos::Black));
    }

    #[test]
    fn eight_node_worst_case_fits_the_scan_pane_height() {
        // Root with three children, each child with one more: layers of up to 3 nodes.
        let nodes = vec![
            node(0, "portal0", 0, 0, &[1, 2, 3]),
            node(1, "gate1", 1, 0, &[0, 4]),
            node(2, "gate2", 1, 1, &[0, 5]),
            node(3, "gate3", 1, 2, &[0, 6]),
            node(4, "gate4", 2, 0, &[1, 7]),
            node(5, "gate5", 2, 1, &[2]),
            node(6, "gate6", 2, 2, &[3]),
            node(7, "core7", 3, 0, &[4]),
        ];
        let g = MapGrid::build(&nodes, MapScale::Full);
        assert!(g.height() <= 22, "height {}", g.height());
        assert_eq!(g.height(), 7);
        assert_eq!(g.width(), 3 * 14 + 7);
        // Every label is drawn whole.
        for n in &nodes {
            let (x, y, w) = g.node_rect(n.index).unwrap();
            let got: String = (x..x + w).map(|i| g.cell(i, y).ch).collect();
            assert_eq!(got, format!("[{}]", n.id));
        }
    }

    #[test]
    fn mini_map_uses_glyphs_and_fits_the_side_panel() {
        let mut nodes = chain(8);
        nodes[2].is_current = true;
        nodes[3].ice = Some(IceKindAt::Black);
        nodes[0].cleared = true;
        nodes[1].cleared = true;
        let g = MapGrid::build(&nodes, MapScale::Mini);
        assert_eq!(g.height(), 1);
        assert_eq!(g.width(), 7 * 4 + 1);
        assert_eq!(g.row_text(0), "•───•───☺───Θ───○───○───○───○");
        // Wider than the 24-column side panel: the scroll keeps the far end in view.
        let (sx, _) = g.scroll_for(7, 24, 7);
        assert!(28 >= sx && 29 <= sx + 24);
    }

    #[test]
    fn empty_map_is_empty() {
        let g = MapGrid::build(&[], MapScale::Full);
        assert_eq!((g.width(), g.height()), (0, 0));
        assert_eq!(g.scroll_for(0, 10, 10), (0, 0));
        assert_eq!(g.cell(3, 3).kind, CellKind::Empty);
    }
}
