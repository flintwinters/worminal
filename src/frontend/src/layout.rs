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
    /// `id` joins the pane showing `anchor` on the `zone` side, or takes its place for `Center`.
    pub fn insert(self, anchor: u32, id: u32, zone: Zone) -> Self {
        match self {
            Self::Leaf(leaf) if leaf == anchor => {
                let (new, old) = (Box::new(Self::Leaf(id)), Box::new(Self::Leaf(leaf)));
                match zone {
                    Zone::Center => Self::Leaf(id),
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
    /// A layout showing one tab is no split.
    pub fn split_only(self) -> Option<Self> {
        matches!(self, Self::Split { .. }).then_some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout::*, Zone};

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
        assert_eq!(
            nested.clone().insert(2, 4, Zone::Center).leaves(),
            [4, 1, 3]
        );
        assert_eq!(nested.insert(9, 5, Zone::Left).leaves(), [2, 1, 3]);
    }

    #[test]
    fn removing_a_pane_collapses_its_parent_into_the_sibling() {
        let layout = Leaf(1).insert(1, 2, Zone::Right).insert(2, 3, Zone::Bottom);
        assert_eq!(layout.clone().without(3).unwrap().leaves(), [1, 2]);
        assert_eq!(layout.clone().without(1).unwrap().leaves(), [2, 3]);
        let alone = layout.without(1).unwrap().without(3).unwrap();
        assert_eq!(alone, Leaf(2));
        assert_eq!(alone.clone().split_only(), None);
        assert_eq!(alone.without(2), None);
    }
}
