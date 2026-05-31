use std::path::PathBuf;
use unicode_width::UnicodeWidthChar;

/// Minimum display width of the item title column, so short titles pad out and
/// summaries line up into a table for the common case.
const TITLE_WIDTH: usize = 18;

fn char_cells(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

/// Pad `text` with spaces to at least `min` display cells so the following
/// column aligns. Cell-aware so CJK titles pad correctly. Titles longer than
/// `min` are left intact (no information lost — the box truncates the row at its
/// edge if it overflows the popup width).
fn pad_title(text: &str, min: usize) -> String {
    let width: usize = text.chars().map(char_cells).sum();
    let mut out = text.to_string();
    if width < min {
        out.push_str(&" ".repeat(min - width));
    }
    out
}

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
    pub pinned: bool,
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
    pub confirm_mode: bool,
    pub confirm_prompt: String,
    pub confirm_target: Option<PopupTarget>,
    pub new_mode: bool,
    pub new_text: String,
    pub new_path: Option<PathBuf>,
    pub rename_mode: bool,
    pub rename_text: String,
    pub rename_target: Option<PopupTarget>,
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
            confirm_mode: false,
            confirm_prompt: String::new(),
            confirm_target: None,
            new_mode: false,
            new_text: String::new(),
            new_path: None,
            rename_mode: false,
            rename_text: String::new(),
            rename_target: None,
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
        } else if self.confirm_mode {
            self.confirm_mode = false;
            self.confirm_prompt.clear();
            self.confirm_target = None;
            PopupCloseResult::StayOpen
        } else if self.new_mode {
            self.new_mode = false;
            self.new_text.clear();
            self.new_path = None;
            PopupCloseResult::StayOpen
        } else if self.rename_mode {
            self.rename_mode = false;
            self.rename_text.clear();
            self.rename_target = None;
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

    // Defer each header until a matching item under it is found, so a group
    // whose items are all filtered out does not leave an orphan header behind.
    let mut result = Vec::new();
    let mut pending_header: Option<&PopupRow> = None;
    for row in rows {
        if row.kind == PopupRowKind::Header {
            pending_header = Some(row);
            continue;
        }
        if row_matches_filter(row, &needle) {
            if let Some(header) = pending_header.take() {
                result.push(header.clone());
            }
            result.push(row.clone());
        }
    }
    result
}

fn row_matches_filter(row: &PopupRow, needle: &str) -> bool {
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
    .contains(needle)
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
        pinned: false,
    }
}

/// A rendered popup split into final on-screen lines plus the metadata the
/// client needs to keep the selection visible (and, later, to highlight it):
/// `focus_line` is the index of the selected item's line, and `pinned_tail` is
/// the number of trailing chrome lines (peek/prompt/footer) that must stay on
/// screen rather than scroll with the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupView {
    pub lines: Vec<String>,
    pub focus_line: Option<usize>,
    pub pinned_tail: usize,
    /// (1-based position of the selection, total selectable rows) for the title
    /// bar's `N/M` indicator. None when nothing is selectable.
    pub count: Option<(usize, usize)>,
}

/// Convenience wrapper used by tests to assert on the flat rendered text.
#[cfg(test)]
pub fn render_popup_text(state: &PopupState, model: &PopupModel, peek: Option<&str>) -> String {
    render_popup_view(state, model, peek).lines.join("\n")
}

