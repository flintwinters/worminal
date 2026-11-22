//! The window's split layout as a binary tree of tab ids, independent of GTK. A window with no split has no layout.

/// Where a dragged tab lands relative to the pane under the pointer: beside it, or in its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}
/// A keyboard direction across the pane tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
impl Direction {
    /// The edge of a pane that a tab moved this way would split off when no pane lies that way.
    pub fn zone(self) -> Zone {
        match self {
            Self::Left => Zone::Left,
            Self::Right => Zone::Right,
            Self::Up => Zone::Top,
            Self::Down => Zone::Bottom,
        }
    }
}
/// A pane's on-screen rectangle: `(x, y, width, height)` in one shared coordinate space.
pub type PaneRect = (i32, i32, i32, i32);
/// The pane to reach from `from` toward `direction`, among panes laid out as `panes` (id and rectangle). It lies
/// wholly beyond the near edge of `from`; panes sharing a stretch of that edge win over ones that only lie that
/// way, then the closest, then the longest shared stretch. Geometry, not the tree, decides: entering a split
/// subtree must land beside the pane being left, not on its first leaf, and dragged dividers count.
pub fn neighbor(panes: &[(u32, PaneRect)], from: u32, direction: Direction) -> Option<u32> {
    let (_, (x, y, width, height)) = *panes.iter().find(|(id, _)| *id == from)?;
    panes
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|&(id, (px, py, pw, ph))| {
            // Distance beyond the near edge, then the overlap of the two spans along that edge.
            let (gap, from_span, span) = match direction {
                Direction::Right => (px - (x + width), (y, y + height), (py, py + ph)),
                Direction::Left => (x - (px + pw), (y, y + height), (py, py + ph)),
                Direction::Down => (py - (y + height), (x, x + width), (px, px + pw)),
                Direction::Up => (y - (py + ph), (x, x + width), (px, px + pw)),
            };
            // Panes touching or overlapping the edge by a pixel or two still count as beyond it.
            (gap >= -2).then(|| {
                let shared = from_span.1.min(span.1) - from_span.0.max(span.0);
                ((shared <= 0, gap.max(0), -shared), id)
            })
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, id)| id)
}
/// Share of a pane's width or height that each edge zone covers.
const EDGE: f64 = 0.25;
impl Zone {
    /// The zone of a point inside a `width` x `height` pane; the nearest edge wins inside the corner overlaps.
    pub fn at(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
            return None;
        }
        let (fx, fy) = (x / width, y / height);
        // Distance into each edge band, as a share of that band: 1 at the border, 0 where it ends.
        let reach = |f: f64| ((EDGE - f.min(1.0 - f)) / EDGE).max(0.0);
        let (horizontal, vertical) = (reach(fx), reach(fy));
        Some(if horizontal == 0.0 && vertical == 0.0 {
            Self::Center
        } else if horizontal >= vertical {
            if fx < 0.5 {
                Self::Left
            } else {
                Self::Right
            }
        } else if fy < 0.5 {
            Self::Top
        } else {
            Self::Bottom
        })
    }
    /// The part of a `width` x `height` pane this zone claims: `(x, y, width, height)`.
    pub fn rect(self, width: f64, height: f64) -> (f64, f64, f64, f64) {
        let (half_width, half_height) = (width / 2.0, height / 2.0);
        match self {
            Self::Left => (0.0, 0.0, half_width, height),
            Self::Right => (half_width, 0.0, half_width, height),
            Self::Top => (0.0, 0.0, width, half_height),
            Self::Bottom => (0.0, half_height, width, half_height),
            Self::Center => (0.0, 0.0, width, height),
        }
    }
}

/// A drop onto one visible pane: the pane's tab and the region of it the drop claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub anchor: u32,
    pub zone: Zone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Leaf(u32),
    Split {
        /// Children sit left and right; otherwise top and bottom.
        side_by_side: bool,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}
