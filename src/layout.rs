use crate::protocol::SplitDirection;
use crate::pty::PtySize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LayoutNode {
    Pane(usize),
    Split {
        direction: SplitDirection,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub(crate) fn split_pane(
        &mut self,
        target: usize,
        direction: SplitDirection,
        new_index: usize,
    ) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == target => {
                *self = LayoutNode::Split {
                    direction,
                    first: Box::new(LayoutNode::Pane(target)),
                    second: Box::new(LayoutNode::Pane(new_index)),
                };
                true
            }
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                first.split_pane(target, direction, new_index)
                    || second.split_pane(target, direction, new_index)
            }
        }
    }

    pub(crate) fn remove_pane(&mut self, removed: usize) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == removed => false,
            LayoutNode::Pane(index) => {
                if *index > removed {
                    *index -= 1;
                }
                true
            }
            LayoutNode::Split { first, second, .. } => {
                let keep_first = first.remove_pane(removed);
                let keep_second = second.remove_pane(removed);
                match (keep_first, keep_second) {
                    (true, true) => true,
                    (true, false) => {
                        *self = (**first).clone();
                        true
                    }
                    (false, true) => {
                        *self = (**second).clone();
                        true
                    }
                    (false, false) => false,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneRegion {
    pub(crate) pane: usize,
    pub(crate) row_start: usize,
    pub(crate) row_end: usize,
    pub(crate) col_start: usize,
    pub(crate) col_end: usize,
}

pub(crate) fn layout_regions_for_size(layout: &LayoutNode, size: PtySize) -> Vec<PaneRegion> {
    let mut regions = Vec::new();
    collect_sized_layout_regions(
        layout,
        0,
        size.rows as usize,
        0,
        size.cols as usize,
        &mut regions,
    );
    regions
}

fn collect_sized_layout_regions(
    layout: &LayoutNode,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
    regions: &mut Vec<PaneRegion>,
) {
    match layout {
        LayoutNode::Pane(index) => regions.push(PaneRegion {
            pane: *index,
            row_start,
            row_end,
            col_start,
            col_end,
        }),
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent(col_start, col_end, 3);
            collect_sized_layout_regions(
                first,
                row_start,
                row_end,
                first_start,
                first_end,
                regions,
            );
            collect_sized_layout_regions(
                second,
                row_start,
                row_end,
                second_start,
                second_end,
                regions,
            );
        }
        LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) =
                split_extent(row_start, row_end, 1);
            collect_sized_layout_regions(
                first,
                first_start,
                first_end,
                col_start,
                col_end,
                regions,
            );
            collect_sized_layout_regions(
                second,
                second_start,
                second_end,
                col_start,
                col_end,
                regions,
            );
        }
    }
}

pub(crate) fn split_extent(
    start: usize,
    end: usize,
    separator: usize,
) -> ((usize, usize), (usize, usize)) {
    let total = end.saturating_sub(start);
    if total <= 1 {
        return ((start, start), (start, end));
    }

    let gap = if total >= separator + 2 { separator } else { 0 };
    let content = total - gap;
    let first = content / 2;
    let second = content - first;

    ((start, start + first), (end - second, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pane_replaces_target_leaf_with_split() {
        let mut layout = LayoutNode::Pane(0);

        assert!(layout.split_pane(0, SplitDirection::Horizontal, 1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn remove_pane_collapses_split_and_reindexes_higher_panes() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };

        assert!(layout.remove_pane(1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn layout_regions_for_size_splits_horizontal_panes_around_separator() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 83, rows: 24 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 24,
                    col_start: 0,
                    col_end: 40,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 24,
                    col_start: 43,
                    col_end: 83,
                },
            ]
        );
    }
}