pub fn render_popup_view(state: &PopupState, model: &PopupModel, peek: Option<&str>) -> PopupView {
    let selectable = model.selectable_row_ids();
    let count = if selectable.is_empty() {
        None
    } else {
        let position = state
            .selected
            .as_deref()
            .and_then(|selected| selectable.iter().position(|id| id == selected))
            .map(|index| index + 1)
            .unwrap_or(1);
        Some((position, selectable.len()))
    };

    let mut list = Vec::new();
    let mut focus_line = None;
    for row in &model.rows {
        match row.kind {
            PopupRowKind::Header => {
                // Skip empty headers: the draw path drops blank lines, which
                // would otherwise shift focus_line off the selected row. Rule the
                // title so groups read as separators, not as list rows.
                let title = row.title.trim_end();
                if !title.is_empty() {
                    list.push(format!("── {title} ──"));
                }
            }
            PopupRowKind::Item | PopupRowKind::DisabledItem => {
                let selected = state.selected.as_deref() == Some(row.id.as_str());
                if selected {
                    focus_line = Some(list.len());
                }
                let marker = if selected { ">" } else { " " };
                let state_marker = state_marker(row.state);
                // Compact 2-cell flag column: p=pinned, d=disabled (left-aligned).
                let mut flags = String::new();
                if row.pinned {
                    flags.push('p');
                }
                if row.kind == PopupRowKind::DisabledItem {
                    flags.push('d');
                }
                while flags.chars().count() < 2 {
                    flags.push(' ');
                }
                list.push(
                    format!(
                        "{} {} {} {} {}",
                        marker,
                        state_marker,
                        flags,
                        pad_title(&row.title, TITLE_WIDTH),
                        row.summary
                    )
                    .trim_end()
                    .to_string(),
                );
            }
        }
    }

    // Trailing chrome: kept on screen (pinned) while the list scrolls under it.
    let mut chrome = Vec::new();
    if let Some(peek) = peek {
        chrome.push("Peek".to_string());
        chrome.extend(
            peek.lines()
                .map(str::trim_end)
                .filter(|line| !line.is_empty())
                .map(str::to_string),
        );
    }
    // Text-input sub-modes carry a synthetic '|' caret: the hardware cursor is
    // hidden while the popup is up, so without it an empty field looks frozen.
    if state.reply_mode {
        chrome.push(format!("Reply: {}|", state.reply_text));
    }
    if state.confirm_mode {
        chrome.push(format!("{} (y/N)", state.confirm_prompt));
    }
    if state.new_mode {
        chrome.push(format!("New session: {}|", state.new_text));
    }
    if state.rename_mode {
        chrome.push(format!("Rename: {}|", state.rename_text));
    }
    // Make the filter query visible — the footer advertises "/: filter" but the
    // typed text was previously shown nowhere.
    if state.filter_mode {
        chrome.push(format!("Filter: {}|", state.filter));
    } else if !state.filter.is_empty() {
        chrome.push(format!("Filter: {}   (Esc to clear)", state.filter));
    }
    // Footer hints. Long footers are split across two lines so the box does not
    // have to grow to ~150 cols and then truncate the hints mid-word (which cut
    // off "Esc: close" on an 80-column terminal).
    match state.mode {
        PopupMode::Attention => {
            chrome.push("Space: peek   r: reply   Tab: group   /: filter   Esc: close".to_string());
        }
        PopupMode::Workspace => {
            chrome.push(
                "Enter: focus/attach   o: open/reopen   n: new   R: rename   x: kill   p: pin"
                    .to_string(),
            );
            chrome.push(
                "J/K: reorder   Space: peek   r: reply   Tab: group   /: filter   Esc: close"
                    .to_string(),
            );
        }
        PopupMode::Tree => {
            chrome
                .push("Enter: focus/attach   R: rename window   x: kill   Space: peek".to_string());
            chrome.push("r: reply   Tab: group   /: filter   Esc: close".to_string());
        }
    }

    let pinned_tail = chrome.len();
    let mut lines = list;
    lines.extend(chrome);
    PopupView {
        lines,
        focus_line,
        pinned_tail,
        count,
    }
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
            pinned: false,
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
    fn filtering_drops_headers_with_no_matching_items() {
        let rows = vec![
            row("h1", "Working", PopupRowKind::Header),
            row("a", "alpha api", PopupRowKind::Item),
            row("h2", "Needs input", PopupRowKind::Header),
            row("b", "beta ui", PopupRowKind::Item),
        ];

        // "api" matches only the item under "Working"; the "Needs input" header
        // must not survive as an orphan with no rows beneath it.
        let filtered = filter_rows(&rows, "api");
        assert_eq!(
            filtered
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["h1", "a"]
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
        assert!(
            text.contains("> *"),
            "selected marker + state glyph: {text}"
        );
        assert!(text.contains("alpha"), "{text}");
        assert!(text.contains("Enter: focus/attach"));
    }

    #[test]
    fn pad_title_aligns_short_titles_and_preserves_long_ones() {
        assert_eq!(pad_title("abc", 6), "abc   ");
        assert_eq!(pad_title("abcdef", 6), "abcdef");
        // Longer than the column: left intact, never truncated.
        assert_eq!(pad_title("abcdefgh", 6), "abcdefgh");
        // CJK glyphs are two cells wide.
        assert_eq!(pad_title("한국", 6), "한국  ");
    }

    #[test]
    fn render_disabled_item_shows_flag_column() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::DisabledItem)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(
            text.contains("> * d"),
            "disabled flag in the flag column: {text}"
        );
        assert!(!text.contains("disabled"), "no English flag words: {text}");
    }

    #[test]
    fn render_popup_view_reports_focus_line_and_pinned_tail() {
        let model = PopupModel::new(vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha", PopupRowKind::Item),
            row("b", "beta", PopupRowKind::Item),
        ]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("b".to_string());

        let view = render_popup_view(&state, &model, None);

        // lines: [header, alpha, beta, footer]; "beta" is the 3rd line (index 2).
        assert_eq!(view.focus_line, Some(2));
        // Only the footer is pinned when there is no peek or sub-mode prompt.
        assert_eq!(view.pinned_tail, 1);
        assert!(
            view.lines.last().unwrap().contains("Esc: close"),
            "{view:?}"
        );
    }

    #[test]
    fn render_popup_view_pins_peek_and_prompt_chrome() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("a".to_string());
        state.reply_mode = true;
        state.reply_text = "hi".to_string();

        let view = render_popup_view(&state, &model, Some("line1\nline2"));

        // pinned tail = "Peek" + 2 capture lines + "Reply: hi" + footer = 5.
        assert_eq!(view.pinned_tail, 5);
        assert_eq!(view.focus_line, Some(0));
    }

    #[test]
    fn render_popup_view_shows_filter_prompt_with_caret() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("a".to_string());
        state.filter_mode = true;
        state.filter = "ap".to_string();

        let view = render_popup_view(&state, &model, None);

        assert!(
            view.lines.iter().any(|line| line == "Filter: ap|"),
            "filter prompt with caret: {view:?}"
        );
    }

    #[test]
    fn render_popup_view_text_inputs_show_caret() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());
        state.rename_mode = true;
        state.rename_text = "new-name".to_string();

        let view = render_popup_view(&state, &model, None);

        assert!(
            view.lines.iter().any(|line| line == "Rename: new-name|"),
            "rename field with caret: {view:?}"
        );
    }

    #[test]
    fn render_popup_view_rules_group_headers() {
        let model = PopupModel::new(vec![
            row("h", "Needs input", PopupRowKind::Header),
            row("a", "alpha", PopupRowKind::Item),
        ]);
        let mut state = PopupState::new(PopupMode::Attention);
        state.selected = Some("a".to_string());

        let view = render_popup_view(&state, &model, None);

        assert_eq!(view.lines[0], "── Needs input ──");
        // The item sits on the line after the ruled header.
        assert_eq!(view.focus_line, Some(1));
    }

    #[test]
    fn render_marks_pinned_rows() {
        let mut pinned = row("a", "alpha", PopupRowKind::Item);
        pinned.pinned = true;
        let model = PopupModel::new(vec![pinned]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("alpha"), "{text}");
        assert!(
            text.contains("> * p"),
            "pinned flag in the flag column: {text}"
        );
        assert!(!text.contains("pinned"), "no English flag words: {text}");
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
    fn workspace_footer_splits_across_two_lines_without_truncation() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());

        let view = render_popup_view(&state, &model, None);

        // The footer wraps to two lines that each fit a typical popup width, with
        // the always-important Esc hint on the final line (never truncated).
        assert!(view.pinned_tail >= 2, "{view:?}");
        assert!(
            view.lines.last().unwrap().contains("Esc: close"),
            "{view:?}"
        );
        assert!(
            view.lines.iter().all(|line| line.chars().count() <= 78),
            "footer lines must fit a typical popup width: {view:?}"
        );
    }

    #[test]
    fn tree_footer_shows_kill_action() {
        let model = PopupModel::new(vec![row("a", "pane 0", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Tree);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("x: kill"), "{text}");
    }

    #[test]
    fn tree_footer_shows_rename_window_action() {
        let model = PopupModel::new(vec![row("a", "pane 0", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Tree);
        state.selected = Some("a".to_string());

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("R: rename window"), "{text}");
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
    fn escape_cancels_confirm_before_closing_popup() {
        let mut state = PopupState::new(PopupMode::Workspace);
        state.confirm_mode = true;
        state.confirm_prompt = "kill session dev?".to_string();
        state.confirm_target = Some(PopupTarget {
            session: "dev".to_string(),
            window_index: None,
            window_id: None,
            pane_index: None,
            pane_id: None,
        });

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.confirm_mode);
        assert!(state.confirm_prompt.is_empty());
        assert!(state.confirm_target.is_none());

        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }

    #[test]
    fn render_shows_confirm_prompt_when_confirming() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());
        state.confirm_mode = true;
        state.confirm_prompt = "kill session dev?".to_string();

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("kill session dev? (y/N)"), "{text}");
        assert!(text.contains("x: kill"), "{text}");
    }

    #[test]
    fn escape_cancels_new_session_before_closing_popup() {
        let mut state = PopupState::new(PopupMode::Workspace);
        state.new_mode = true;
        state.new_text = "api".to_string();
        state.new_path = Some(PathBuf::from("/tmp/repo"));

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.new_mode);
        assert!(state.new_text.is_empty());
        assert!(state.new_path.is_none());

        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }

    #[test]
    fn render_shows_new_session_prompt_when_naming() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());
        state.new_mode = true;
        state.new_text = "api".to_string();

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("New session: api"), "{text}");
        assert!(text.contains("n: new"), "{text}");
    }

    #[test]
    fn escape_cancels_rename_before_closing_popup() {
        let mut state = PopupState::new(PopupMode::Workspace);
        state.rename_mode = true;
        state.rename_text = "api".to_string();
        state.rename_target = Some(PopupTarget {
            session: "dev".to_string(),
            window_index: None,
            window_id: None,
            pane_index: None,
            pane_id: None,
        });

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.rename_mode);
        assert!(state.rename_text.is_empty());
        assert!(state.rename_target.is_none());

        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }

    #[test]
    fn render_shows_rename_prompt_when_renaming() {
        let model = PopupModel::new(vec![row("a", "alpha", PopupRowKind::Item)]);
        let mut state = PopupState::new(PopupMode::Workspace);
        state.selected = Some("a".to_string());
        state.rename_mode = true;
        state.rename_text = "renamed".to_string();

        let text = render_popup_text(&state, &model, None);

        assert!(text.contains("Rename: renamed"), "{text}");
        assert!(text.contains("R: rename"), "{text}");
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
