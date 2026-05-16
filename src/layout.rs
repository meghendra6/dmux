use crate::protocol::SplitDirection;

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
}
