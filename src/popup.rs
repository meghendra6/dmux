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
    pub window_id: Option<usize>,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
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
    pub reply_mode: bool,
    pub reply_text: String,
    pub reply_target: Option<PopupTarget>,
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
            reply_mode: false,
            reply_text: String::new(),
            reply_target: None,
        }
    }

    pub fn toggle_grouping(&mut self) {
        self.grouping = match self.grouping {
            PopupGrouping::Attention => PopupGrouping::Repo,
            PopupGrouping::Repo => PopupGrouping::Attention,
        };
    }

    pub fn close_or_clear(&mut self) -> PopupCloseResult {
        if self.reply_mode {
            self.reply_mode = false;
            self.reply_text.clear();
            self.reply_target = None;
            PopupCloseResult::StayOpen
        } else if self.peek {
            self.peek = false;
            self.reply_target = None;
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

pub fn group_rows(rows: Vec<PopupRow>, grouping: PopupGrouping) -> PopupModel {
    match grouping {
        PopupGrouping::Attention => group_rows_by_attention(rows),
        PopupGrouping::Repo => group_rows_by_repo(rows),
    }
}

fn group_rows_by_attention(rows: Vec<PopupRow>) -> PopupModel {
    let mut grouped = Vec::new();
    for (state, title) in attention_groups() {
        let group_rows = rows
            .iter()
            .filter(|row| row.kind != PopupRowKind::Header && row.state == state)
            .cloned()
            .collect::<Vec<_>>();
        if group_rows.is_empty() {
            continue;
        }
        grouped.push(header_row(
            format!("group:attention:{state:?}"),
            title.to_string(),
            state,
            None,
        ));
        grouped.extend(group_rows);
    }
    PopupModel::new(grouped)
}

fn attention_groups() -> [(PopupStateKind, &'static str); 10] {
    [
        (PopupStateKind::NeedsInput, "Needs input"),
        (PopupStateKind::Alert, "Alerts"),
        (PopupStateKind::Ready, "Ready"),
        (PopupStateKind::Failed, "Failed"),
        (PopupStateKind::Working, "Working"),
        (PopupStateKind::Completed, "Completed"),
        (PopupStateKind::Idle, "Idle"),
        (PopupStateKind::Detached, "Detached"),
        (PopupStateKind::Previous, "Previous"),
        (PopupStateKind::Stale, "Stale"),
    ]
}

fn group_rows_by_repo(rows: Vec<PopupRow>) -> PopupModel {
    let mut grouped = Vec::new();
    let mut groups: Vec<(Option<PathBuf>, String, Vec<PopupRow>)> = Vec::new();
    for row in rows
        .into_iter()
        .filter(|row| row.kind != PopupRowKind::Header)
    {
        let title = row
            .repo_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Unregistered".to_string());
        if let Some((_, _, group_rows)) = groups
            .iter_mut()
            .find(|(repo_path, _, _)| *repo_path == row.repo_path)
        {
            group_rows.push(row);
        } else {
            groups.push((row.repo_path.clone(), title, vec![row]));
        }
    }

    for (repo_path, title, group_rows) in groups {
        grouped.push(header_row(
            format!("group:repo:{title}"),
            title,
            PopupStateKind::Idle,
            repo_path,
        ));
        grouped.extend(group_rows);
    }
    PopupModel::new(grouped)
}

fn header_row(
    id: String,
    title: String,
    state: PopupStateKind,
    repo_path: Option<PathBuf>,
) -> PopupRow {
    PopupRow {
        id,
        kind: PopupRowKind::Header,
        repo_path,
        target: None,
        state,
        source: PopupRowSource::Mux,
        title,
        summary: String::new(),
        last_changed: None,
        attachable: false,
    }
}

pub fn render_popup_text(state: &PopupState, model: &PopupModel, peek: Option<&str>) -> String {
    let mut lines = Vec::new();
    for row in &model.rows {
        match row.kind {
            PopupRowKind::Header => lines.push(row.title.clone()),
            PopupRowKind::Item | PopupRowKind::DisabledItem => {
                let selected = state.selected.as_deref() == Some(row.id.as_str());
                let marker = if selected { ">" } else { " " };
                let state_marker = state_marker(row.state);
                let disabled = if row.kind == PopupRowKind::DisabledItem {
                    " disabled"
                } else {
                    ""
                };
                lines.push(format!(
                    "{} {} {:<10} {}{}",
                    marker, state_marker, row.title, row.summary, disabled
                ));
            }
        }
    }
    if let Some(peek) = peek {
        lines.push(String::new());
        lines.push("Peek".to_string());
        lines.extend(peek.lines().map(str::to_string));
    }
    if state.reply_mode {
        lines.push(String::new());
        lines.push(format!("Reply: {}", state.reply_text));
    }
    lines.push(String::new());
    lines.push(match state.mode {
        PopupMode::Attention => {
            "Space: peek   r: reply   Tab: group   /: filter   Esc: close".to_string()
        }
        PopupMode::Workspace => {
            "Enter: focus/attach   o: open   Space: peek   r: reply   Tab: group   /: filter   Esc: close"
                .to_string()
        }
        PopupMode::Tree => {
            "Enter: focus/attach   Space: peek   r: reply   Tab: group   /: filter   Esc: close"
                .to_string()
        }
    });
    lines.join("\n")
}

pub fn render_peek_text(
    metadata: &str,
    capture: &str,
    max_rows: usize,
    max_bytes: usize,
) -> String {
    let bytes = capture.as_bytes();
    let start = bytes.len().saturating_sub(max_bytes);
    let capture_tail = String::from_utf8_lossy(&bytes[start..]);
    let capture_rows = max_rows.saturating_sub(1);
    let capture_lines = capture_tail
        .lines()
        .rev()
        .take(capture_rows)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_string)
        .collect::<Vec<_>>();

    let mut lines = metadata.lines().map(str::to_string).collect::<Vec<_>>();
    if !capture_lines.is_empty() {
        lines.push("capture tail".to_string());
        lines.extend(capture_lines);
    }
    lines.join("\n")
}

pub fn state_marker(state: PopupStateKind) -> &'static str {
    match state {
        PopupStateKind::NeedsInput => "!",
        PopupStateKind::Alert => "!",
        PopupStateKind::Ready => "+",
        PopupStateKind::Failed => "x",
        PopupStateKind::Working => "*",
        PopupStateKind::Completed => "+",
        PopupStateKind::Idle => "-",
        PopupStateKind::Detached => ".",
        PopupStateKind::Previous => ".",
        PopupStateKind::Stale => "x",
    }
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
                window_id: Some(0),
                pane_index: Some(0),
                pane_id: Some(0),
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
    fn attention_grouping_orders_rows_by_state_priority() {
        let mut working = row("working", "working row", PopupRowKind::Item);
        working.state = PopupStateKind::Working;
        let mut needs = row("needs", "needs row", PopupRowKind::Item);
        needs.state = PopupStateKind::NeedsInput;

        let grouped = group_rows(vec![working, needs], PopupGrouping::Attention);

        assert_eq!(
            grouped
                .rows
                .iter()
                .map(|row| (row.kind, row.title.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (PopupRowKind::Header, "Needs input"),
                (PopupRowKind::Item, "needs row"),
                (PopupRowKind::Header, "Working"),
                (PopupRowKind::Item, "working row"),
            ]
        );
    }

    #[test]
    fn render_marks_selected_row_and_footer() {
        let model = PopupModel::new(vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha", PopupRowKind::Item),
        ]);
        let mut state = PopupState::new(PopupMode::Tree);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("Working"));
        assert!(text.contains("> * alpha"));
        assert!(text.contains("Enter: focus/attach"));
    }

    #[test]
    fn attention_footer_omits_enter_action() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("Space: peek"));
        assert!(!text.contains("Enter: focus/attach"), "{text}");
    }

    #[test]
    fn workspace_footer_shows_open_action() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("Enter: focus/attach"), "{text}");
        assert!(text.contains("o: open"), "{text}");
    }

    #[test]
    fn escape_closes_peek_before_popup() {
        let mut state = PopupState::new(PopupMode::Attention);
        state.peek = true;

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.peek);
        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }

    #[test]
    fn escape_cancels_reply_before_closing_peek() {
        let mut state = PopupState::new(PopupMode::Attention);
        state.peek = true;
        state.reply_mode = true;
        state.reply_text = "hello".to_string();

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(state.peek);
        assert!(!state.reply_mode);
        assert!(state.reply_text.is_empty());

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.peek);
    }

    #[test]
    fn render_shows_reply_prompt_when_replying_from_peek() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("a".to_string());
        state.peek = true;
        state.reply_mode = true;
        state.reply_text = "hello".to_string();

        let text = render_popup_text(&state, &model, Some("session=dev"));

        assert!(text.contains("Peek"), "{text}");
        assert!(text.contains("Reply: hello"), "{text}");
        assert!(text.contains("r: reply"), "{text}");
    }

    #[test]
    fn peek_text_is_bounded() {
        let long_capture = (0..100)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let text = render_peek_text(
            "repo=/tmp/repo\nsession=dev\npane=1",
            &long_capture,
            40,
            8 * 1024,
        );

        assert!(text.contains("repo=/tmp/repo"), "{text}");
        assert!(text.contains("session=dev"), "{text}");
        assert!(text.contains("pane=1"), "{text}");
        assert!(text.contains("line 99"), "{text}");
        assert!(text.lines().count() <= 43, "{text}");
    }
}