impl Layout {
    /// Tab ids in reading order.
    pub fn leaves(&self) -> Vec<u32> {
        match self {
            Self::Leaf(id) => vec![*id],
            Self::Split { first, second, .. } => {
                let mut leaves = first.leaves();
                leaves.extend(second.leaves());
                leaves
            }
        }
    }
    pub fn contains(&self, id: u32) -> bool {
        self.leaves().contains(&id)
    }
    /// The layout without `id`; its sibling takes the freed space. `None` when nothing remains.
    pub fn without(self, id: u32) -> Option<Self> {
        match self {
            Self::Leaf(leaf) => (leaf != id).then_some(self),
            Self::Split {
                side_by_side,
                first,
                second,
            } => match (first.without(id), second.without(id)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    side_by_side,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (only, None) | (None, only) => only,
            },
        }
    }
    /// `id` joins the pane showing `anchor` on the `zone` side; `Center` is no split and changes nothing.
    pub fn insert(self, anchor: u32, id: u32, zone: Zone) -> Self {
        match self {
            Self::Leaf(leaf) if leaf == anchor => {
                let (new, old) = (Box::new(Self::Leaf(id)), Box::new(Self::Leaf(leaf)));
                match zone {
                    Zone::Center => Self::Leaf(leaf),
                    Zone::Left => Self::split(true, new, old),
                    Zone::Right => Self::split(true, old, new),
                    Zone::Top => Self::split(false, new, old),
                    Zone::Bottom => Self::split(false, old, new),
                }
            }
            Self::Leaf(_) => self,
            Self::Split {
                side_by_side,
                first,
                second,
            } => Self::split(
                side_by_side,
                Box::new(first.insert(anchor, id, zone)),
                Box::new(second.insert(anchor, id, zone)),
            ),
        }
    }
    fn split(side_by_side: bool, first: Box<Self>, second: Box<Self>) -> Self {
        Self::Split {
            side_by_side,
            first,
            second,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{neighbor, Direction::*, Layout::*, PaneRect, Zone};

    #[test]
    fn zones_split_a_pane_into_edges_and_a_center() {
        let at = |x, y| Zone::at(x, y, 100.0, 100.0);
        assert_eq!(at(5.0, 50.0), Some(Zone::Left));
        assert_eq!(at(95.0, 50.0), Some(Zone::Right));
        assert_eq!(at(50.0, 5.0), Some(Zone::Top));
        assert_eq!(at(50.0, 95.0), Some(Zone::Bottom));
        assert_eq!(at(50.0, 50.0), Some(Zone::Center));
        // The nearer edge wins in a corner.
        assert_eq!(at(10.0, 2.0), Some(Zone::Top));
        assert_eq!(at(2.0, 10.0), Some(Zone::Left));
        assert_eq!(at(-1.0, 50.0), None);
        assert_eq!(at(50.0, 100.0), None);
    }

    #[test]
    fn insert_splits_the_anchor_pane_on_the_dropped_side() {
        let left = Leaf(1).insert(1, 2, Zone::Left);
        assert_eq!(left.leaves(), [2, 1]);
        let nested = left.insert(1, 3, Zone::Bottom);
        assert_eq!(nested.leaves(), [2, 1, 3]);
        assert_eq!(nested.clone().insert(2, 4, Zone::Center), nested);
        assert_eq!(nested.insert(9, 5, Zone::Left).leaves(), [2, 1, 3]);
    }

    #[test]
    fn removing_a_pane_collapses_its_parent_into_the_sibling() {
        let layout = Leaf(1).insert(1, 2, Zone::Right).insert(2, 3, Zone::Bottom);
        assert_eq!(layout.clone().without(3).unwrap().leaves(), [1, 2]);
        assert_eq!(layout.clone().without(1).unwrap().leaves(), [2, 3]);
        let alone = layout.without(1).unwrap().without(3).unwrap();
        assert_eq!(alone, Leaf(2));
        assert_eq!(alone.without(2), None);
    }

    /// Two columns, each split top and bottom: ids 1/2 on the left (top/bottom), 3/4 on the right.
    fn grid() -> Vec<(u32, PaneRect)> {
        vec![
            (1, (0, 0, 100, 50)),
            (2, (0, 50, 100, 50)),
            (3, (100, 0, 100, 50)),
            (4, (100, 50, 100, 50)),
        ]
    }

    #[test]
    fn neighbor_stays_on_the_row_or_column_being_left() {
        let grid = grid();
        // Leaving the bottom-left tile to the right lands on the bottom-right one, not the first leaf.
        assert_eq!(neighbor(&grid, 2, Right), Some(4));
        assert_eq!(neighbor(&grid, 1, Right), Some(3));
        assert_eq!(neighbor(&grid, 4, Left), Some(2));
        assert_eq!(neighbor(&grid, 3, Left), Some(1));
        assert_eq!(neighbor(&grid, 1, Down), Some(2));
        assert_eq!(neighbor(&grid, 4, Up), Some(3));
        assert_eq!(neighbor(&grid, 1, Left), None);
        assert_eq!(neighbor(&grid, 1, Up), None);
        assert_eq!(neighbor(&grid, 4, Right), None);
        assert_eq!(neighbor(&grid, 4, Down), None);
        assert_eq!(neighbor(&grid, 9, Right), None);
    }

    #[test]
    fn neighbor_follows_dragged_dividers_and_uneven_splits() {
        // One tall pane left of a column whose divider sits low: the tall pane reaches the larger share.
        let uneven = vec![
            (1, (0, 0, 100, 100)),
            (2, (100, 0, 100, 20)),
            (3, (100, 20, 100, 80)),
        ];
        assert_eq!(neighbor(&uneven, 1, Right), Some(3));
        assert_eq!(neighbor(&uneven, 2, Left), Some(1));
        assert_eq!(neighbor(&uneven, 3, Left), Some(1));
        // With no pane sharing the edge, the nearest one in that direction still answers.
        let stair = vec![(1, (0, 0, 100, 50)), (2, (100, 60, 100, 40))];
        assert_eq!(neighbor(&stair, 1, Right), Some(2));
    }
}
