#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PaneId(usize);

impl PaneId {
    pub(crate) fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TabId(usize);

impl TabId {
    pub(crate) fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) fn as_usize(self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_id_exposes_numeric_value() {
        assert_eq!(PaneId::new(7).as_usize(), 7);
    }

    #[test]
    fn tab_id_exposes_numeric_value() {
        assert_eq!(TabId::new(3).as_usize(), 3);
    }
}
