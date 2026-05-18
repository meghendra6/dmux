use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupMode {
    Attention,
    Workspace,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupGrouping {
    Attention,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PopupStateKind {
    NeedsInput,
    Alert,
    Ready,
    Failed,
    Working,
    Completed,
    Idle,
    Detached,
    Previous,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupRowKind {
    Header,
    Item,
    DisabledItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupRowSource {
    Mux,
    Registry,
    AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupTarget {
    pub session: String,
    pub window_index: Option<usize>,
    pub pane_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupRow {
    pub id: String,
    pub kind: PopupRowKind,
    pub repo_path: Option<PathBuf>,
    pub target: Option<PopupTarget>,
    pub state: PopupStateKind,
    pub source: PopupRowSource,
    pub title: String,
    pub summary: String,
    pub last_changed: Option<u64>,
    pub attachable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupModel {
    pub rows: Vec<PopupRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupState {
    pub mode: PopupMode,
    pub grouping: PopupGrouping,
    pub selected: Option<String>,
    pub scroll: usize,
    pub filter: String,
    pub filter_mode: bool,
    pub peek: bool,
}

impl PopupState {
    pub fn new(mode: PopupMode) -> Self {
        Self {
            mode,
            grouping: PopupGrouping::Attention,
            selected: None,
            scroll: 0,
            filter: String::new(),
            filter_mode: false,
            peek: false,
        }
    }

    pub fn toggle_grouping(&mut self) {
        self.grouping = match self.grouping {
            PopupGrouping::Attention => PopupGrouping::Repo,
            PopupGrouping::Repo => PopupGrouping::Attention,
        };
    }

    pub fn close_or_clear(&mut self) -> PopupCloseResult {
        if self.peek {
            self.peek = false;
            PopupCloseResult::StayOpen
        } else if self.filter_mode {
            self.filter_mode = false;
            self.filter.clear();
            PopupCloseResult::StayOpen
        } else if !self.filter.is_empty() {
            self.filter.clear();
            PopupCloseResult::StayOpen
        } else {
            PopupCloseResult::Close
        }
    }

    pub fn ensure_selection(&mut self, model: &PopupModel) {
        if let Some(selected) = &self.selected {
            if model
                .rows
                .iter()
                .any(|row| row.id == *selected && row.kind != PopupRowKind::Header)
            {
                return;
            }
        }
        self.selected = model
            .rows
            .iter()
            .find(|row| row.kind != PopupRowKind::Header)
            .map(|row| row.id.clone());
    }

    pub fn move_selection(&mut self, model: &PopupModel, delta: isize) {
        let selectable = model.selectable_row_ids();
        if selectable.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| selectable.iter().position(|candidate| candidate == id))
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(selectable.len().saturating_sub(1))
        };
        self.selected = Some(selectable[next].clone());
    }

    pub fn selected_row<'a>(&self, model: &'a PopupModel) -> Option<&'a PopupRow> {
        let selected = self.selected.as_ref()?;
        model.rows.iter().find(|row| row.id == *selected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupCloseResult {
    StayOpen,
    Close,
}

impl PopupModel {
    pub fn new(rows: Vec<PopupRow>) -> Self {
        Self { rows }
    }

    pub fn selectable_row_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.kind != PopupRowKind::Header)
            .map(|row| row.id.clone())
            .collect()
    }
}

pub fn filter_rows(rows: &[PopupRow], filter: &str) -> Vec<PopupRow> {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return rows.to_vec();
    }

    rows.iter()
        .filter(|row| {
            if row.kind == PopupRowKind::Header {
                return true;
            }
            let repo = row
                .repo_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            let target = row
                .target
                .as_ref()
                .map(|target| {
                    format!(
                        "{} {} {}",
                        target.session,
                        target
                            .window_index
                            .map(|index| index.to_string())
                            .unwrap_or_default(),
                        target
                            .pane_index
                            .map(|index| index.to_string())
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            format!(
                "{} {} {} {} {:?}",
                repo, target, row.title, row.summary, row.state
            )
            .to_ascii_lowercase()
            .contains(&needle)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, title: &str, kind: PopupRowKind) -> PopupRow {
        PopupRow {
            id: id.to_string(),
            kind,
            repo_path: None,
            target: Some(PopupTarget {
                session: "dev".to_string(),
                window_index: Some(0),
                pane_index: Some(0),
            }),
            state: PopupStateKind::Working,
            source: PopupRowSource::Mux,
            title: title.to_string(),
            summary: String::new(),
            last_changed: None,
            attachable: kind == PopupRowKind::Item,
        }
    }

    #[test]
    fn selection_skips_headers() {
        let model = PopupModel::new(vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha", PopupRowKind::Item),
            row("b", "beta", PopupRowKind::Item),
        ]);
        let mut state = PopupState::new(PopupMode::Tree);

        state.ensure_selection(&model);
        assert_eq!(state.selected.as_deref(), Some("a"));

        state.move_selection(&model, 1);
        assert_eq!(state.selected.as_deref(), Some("b"));

        state.move_selection(&model, -1);
        assert_eq!(state.selected.as_deref(), Some("a"));
    }

    #[test]
    fn filtering_keeps_headers_and_matching_items() {
        let rows = vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha api", PopupRowKind::Item),
            row("b", "beta ui", PopupRowKind::Item),
        ];

        let filtered = filter_rows(&rows, "api");
        assert_eq!(
            filtered
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["h", "a"]
        );
    }

    #[test]
    fn escape_closes_peek_before_popup() {
        let mut state = PopupState::new(PopupMode::Attention);
        state.peek = true;

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.peek);
        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }
}
