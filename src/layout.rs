use crate::protocol::{PaneResizeDirection, SplitDirection};
use crate::pty::PtySize;

const HORIZONTAL_SEPARATOR_CELLS: usize = 3;
const VERTICAL_SEPARATOR_CELLS: usize = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LayoutNode {
    Pane(usize),
    Split {
        direction: SplitDirection,
        first_weight: usize,
        second_weight: usize,
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
                    first_weight: 1,
                    second_weight: 1,
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

    pub(crate) fn resize_pane(
        &mut self,
        target: usize,
        direction: PaneResizeDirection,
        amount: usize,
        size: PtySize,
    ) -> Result<(), String> {
        if amount == 0 {
            return Err("resize amount must be a positive integer".to_string());
        }

        let mut resized = self.clone();
        resized.resize_pane_in_region(
            target,
            direction,
            amount,
            0,
            size.rows as usize,
            0,
            size.cols as usize,
        )?;
        if layout_regions_for_size(&resized, size)
            .into_iter()
            .any(|region| region.row_start >= region.row_end || region.col_start >= region.col_end)
        {
            return Err("resize would exceed minimum pane size".to_string());
        }

        *self = resized;
        Ok(())
    }

    fn contains_pane(&self, target: usize) -> bool {
        match self {
            LayoutNode::Pane(index) => *index == target,
            LayoutNode::Split { first, second, .. } => {
                first.contains_pane(target) || second.contains_pane(target)
            }
        }
    }

    fn resize_pane_in_region(
        &mut self,
        target: usize,
        resize_direction: PaneResizeDirection,
        amount: usize,
        row_start: usize,
        row_end: usize,
        col_start: usize,
        col_end: usize,
    ) -> Result<(), String> {
        match self {
            LayoutNode::Pane(index) => {
                if *index == target {
                    Err("missing adjacent pane".to_string())
                } else {
                    Err("missing pane".to_string())
                }
            }
            LayoutNode::Split {
                direction,
                first_weight,
                second_weight,
                first,
                second,
            } => {
                let separator = separator_for_direction(*direction);
                let ((first_start, first_end), (second_start, second_end)) = match direction {
                    SplitDirection::Horizontal => split_extent_weighted(
                        col_start,
                        col_end,
                        separator,
                        *first_weight,
                        *second_weight,
                    ),
                    SplitDirection::Vertical => split_extent_weighted(
                        row_start,
                        row_end,
                        separator,
                        *first_weight,
                        *second_weight,
                    ),
                };

                if first.contains_pane(target) {
                    let first_result = match direction {
                        SplitDirection::Horizontal => first.resize_pane_in_region(
                            target,
                            resize_direction,
                            amount,
                            row_start,
                            row_end,
                            first_start,
                            first_end,
                        ),
                        SplitDirection::Vertical => first.resize_pane_in_region(
                            target,
                            resize_direction,
                            amount,
                            first_start,
                            first_end,
                            col_start,
                            col_end,
                        ),
                    };
                    return match first_result {
                        Ok(()) => Ok(()),
                        Err(message) if message != "missing adjacent pane" => Err(message),
                        Err(_) if split_can_resize_from_first(*direction, resize_direction) => {
                            resize_split_weights(
                                first_weight,
                                second_weight,
                                first_end.saturating_sub(first_start),
                                second_end.saturating_sub(second_start),
                                amount,
                                true,
                            )
                        }
                        Err(message) => Err(message),
                    };
                }

                if second.contains_pane(target) {
                    let second_result = match direction {
                        SplitDirection::Horizontal => second.resize_pane_in_region(
                            target,
                            resize_direction,
                            amount,
                            row_start,
                            row_end,
                            second_start,
                            second_end,
                        ),
                        SplitDirection::Vertical => second.resize_pane_in_region(
                            target,
                            resize_direction,
                            amount,
                            second_start,
                            second_end,
                            col_start,
                            col_end,
                        ),
                    };
                    return match second_result {
                        Ok(()) => Ok(()),
                        Err(message) if message != "missing adjacent pane" => Err(message),
                        Err(_) if split_can_resize_from_second(*direction, resize_direction) => {
                            resize_split_weights(
                                first_weight,
                                second_weight,
                                first_end.saturating_sub(first_start),
                                second_end.saturating_sub(second_start),
                                amount,
                                false,
                            )
                        }
                        Err(message) => Err(message),
                    };
                }

                Err("missing pane".to_string())
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
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) = split_extent_weighted(
                col_start,
                col_end,
                HORIZONTAL_SEPARATOR_CELLS,
                *first_weight,
                *second_weight,
            );
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
            first_weight,
            second_weight,
            first,
            second,
        } => {
            let ((first_start, first_end), (second_start, second_end)) = split_extent_weighted(
                row_start,
                row_end,
                VERTICAL_SEPARATOR_CELLS,
                *first_weight,
                *second_weight,
            );
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

#[allow(dead_code)]
pub(crate) fn split_extent(
    start: usize,
    end: usize,
    separator: usize,
) -> ((usize, usize), (usize, usize)) {
    split_extent_weighted(start, end, separator, 1, 1)
}

pub(crate) fn split_extent_weighted(
    start: usize,
    end: usize,
    separator: usize,
    first_weight: usize,
    second_weight: usize,
) -> ((usize, usize), (usize, usize)) {
    let total = end.saturating_sub(start);
    if total <= 1 {
        return ((start, start), (start, end));
    }

    let gap = if total >= separator + 2 { separator } else { 0 };
    let content = total - gap;
    let weight_total = first_weight.saturating_add(second_weight).max(1);
    let mut first = content.saturating_mul(first_weight) / weight_total;
    if content >= 2 {
        first = first.clamp(1, content - 1);
    }
    let second = content - first;

    ((start, start + first), (end - second, end))
}

fn separator_for_direction(direction: SplitDirection) -> usize {
    match direction {
        SplitDirection::Horizontal => HORIZONTAL_SEPARATOR_CELLS,
        SplitDirection::Vertical => VERTICAL_SEPARATOR_CELLS,
    }
}

fn split_can_resize_from_first(
    split_direction: SplitDirection,
    resize_direction: PaneResizeDirection,
) -> bool {
    matches!(
        (split_direction, resize_direction),
        (SplitDirection::Horizontal, PaneResizeDirection::Right)
            | (SplitDirection::Vertical, PaneResizeDirection::Down)
    )
}

fn split_can_resize_from_second(
    split_direction: SplitDirection,
    resize_direction: PaneResizeDirection,
) -> bool {
    matches!(
        (split_direction, resize_direction),
        (SplitDirection::Horizontal, PaneResizeDirection::Left)
            | (SplitDirection::Vertical, PaneResizeDirection::Up)
    )
}

fn resize_split_weights(
    first_weight: &mut usize,
    second_weight: &mut usize,
    first_size: usize,
    second_size: usize,
    amount: usize,
    grow_first: bool,
) -> Result<(), String> {
    let (new_first, new_second) = if grow_first {
        let Some(new_second) = second_size.checked_sub(amount) else {
            return Err("resize would exceed minimum pane size".to_string());
        };
        (first_size + amount, new_second)
    } else {
        let Some(new_first) = first_size.checked_sub(amount) else {
            return Err("resize would exceed minimum pane size".to_string());
        };
        (new_first, second_size + amount)
    };

    if new_first < 1 || new_second < 1 {
        return Err("resize would exceed minimum pane size".to_string());
    }

    *first_weight = new_first;
    *second_weight = new_second;
    Ok(())
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
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn remove_pane_collapses_split_and_reindexes_higher_panes() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };

        assert!(layout.remove_pane(1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn layout_regions_for_size_splits_horizontal_panes_around_separator() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
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

    #[test]
    fn weighted_split_regions_preserve_custom_ratio() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 30,
            second_weight: 50,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        let regions = layout_regions_for_size(
            &layout,
            PtySize {
                cols: 163,
                rows: 24,
            },
        );

        assert_eq!(regions[0].col_end - regions[0].col_start, 60);
        assert_eq!(regions[1].col_end - regions[1].col_start, 100);
    }

    #[test]
    fn resize_active_right_pane_left_adjusts_nearest_horizontal_split() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        layout
            .resize_pane(
                1,
                PaneResizeDirection::Left,
                5,
                PtySize { cols: 83, rows: 24 },
            )
            .unwrap();

        assert_eq!(
            layout_regions_for_size(&layout, PtySize { cols: 83, rows: 24 }),
            vec![
                PaneRegion {
                    pane: 0,
                    row_start: 0,
                    row_end: 24,
                    col_start: 0,
                    col_end: 35,
                },
                PaneRegion {
                    pane: 1,
                    row_start: 0,
                    row_end: 24,
                    col_start: 38,
                    col_end: 83,
                },
            ]
        );
    }

    #[test]
    fn resize_without_adjacent_split_returns_error() {
        let mut layout = LayoutNode::Pane(0);

        let err = layout
            .resize_pane(
                0,
                PaneResizeDirection::Left,
                1,
                PtySize { cols: 80, rows: 24 },
            )
            .unwrap_err();

        assert_eq!(err, "missing adjacent pane");
    }

    #[test]
    fn resize_outer_edge_of_split_returns_missing_adjacent_pane() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        let err = layout
            .resize_pane(
                0,
                PaneResizeDirection::Left,
                1,
                PtySize { cols: 80, rows: 24 },
            )
            .unwrap_err();

        assert_eq!(err, "missing adjacent pane");
    }

    #[test]
    fn resize_rejects_shrinking_either_side_below_one_cell() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        };

        let err = layout
            .resize_pane(
                1,
                PaneResizeDirection::Left,
                40,
                PtySize { cols: 83, rows: 24 },
            )
            .unwrap_err();

        assert!(err.contains("minimum pane size"), "{err}");
    }

    #[test]
    fn resize_rejects_nested_zero_sized_descendant() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first_weight: 1,
            second_weight: 1,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first_weight: 1,
                second_weight: 1,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };
        let size = PtySize { cols: 8, rows: 5 };
        let before = layout_regions_for_size(&layout, size);

        let err = layout
            .resize_pane(0, PaneResizeDirection::Right, 3, size)
            .unwrap_err();

        assert!(err.contains("minimum pane size"), "{err}");
        assert_eq!(layout_regions_for_size(&layout, size), before);
    }
}
