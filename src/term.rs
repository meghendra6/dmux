use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;

pub struct TerminalState {
    screen: TerminalScreen,
    alternate_screen: Option<TerminalScreen>,
    use_alternate_screen: bool,
    cursor_visible: bool,
    scrollback: Scrollback,
    parser: vte::Parser,
    style: CellStyle,
    changes: TerminalChanges,
    render_next_primary_output_immediately: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminalChanges {
    pub alternate_screen: bool,
    pub post_alternate_screen_exit: bool,
}

impl TerminalChanges {
    pub fn requires_immediate_render(self) -> bool {
        self.alternate_screen || self.post_alternate_screen_exit
    }
}

impl TerminalState {
    pub fn new(width: usize, height: usize, max_scrollback_lines: usize) -> Self {
        Self {
            screen: TerminalScreen::new(width, height),
            alternate_screen: None,
            use_alternate_screen: false,
            cursor_visible: true,
            scrollback: Scrollback::new(max_scrollback_lines),
            parser: vte::Parser::new(),
            style: CellStyle::default(),
            changes: TerminalChanges::default(),
            render_next_primary_output_immediately: false,
        }
    }

    pub fn apply_bytes(&mut self, bytes: &[u8]) -> TerminalChanges {
        self.changes = TerminalChanges {
            post_alternate_screen_exit: self.render_next_primary_output_immediately
                && !bytes.is_empty(),
            ..TerminalChanges::default()
        };
        self.render_next_primary_output_immediately = false;
        let mut parser = std::mem::take(&mut self.parser);
        for byte in bytes {
            parser.advance(self, *byte);
        }
        self.parser = parser;
        let changes = self.changes;
        self.changes = TerminalChanges::default();
        changes
    }

    pub fn capture_text(&self) -> String {
        join_capture_lines(
            self.scrollback
                .lines
                .iter()
                .cloned()
                .chain(self.active_screen().non_empty_lines())
                .collect(),
        )
    }

    pub fn capture_history_text(&self) -> String {
        join_capture_lines(self.scrollback.lines.iter().cloned().collect())
    }

    pub fn capture_screen_text(&self) -> String {
        join_capture_lines(self.active_screen().non_empty_lines())
    }

    #[allow(dead_code)]
    pub fn render_screen_ansi_text(&self) -> String {
        self.active_screen().render_ansi_text()
    }

    #[allow(dead_code)]
    pub fn render_screen_ansi_lines(&self, width: usize, height: usize) -> Vec<String> {
        self.active_screen().render_ansi_lines(width, height)
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.active_screen().cursor_position()
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.screen.resize(width, height);
        if let Some(screen) = &mut self.alternate_screen {
            screen.resize(width, height);
        }
    }

    fn line_feed(&mut self) {
        let style = self.style;
        if self.use_alternate_screen {
            self.active_screen_mut().line_feed_without_scrollback(style);
        } else {
            self.screen.line_feed(style, Some(&mut self.scrollback));
        }
    }

    fn tab(&mut self) {
        let next_tab = ((self.active_screen().cursor_col / 8) + 1) * 8;
        while self.active_screen().cursor_col < next_tab {
            self.put_char(' ');
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.use_alternate_screen {
            let style = self.style;
            self.active_screen_mut()
                .put_char_without_scrollback(ch, style);
        } else {
            self.screen.put_char(ch, self.style, &mut self.scrollback);
        }
    }

    fn active_screen(&self) -> &TerminalScreen {
        if self.use_alternate_screen {
            self.alternate_screen.as_ref().unwrap_or(&self.screen)
        } else {
            &self.screen
        }
    }

    fn active_screen_mut(&mut self) -> &mut TerminalScreen {
        if self.use_alternate_screen {
            self.alternate_screen
                .get_or_insert_with(|| TerminalScreen::new(self.screen.width, self.screen.height))
        } else {
            &mut self.screen
        }
    }

    fn enter_alternate_screen(&mut self) {
        self.changes.alternate_screen = true;
        self.alternate_screen = Some(TerminalScreen::new(self.screen.width, self.screen.height));
        self.use_alternate_screen = true;
    }

    fn exit_alternate_screen(&mut self) {
        self.changes.alternate_screen = true;
        self.render_next_primary_output_immediately = true;
        self.use_alternate_screen = false;
    }

    fn reset_terminal(&mut self) {
        if self.use_alternate_screen || self.alternate_screen.is_some() {
            self.changes.alternate_screen = true;
            self.render_next_primary_output_immediately = true;
        }
        self.screen = TerminalScreen::new(self.screen.width, self.screen.height);
        self.alternate_screen = None;
        self.use_alternate_screen = false;
        self.cursor_visible = true;
        self.scrollback.clear();
        self.style = CellStyle::default();
    }

    fn mark_primary_output_after_alternate_screen_exit(&mut self) {
        if !self.use_alternate_screen && self.render_next_primary_output_immediately {
            self.changes.post_alternate_screen_exit = true;
            self.render_next_primary_output_immediately = false;
        }
    }

    fn apply_sgr(&mut self, params: &vte::Params) {
        let params = sgr_params(params);
        if params.is_empty() {
            self.style = CellStyle::default();
            return;
        }

        let mut index = 0;
        while index < params.len() {
            let param = &params[index];
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.style = CellStyle::default(),
                1 => self.style.bold = true,
                2 => self.style.dim = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                7 => self.style.inverse = true,
                22 => {
                    self.style.bold = false;
                    self.style.dim = false;
                }
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.fg = Some(Color::Ansi(code as u8 - 30)),
                39 => self.style.fg = None,
                40..=47 => self.style.bg = Some(Color::Ansi(code as u8 - 40)),
                49 => self.style.bg = None,
                90..=97 => self.style.fg = Some(Color::Ansi(code as u8 - 90 + 8)),
                100..=107 => self.style.bg = Some(Color::Ansi(code as u8 - 100 + 8)),
                38 | 48 => {
                    if let Some((color, consumed)) = parse_sgr_color(param, &params[index + 1..]) {
                        if code == 38 {
                            self.style.fg = Some(color);
                        } else {
                            self.style.bg = Some(color);
                        }
                        index += consumed;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

impl vte::Perform for TerminalState {
    fn print(&mut self, c: char) {
        self.mark_primary_output_after_alternate_screen_exit();
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.mark_primary_output_after_alternate_screen_exit();
        match byte {
            b'\r' => self.active_screen_mut().carriage_return(),
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\t' => self.tab(),
            b'\x08' => self.active_screen_mut().backspace(),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore {
            return;
        }

        match action {
            'm' => self.apply_sgr(params),
            'J' => {
                let style = self.style;
                match first_param(params, 0) {
                    0 => self.active_screen_mut().clear_display_from_cursor(style),
                    1 => self.active_screen_mut().clear_display_to_cursor(style),
                    2 => self.active_screen_mut().clear_display(style),
                    3 => self.scrollback.clear(),
                    _ => {}
                }
            }
            'K' => {
                let style = self.style;
                match first_param(params, 0) {
                    0 => self.active_screen_mut().clear_line_from_cursor(style),
                    1 => self.active_screen_mut().clear_line_to_cursor(style),
                    2 => self.active_screen_mut().clear_line(style),
                    _ => {}
                }
            }
            'H' | 'f' => {
                let (row, col) = cursor_position(params);
                self.active_screen_mut().move_cursor(row, col);
            }
            'A' => self.active_screen_mut().move_up(first_param(params, 1)),
            'B' => self.active_screen_mut().move_down(first_param(params, 1)),
            'C' => self.active_screen_mut().move_right(first_param(params, 1)),
            'D' => self.active_screen_mut().move_left(first_param(params, 1)),
            'E' => {
                let count = first_param(params, 1);
                let screen = self.active_screen_mut();
                screen.move_down(count);
                screen.carriage_return();
            }
            'F' => {
                let count = first_param(params, 1);
                let screen = self.active_screen_mut();
                screen.move_up(count);
                screen.carriage_return();
            }
            'G' => self.active_screen_mut().move_to_col(first_param(params, 1)),
            'd' => self.active_screen_mut().move_to_row(first_param(params, 1)),
            'r' => {
                let top = nth_param(params, 0, 1);
                let bottom = nth_param(params, 1, self.active_screen().height);
                self.active_screen_mut().set_scroll_region(top, bottom);
            }
            'M' => {
                let count = first_param(params, 1);
                let style = self.style;
                self.active_screen_mut()
                    .delete_lines_in_scroll_region(count, style);
            }
            'L' => {
                let count = first_param(params, 1);
                let style = self.style;
                self.active_screen_mut()
                    .insert_lines_in_scroll_region(count, style);
            }
            'S' if !is_private_mode_sequence(intermediates) => {
                let count = first_param(params, 1);
                let style = self.style;
                if self.use_alternate_screen {
                    self.active_screen_mut()
                        .scroll_region_up(count, style, None);
                } else {
                    self.screen
                        .scroll_region_up(count, style, Some(&mut self.scrollback));
                }
            }
            'T' => {
                let count = first_param(params, 1);
                let style = self.style;
                self.active_screen_mut().scroll_region_down(count, style);
            }
            'X' => {
                let count = first_param(params, 1);
                let style = self.style;
                self.active_screen_mut().erase_chars(count, style);
            }
            's' => self.active_screen_mut().save_cursor(),
            'u' => self.active_screen_mut().restore_cursor(),
            'h' if is_private_mode_sequence(intermediates) => {
                self.apply_private_modes(params, true)
            }
            'l' if is_private_mode_sequence(intermediates) => {
                self.apply_private_modes(params, false);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }

        match byte {
            b'7' => self.active_screen_mut().save_cursor(),
            b'8' => self.active_screen_mut().restore_cursor(),
            b'D' => self.line_feed(),
            b'E' => {
                self.active_screen_mut().carriage_return();
                self.line_feed();
            }
            b'M' => {
                let style = self.style;
                self.active_screen_mut().reverse_index(style);
            }
            b'c' => self.reset_terminal(),
            _ => {}
        }
    }
}

impl TerminalState {
    fn apply_private_modes(&mut self, params: &vte::Params, enabled: bool) {
        for mode in flat_params(params) {
            match (mode, enabled) {
                (1049 | 1047, true) => self.enter_alternate_screen(),
                (1049 | 1047, false) => self.exit_alternate_screen(),
                (25, true) => self.cursor_visible = true,
                (25, false) => self.cursor_visible = false,
                (6, true) => self.active_screen_mut().set_origin_mode(true),
                (6, false) => self.active_screen_mut().set_origin_mode(false),
                _ => {}
            }
        }
    }
}

fn join_capture_lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

struct TerminalScreen {
    width: usize,
    height: usize,
    rows: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: Option<(usize, usize)>,
    scroll_top: usize,
    scroll_bottom: usize,
    origin_mode: bool,
}

impl TerminalScreen {
    fn new(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            rows: vec![vec![Cell::blank(); width]; height],
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: height - 1,
            origin_mode: false,
        }
    }

    fn put_char(&mut self, ch: char, style: CellStyle, scrollback: &mut Scrollback) {
        let width = char_cell_width(ch).min(self.width);
        if self.cursor_col + width > self.width {
            self.cursor_col = 0;
            self.line_feed(style, Some(scrollback));
        }
        if self.cursor_col >= self.width {
            self.cursor_col = 0;
            self.line_feed(style, Some(scrollback));
        }

        self.write_char_cells(ch, style, width);
    }

    fn put_char_without_scrollback(&mut self, ch: char, style: CellStyle) {
        let width = char_cell_width(ch).min(self.width);
        if self.cursor_col + width > self.width {
            self.cursor_col = 0;
            self.line_feed_without_scrollback(style);
        }
        if self.cursor_col >= self.width {
            self.cursor_col = 0;
            self.line_feed_without_scrollback(style);
        }

        self.write_char_cells(ch, style, width);
    }

    fn write_char_cells(&mut self, ch: char, style: CellStyle, width: usize) {
        self.rows[self.cursor_row][self.cursor_col] = Cell {
            ch,
            style,
            wide_continuation: false,
        };
        for offset in 1..width {
            self.rows[self.cursor_row][self.cursor_col + offset] = Cell {
                ch: ' ',
                style,
                wide_continuation: true,
            };
        }
        self.cursor_col += width;
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    fn line_feed(&mut self, style: CellStyle, scrollback: Option<&mut Scrollback>) {
        if self.cursor_row == self.scroll_bottom && self.cursor_in_scroll_region() {
            self.scroll_region_up(1, style, scrollback);
        } else {
            self.cursor_row = (self.cursor_row + 1).min(self.height - 1);
        }
    }

    fn line_feed_without_scrollback(&mut self, style: CellStyle) {
        self.line_feed(style, None);
    }

    fn reverse_index(&mut self, style: CellStyle) {
        if self.cursor_row == self.scroll_top && self.cursor_in_scroll_region() {
            self.scroll_region_down(1, style);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    fn backspace(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
        while self.cursor_col > 0 && self.rows[self.cursor_row][self.cursor_col].wide_continuation {
            self.cursor_col -= 1;
        }
    }

    fn clear_display(&mut self, style: CellStyle) {
        self.rows = vec![self.blank_row(style); self.height];
    }

    fn clear_display_from_cursor(&mut self, style: CellStyle) {
        for col in self.cursor_col..self.width {
            self.rows[self.cursor_row][col] = Cell::blank_with_style(style);
        }
        for row in (self.cursor_row + 1)..self.height {
            self.rows[row] = self.blank_row(style);
        }
    }

    fn clear_display_to_cursor(&mut self, style: CellStyle) {
        for row in 0..self.cursor_row {
            self.rows[row] = self.blank_row(style);
        }
        for col in 0..=self.cursor_col.min(self.width - 1) {
            self.rows[self.cursor_row][col] = Cell::blank_with_style(style);
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let height = height.max(1);
        let old_cursor_col = self.cursor_col;
        let old_height = self.height;
        let old_width = self.width;
        let copy_rows = old_height.min(height);
        let copy_cols = old_width.min(width);
        let source_row_start = if height < old_height && self.cursor_row >= height {
            (self.cursor_row + 1 - height).min(old_height - copy_rows)
        } else {
            0
        };
        let mut rows = vec![vec![Cell::blank(); width]; height];

        for (row_index, row) in rows.iter_mut().enumerate().take(copy_rows) {
            let source_row = source_row_start + row_index;
            row[..copy_cols].copy_from_slice(&self.rows[source_row][..copy_cols]);
        }

        self.width = width;
        self.height = height;
        self.rows = rows;
        self.reset_scroll_region();
        self.cursor_row = self
            .cursor_row
            .saturating_sub(source_row_start)
            .min(self.height - 1);
        if old_cursor_col >= self.width {
            self.cursor_col = 0;
            self.cursor_row = (self.cursor_row + 1).min(self.height - 1);
        } else {
            self.cursor_col = self.cursor_col.min(self.width - 1);
        }
        if let Some((row, col)) = self.saved_cursor {
            self.saved_cursor = Some((row.saturating_sub(source_row_start), col));
        }
    }

    fn clear_line_from_cursor(&mut self, style: CellStyle) {
        for col in self.cursor_col..self.width {
            self.rows[self.cursor_row][col] = Cell::blank_with_style(style);
        }
    }

    fn clear_line_to_cursor(&mut self, style: CellStyle) {
        for col in 0..=self.cursor_col.min(self.width - 1) {
            self.rows[self.cursor_row][col] = Cell::blank_with_style(style);
        }
    }

    fn clear_line(&mut self, style: CellStyle) {
        self.rows[self.cursor_row] = self.blank_row(style);
    }

    fn move_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = self.resolve_cursor_row(row);
        self.cursor_col = col.saturating_sub(1).min(self.width - 1);
    }

    fn move_to_row(&mut self, row: usize) {
        self.cursor_row = self.resolve_cursor_row(row);
    }

    fn move_to_col(&mut self, col: usize) {
        self.cursor_col = col.saturating_sub(1).min(self.width - 1);
    }

    fn move_up(&mut self, count: usize) {
        if self.cursor_in_scroll_region() {
            self.cursor_row = self.cursor_row.saturating_sub(count).max(self.scroll_top);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(count);
        }
    }

    fn move_down(&mut self, count: usize) {
        if self.cursor_in_scroll_region() {
            self.cursor_row = (self.cursor_row + count).min(self.scroll_bottom);
        } else {
            self.cursor_row = (self.cursor_row + count).min(self.height - 1);
        }
    }

    fn move_right(&mut self, count: usize) {
        self.cursor_col = (self.cursor_col + count).min(self.width - 1);
    }

    fn move_left(&mut self, count: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(count);
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor_row, self.cursor_col));
    }

    fn restore_cursor(&mut self) {
        if let Some((row, col)) = self.saved_cursor {
            self.cursor_row = row.min(self.height - 1);
            self.cursor_col = col.min(self.width - 1);
        }
    }

    fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.saturating_sub(1).min(self.height - 1);
        let bottom = bottom.saturating_sub(1).min(self.height - 1);
        if top >= bottom {
            return;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        self.cursor_col = 0;
        self.cursor_row = if self.origin_mode { self.scroll_top } else { 0 };
    }

    fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.height - 1;
    }

    fn set_origin_mode(&mut self, enabled: bool) {
        self.origin_mode = enabled;
        self.cursor_col = 0;
        self.cursor_row = if enabled { self.scroll_top } else { 0 };
    }

    fn scroll_region_up(
        &mut self,
        count: usize,
        style: CellStyle,
        mut scrollback: Option<&mut Scrollback>,
    ) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);
        for _ in 0..count {
            if self.scroll_top == 0 {
                if let Some(scrollback) = scrollback.as_deref_mut() {
                    scrollback.push(self.row_to_string(0));
                }
            }
            self.rows.remove(self.scroll_top);
            self.rows.insert(self.scroll_bottom, self.blank_row(style));
        }
    }

    fn scroll_region_down(&mut self, count: usize, style: CellStyle) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);
        for _ in 0..count {
            self.rows.remove(self.scroll_bottom);
            self.rows.insert(self.scroll_top, self.blank_row(style));
        }
    }

    fn delete_lines_in_scroll_region(&mut self, count: usize, style: CellStyle) {
        if !self.cursor_in_scroll_region() {
            return;
        }
        let count = count.min(self.scroll_bottom - self.cursor_row + 1);
        for _ in 0..count {
            self.rows.remove(self.cursor_row);
            self.rows.insert(self.scroll_bottom, self.blank_row(style));
        }
        self.cursor_col = 0;
    }

    fn insert_lines_in_scroll_region(&mut self, count: usize, style: CellStyle) {
        if !self.cursor_in_scroll_region() {
            return;
        }
        let count = count.min(self.scroll_bottom - self.cursor_row + 1);
        for _ in 0..count {
            self.rows.remove(self.scroll_bottom);
            self.rows.insert(self.cursor_row, self.blank_row(style));
        }
        self.cursor_col = 0;
    }

    fn erase_chars(&mut self, count: usize, style: CellStyle) {
        let end = (self.cursor_col + count).min(self.width);
        for col in self.cursor_col..end {
            self.rows[self.cursor_row][col] = Cell::blank_with_style(style);
        }
    }

    fn cursor_in_scroll_region(&self) -> bool {
        self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom
    }

    fn resolve_cursor_row(&self, row: usize) -> usize {
        let row = row.saturating_sub(1);
        if self.origin_mode {
            (self.scroll_top + row).min(self.scroll_bottom)
        } else {
            row.min(self.height - 1)
        }
    }

    fn blank_row(&self, style: CellStyle) -> Vec<Cell> {
        vec![Cell::blank_with_style(style); self.width]
    }

    fn non_empty_lines(&self) -> Vec<String> {
        let mut lines = self
            .rows
            .iter()
            .map(|row| {
                trim_trailing_spaces(
                    row.iter()
                        .filter(|cell| !cell.wide_continuation)
                        .map(|cell| cell.ch)
                        .collect::<String>(),
                )
            })
            .collect::<Vec<_>>();

        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }

        lines
    }

    fn row_to_string(&self, row: usize) -> String {
        trim_trailing_spaces(
            self.rows[row]
                .iter()
                .filter(|cell| !cell.wide_continuation)
                .map(|cell| cell.ch)
                .collect::<String>(),
        )
    }

    #[allow(dead_code)]
    fn render_ansi_text(&self) -> String {
        let Some(last_row) = self.last_non_empty_row() else {
            return String::new();
        };

        let mut output = String::new();
        let mut current_style = CellStyle::default();
        for row_index in 0..=last_row {
            let row = &self.rows[row_index];
            let last_col = last_non_blank_cell(row).unwrap_or(0);
            for cell in row.iter().take(last_col + 1) {
                if cell.wide_continuation {
                    continue;
                }
                if cell.style != current_style {
                    output.push_str(&style_transition_from(current_style, cell.style));
                    current_style = cell.style;
                }
                output.push(cell.ch);
            }
            if current_style != CellStyle::default() {
                output.push_str("\x1b[0m");
                current_style = CellStyle::default();
            }
            output.push('\n');
        }
        output
    }

    #[allow(dead_code)]
    fn render_ansi_lines(&self, width: usize, height: usize) -> Vec<String> {
        (0..height)
            .map(|row_index| self.render_ansi_line(row_index, width))
            .collect()
    }

    fn render_ansi_line(&self, row_index: usize, width: usize) -> String {
        let mut output = String::new();
        let mut current_style = CellStyle::default();
        let mut styled_text_emitted = false;
        let row = self.rows.get(row_index);
        let cells = row.into_iter().flat_map(|row| row.iter());
        let mut visible_width = 0;

        for cell in cells {
            if cell.wide_continuation {
                continue;
            }
            let cell_width = char_cell_width(cell.ch);
            if visible_width + cell_width > width {
                break;
            }
            if cell.style != current_style {
                output.push_str(&style_transition_from(current_style, cell.style));
                current_style = cell.style;
            }
            if cell.style != CellStyle::default() {
                styled_text_emitted = true;
            }
            output.push(cell.ch);
            visible_width += cell_width;
        }

        if current_style != CellStyle::default() && styled_text_emitted {
            output.push_str("\x1b[0m");
        }
        if visible_width < width {
            output.push_str(&" ".repeat(width - visible_width));
        }

        output
    }

    #[allow(dead_code)]
    fn last_non_empty_row(&self) -> Option<usize> {
        self.rows.iter().rposition(|row| {
            row.iter()
                .any(|cell| !cell.wide_continuation && cell.ch != ' ')
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: CellStyle,
    wide_continuation: bool,
}

impl Cell {
    fn blank() -> Self {
        Self::blank_with_style(CellStyle::default())
    }

    fn blank_with_style(style: CellStyle) -> Self {
        Self {
            ch: ' ',
            style,
            wide_continuation: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CellStyle {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Color {
    Ansi(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

struct Scrollback {
    lines: VecDeque<String>,
    max_lines: usize,
}

impl Scrollback {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines,
        }
    }

    fn push(&mut self, line: String) {
        if self.max_lines == 0 {
            return;
        }
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
    }

    fn clear(&mut self) {
        self.lines.clear();
    }
}

fn flat_params(params: &vte::Params) -> Vec<usize> {
    params
        .iter()
        .flat_map(|param| param.iter().copied())
        .map(usize::from)
        .collect()
}

fn sgr_params(params: &vte::Params) -> Vec<Vec<usize>> {
    params
        .iter()
        .map(|param| param.iter().copied().map(usize::from).collect())
        .collect()
}

fn first_param(params: &vte::Params, default: usize) -> usize {
    params
        .iter()
        .next()
        .and_then(|param| param.first())
        .copied()
        .map(usize::from)
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn nth_param(params: &vte::Params, index: usize, default: usize) -> usize {
    params
        .iter()
        .nth(index)
        .and_then(|param| param.first())
        .copied()
        .map(usize::from)
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn cursor_position(params: &vte::Params) -> (usize, usize) {
    (nth_param(params, 0, 1), nth_param(params, 1, 1))
}

fn is_private_mode_sequence(intermediates: &[u8]) -> bool {
    intermediates.contains(&b'?')
}

fn parse_sgr_color(param: &[usize], following: &[Vec<usize>]) -> Option<(Color, usize)> {
    if param.len() > 1 {
        return parse_sgr_color_components(&param[1..]).map(|color| (color, 0));
    }

    let mode = following.first()?.first().copied()?;
    match mode {
        5 => {
            let value = following.get(1)?.first().copied()?;
            u8::try_from(value)
                .ok()
                .map(|value| (Color::Indexed(value), 2))
        }
        2 => {
            let red = following.get(1)?.first().copied()?;
            let green = following.get(2)?.first().copied()?;
            let blue = following.get(3)?.first().copied()?;
            parse_rgb(red, green, blue).map(|color| (color, 4))
        }
        _ => None,
    }
}

fn parse_sgr_color_components(components: &[usize]) -> Option<Color> {
    match components {
        [5, value, ..] => u8::try_from(*value).ok().map(Color::Indexed),
        [2, _color_space, red, green, blue, ..] => parse_rgb(*red, *green, *blue),
        [2, red, green, blue, ..] => parse_rgb(*red, *green, *blue),
        _ => None,
    }
}

fn parse_rgb(red: usize, green: usize, blue: usize) -> Option<Color> {
    Some(Color::Rgb(
        u8::try_from(red).ok()?,
        u8::try_from(green).ok()?,
        u8::try_from(blue).ok()?,
    ))
}

#[allow(dead_code)]
fn last_non_blank_cell(row: &[Cell]) -> Option<usize> {
    row.iter()
        .rposition(|cell| !cell.wide_continuation && cell.ch != ' ')
}

#[allow(dead_code)]
fn style_transition(style: CellStyle) -> String {
    if style == CellStyle::default() {
        return "\x1b[0m".to_string();
    }

    let mut params = Vec::new();
    if style.bold {
        params.push("1".to_string());
    }
    if style.dim {
        params.push("2".to_string());
    }
    if style.italic {
        params.push("3".to_string());
    }
    if style.underline {
        params.push("4".to_string());
    }
    if style.inverse {
        params.push("7".to_string());
    }
    if let Some(color) = style.fg {
        push_color_params(&mut params, 30, 90, "38", color);
    }
    if let Some(color) = style.bg {
        push_color_params(&mut params, 40, 100, "48", color);
    }

    format!("\x1b[{}m", params.join(";"))
}

#[allow(dead_code)]
fn style_transition_from(current: CellStyle, target: CellStyle) -> String {
    if target == CellStyle::default() {
        return "\x1b[0m".to_string();
    }
    if current == CellStyle::default() {
        return style_transition(target);
    }

    let mut transition = "\x1b[0m".to_string();
    transition.push_str(&style_transition(target));
    transition
}

#[allow(dead_code)]
fn push_color_params(
    params: &mut Vec<String>,
    base: u8,
    bright_base: u8,
    extended_prefix: &str,
    color: Color,
) {
    match color {
        Color::Ansi(value) if value < 8 => params.push((base + value).to_string()),
        Color::Ansi(value) if value < 16 => params.push((bright_base + value - 8).to_string()),
        Color::Ansi(value) => {
            params.push(extended_prefix.to_string());
            params.push("5".to_string());
            params.push(value.to_string());
        }
        Color::Indexed(value) => {
            params.push(extended_prefix.to_string());
            params.push("5".to_string());
            params.push(value.to_string());
        }
        Color::Rgb(red, green, blue) => {
            params.push(extended_prefix.to_string());
            params.push("2".to_string());
            params.push(red.to_string());
            params.push(green.to_string());
            params.push(blue.to_string());
        }
    }
}

fn trim_trailing_spaces(mut value: String) -> String {
    while value.ends_with(' ') {
        value.pop();
    }
    value
}

fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriage_return_rewrites_current_line() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"hello\rworld");
        assert_eq!(state.capture_text(), "world\n");
    }

    #[test]
    fn sgr_sequences_do_not_appear_in_capture() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"\x1b[31mred\x1b[0m\n");
        let captured = state.capture_text();
        assert!(captured.contains("red"), "{captured:?}");
        assert!(!captured.contains('\x1b'), "{captured:?}");
    }

    #[test]
    fn styled_render_preserves_sgr_foreground_and_reset() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"\x1b[31mred\x1b[0m plain");

        assert_eq!(state.capture_screen_text(), "red plain\n");
        assert_eq!(
            state.render_screen_ansi_text(),
            "\x1b[31mred\x1b[0m plain\n"
        );
    }

    #[test]
    fn styled_render_preserves_truecolor_and_bold() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"\x1b[1;38;2;1;2;3mhi\x1b[0m");

        assert_eq!(
            state.render_screen_ansi_text(),
            "\x1b[1;38;2;1;2;3mhi\x1b[0m\n"
        );
    }

    #[test]
    fn styled_render_preserves_colon_truecolor_sgr() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"\x1b[38:2:1:2:3mfg\x1b[48:2::4:5:6mbg\x1b[0m");

        assert_eq!(
            state.render_screen_ansi_text(),
            "\x1b[38;2;1;2;3mfg\x1b[0m\x1b[38;2;1;2;3;48;2;4;5;6mbg\x1b[0m\n"
        );
    }

    #[test]
    fn erase_line_preserves_current_background_style() {
        let mut state = TerminalState::new(4, 2, 100);
        state.apply_bytes(b"\x1b[48;2;1;2;3m\x1b[K");

        assert_eq!(
            state.render_screen_ansi_lines(4, 1),
            vec!["\x1b[48;2;1;2;3m    \x1b[0m"]
        );
    }

    #[test]
    fn erase_line_preserves_reverse_video_style() {
        let mut state = TerminalState::new(4, 2, 100);
        state.apply_bytes(b"\x1b[31;44;7m\x1b[K");

        assert_eq!(
            state.render_screen_ansi_lines(4, 1),
            vec!["\x1b[7;31;44m    \x1b[0m"]
        );
    }

    #[test]
    fn erase_display_and_erase_chars_preserve_current_background_style() {
        let mut state = TerminalState::new(4, 2, 100);
        state.apply_bytes(b"\x1b[48;5;24m\x1b[2J");

        assert_eq!(
            state.render_screen_ansi_lines(4, 1),
            vec!["\x1b[48;5;24m    \x1b[0m"]
        );

        let mut state = TerminalState::new(4, 2, 100);
        state.apply_bytes(b"abcd\x1b[1;2H\x1b[48;5;25m\x1b[2X");
        assert_eq!(
            state.render_screen_ansi_lines(4, 1),
            vec!["a\x1b[48;5;25m  \x1b[0md"]
        );
    }

    #[test]
    fn styled_render_lines_clip_and_pad_visible_cells() {
        let mut state = TerminalState::new(10, 2, 100);
        state.apply_bytes(b"\x1b[31mabcdef\x1b[0m");

        assert_eq!(
            state.render_screen_ansi_lines(4, 2),
            vec!["\x1b[31mabcd\x1b[0m".to_string(), "    ".to_string()]
        );
    }

    #[test]
    fn styled_render_resets_removed_attributes_between_cells() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"\x1b[1;31mA\x1b[22;32mB\x1b[39;1mC");

        assert_eq!(
            state.render_screen_ansi_text(),
            "\x1b[1;31mA\x1b[0m\x1b[32mB\x1b[0m\x1b[1mC\x1b[0m\n"
        );
    }

    #[test]
    fn alternate_screen_preserves_primary_screen() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"primary");
        state.apply_bytes(b"\x1b[?1049halternate");

        assert_eq!(state.capture_screen_text(), "alternate\n");

        state.apply_bytes(b"\x1b[?1049l");
        assert_eq!(state.capture_screen_text(), "primary\n");
    }

    #[test]
    fn apply_bytes_reports_alternate_screen_transitions() {
        let mut state = TerminalState::new(20, 3, 100);

        assert!(!state.apply_bytes(b"primary").alternate_screen);
        assert!(state.apply_bytes(b"\x1b[?1049halternate").alternate_screen);
        assert!(!state.apply_bytes(b"more").alternate_screen);
        assert!(state.apply_bytes(b"\x1b[?1049lprimary").alternate_screen);
        assert!(
            state
                .apply_bytes(b"\x1b[?1049hshort\x1b[?1049l")
                .alternate_screen
        );
    }

    #[test]
    fn apply_bytes_reports_first_primary_output_after_alternate_screen_exit() {
        let mut state = TerminalState::new(20, 3, 100);

        state.apply_bytes(b"\x1b[?1049halternate");
        let exit = state.apply_bytes(b"\x1b[?1049l");
        assert!(exit.alternate_screen);
        assert!(!exit.post_alternate_screen_exit);

        let prompt = state.apply_bytes(b"prompt");
        assert!(!prompt.alternate_screen);
        assert!(prompt.post_alternate_screen_exit);
        assert!(prompt.requires_immediate_render());

        assert_eq!(state.apply_bytes(b"ordinary"), TerminalChanges::default());

        let mut combined = TerminalState::new(20, 3, 100);
        combined.apply_bytes(b"\x1b[?1049halternate");
        let exit_and_prompt = combined.apply_bytes(b"\x1b[?1049lprompt");
        assert!(exit_and_prompt.alternate_screen);
        assert!(exit_and_prompt.post_alternate_screen_exit);
        assert_eq!(
            combined.apply_bytes(b"ordinary"),
            TerminalChanges::default()
        );

        let mut cursor_restore = TerminalState::new(20, 3, 100);
        cursor_restore.apply_bytes(b"\x1b[?1049halternate");
        let restore_only = cursor_restore.apply_bytes(b"\x1b[?1049l\x1b[?25h");
        assert!(restore_only.alternate_screen);
        assert!(!restore_only.post_alternate_screen_exit);
        assert!(
            cursor_restore
                .apply_bytes(b"prompt")
                .post_alternate_screen_exit
        );
    }

    #[test]
    fn alternate_screen_control_chars_update_alternate_screen() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"primary");
        state.apply_bytes(b"\x1b[?1049habc\rZ");

        assert_eq!(state.capture_screen_text(), "Zbc\n");

        state.apply_bytes(b"\x1b[H\x1b[2Jab\x08Z");
        assert_eq!(state.capture_screen_text(), "aZ\n");

        state.apply_bytes(b"\x1b[?1049l");
        assert_eq!(state.capture_screen_text(), "primary\n");
    }

    #[test]
    fn erase_display_does_not_move_cursor() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"abc");
        state.apply_bytes(b"\x1b[2JZ");

        assert_eq!(state.capture_screen_text(), "   Z\n");
    }

    #[test]
    fn erase_line_modes_match_terminal_semantics() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"abcdef");
        state.apply_bytes(b"\x1b[1D\x1b[1KZ");
        assert_eq!(state.capture_screen_text(), "     Z\n");

        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"abcdef");
        state.apply_bytes(b"\x1b[2D\x1b[2KZ");
        assert_eq!(state.capture_screen_text(), "    Z\n");
    }

    #[test]
    fn erase_to_cursor_handles_post_last_column_cursor() {
        let mut state = TerminalState::new(3, 2, 100);
        state.apply_bytes(b"abc\x1b[1K");
        assert_eq!(state.capture_screen_text(), "");

        let mut state = TerminalState::new(3, 2, 100);
        state.apply_bytes(b"abc\x1b[1J");
        assert_eq!(state.capture_screen_text(), "");
    }

    #[test]
    fn save_and_restore_cursor_with_escape_and_csi_sequences() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"ab\x1b7cd\x1b8Z");
        assert_eq!(state.capture_screen_text(), "abZd\n");

        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"ab\x1b[scd\x1b[uZ");
        assert_eq!(state.capture_screen_text(), "abZd\n");
    }

    #[test]
    fn cursor_visibility_tracks_private_mode() {
        let mut state = TerminalState::new(20, 3, 100);
        assert!(state.cursor_visible());

        state.apply_bytes(b"\x1b[?25l");
        assert!(!state.cursor_visible());

        state.apply_bytes(b"\x1b[?25h");
        assert!(state.cursor_visible());
    }

    #[test]
    fn batched_private_modes_update_cursor_visibility_and_alternate_screen() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"primary");

        state.apply_bytes(b"\x1b[?1000;25l");
        assert!(!state.cursor_visible());

        state.apply_bytes(b"\x1b[?25;1049halternate");
        assert!(state.cursor_visible());
        assert_eq!(state.capture_screen_text(), "alternate\n");

        state.apply_bytes(b"\x1b[?1049;25l");
        assert!(!state.cursor_visible());
        assert_eq!(state.capture_screen_text(), "primary\n");
    }

    #[test]
    fn utf8_sequences_print_complete_codepoints() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes("한글 ✓".as_bytes());

        assert_eq!(state.capture_screen_text(), "한글 ✓\n");
    }

    #[test]
    fn styled_render_lines_count_wide_characters_by_cell_width() {
        let mut state = TerminalState::new(10, 2, 100);
        state.apply_bytes("한글".as_bytes());

        assert_eq!(state.render_screen_ansi_lines(4, 1), vec!["한글"]);
        assert_eq!(state.render_screen_ansi_lines(3, 1), vec!["한 "]);
        assert_eq!(state.render_screen_ansi_lines(5, 1), vec!["한글 "]);
    }

    #[test]
    fn scrollback_keeps_lines_that_leave_the_screen() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"1\n2\n3\n4\n");
        let captured = state.capture_text();
        assert!(captured.contains("1"), "{captured:?}");
        assert!(captured.contains("4"), "{captured:?}");
    }

    #[test]
    fn capture_screen_excludes_scrollback() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree\r\nfour");
        let screen = state.capture_screen_text();
        assert_eq!(screen, "two\nthree\nfour\n");
        assert!(!screen.contains("one"), "{screen:?}");
        assert!(screen.contains("four"), "{screen:?}");
    }

    #[test]
    fn capture_history_excludes_current_screen() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree\r\nfour");
        let history = state.capture_history_text();
        assert_eq!(history, "one\n");
        assert!(history.contains("one"), "{history:?}");
        assert!(!history.contains("four"), "{history:?}");
    }

    #[test]
    fn line_feed_scrolls_only_the_active_scroll_region() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");

        state.apply_bytes(b"\x1b[2;4r\x1b[4;1H\nnew");

        assert_screen_rows(&state, &["header", "body-2", "body-3", "new", "footer"]);
        assert_eq!(state.capture_history_text(), "");
    }

    #[test]
    fn reverse_index_scrolls_only_the_active_scroll_region() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");

        state.apply_bytes(b"\x1b[2;4r\x1b[2;1H\x1bMnew");

        assert_screen_rows(&state, &["header", "new", "body-1", "body-2", "footer"]);
        assert_eq!(state.capture_history_text(), "");
    }

    #[test]
    fn delete_line_shifts_rows_inside_scroll_region_only() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");

        state.apply_bytes(b"\x1b[2;4r\x1b[2;1H\x1b[M\x1b[4;1Hnew");

        assert_screen_rows(&state, &["header", "body-2", "body-3", "new", "footer"]);
        assert_eq!(state.capture_history_text(), "");
    }

    #[test]
    fn delete_line_does_not_transfer_removed_rows_to_scrollback() {
        let mut state = TerminalState::new(12, 4, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree");

        state.apply_bytes(b"\x1b[1;1H\x1b[M");

        assert_screen_rows(&state, &["two", "three", "", ""]);
        assert_eq!(state.capture_history_text(), "");
    }

    #[test]
    fn insert_line_shifts_rows_inside_scroll_region_only() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");

        state.apply_bytes(b"\x1b[2;4r\x1b[2;1H\x1b[Lnew");

        assert_screen_rows(&state, &["header", "new", "body-1", "body-2", "footer"]);
        assert_eq!(state.capture_history_text(), "");
    }

    #[test]
    fn insert_and_delete_line_reset_cursor_to_first_column() {
        let mut state = TerminalState::new(12, 4, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree");
        state.apply_bytes(b"\x1b[2;4H\x1b[M");
        assert_eq!(state.cursor_position(), (1, 0));

        let mut state = TerminalState::new(12, 4, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree");
        state.apply_bytes(b"\x1b[2;4H\x1b[L");
        assert_eq!(state.cursor_position(), (1, 0));
    }

    #[test]
    fn scroll_up_and_down_operate_inside_scroll_region_only() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");
        state.apply_bytes(b"\x1b[2;4r\x1b[S\x1b[4;1Hnew");
        assert_screen_rows(&state, &["header", "body-2", "body-3", "new", "footer"]);

        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"header\r\nbody-1\r\nbody-2\r\nbody-3\r\nfooter");
        state.apply_bytes(b"\x1b[2;4r\x1b[T\x1b[2;1Hnew");
        assert_screen_rows(&state, &["header", "new", "body-1", "body-2", "footer"]);
    }

    #[test]
    fn scroll_region_accepts_defaults_and_ignores_invalid_margins() {
        let mut state = TerminalState::new(12, 5, 100);

        state.apply_bytes(b"\x1b[2;4r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (1, 3)
        );

        state.apply_bytes(b"\x1b[r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (0, 4)
        );

        state.apply_bytes(b"\x1b[0;0r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (0, 4)
        );

        state.apply_bytes(b"\x1b[;4r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (0, 3)
        );

        state.apply_bytes(b"\x1b[2r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (1, 4)
        );

        state.apply_bytes(b"\x1b[4;2r");
        assert_eq!(
            (
                state.active_screen().scroll_top,
                state.active_screen().scroll_bottom,
            ),
            (1, 4)
        );
    }

    #[test]
    fn cursor_vertical_moves_clamp_inside_scroll_region() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"\x1b[2;4r\x1b[2;1H\x1b[A");
        assert_eq!(state.cursor_position(), (1, 0));

        state.apply_bytes(b"\x1b[4;1H\x1b[B");
        assert_eq!(state.cursor_position(), (3, 0));
    }

    #[test]
    fn origin_mode_addresses_rows_relative_to_scroll_region() {
        let mut state = TerminalState::new(12, 5, 100);

        state.apply_bytes(b"\x1b[2;4r\x1b[?6h\x1b[1;1Hinside");
        assert_screen_rows(&state, &["", "inside", "", "", ""]);

        state.apply_bytes(b"\x1b[?6l\x1b[1;1Hhome");
        assert_screen_rows(&state, &["home", "inside", "", "", ""]);
    }

    #[test]
    fn alternate_screen_scroll_region_does_not_pollute_scrollback() {
        let mut state = TerminalState::new(12, 5, 100);
        state.apply_bytes(b"primary");

        state.apply_bytes(b"\x1b[?1049halt-1\r\nalt-2\r\nalt-3");
        state.apply_bytes(b"\x1b[1;3r\x1b[3;1H\nnew");

        assert_screen_rows(&state, &["alt-2", "alt-3", "new", "", ""]);
        assert_eq!(state.capture_history_text(), "");

        state.apply_bytes(b"\x1b[?1049l");
        assert_eq!(state.capture_screen_text(), "primary\n");
    }

    #[test]
    fn resize_changes_wrap_width_for_future_output() {
        let mut state = TerminalState::new(5, 3, 100);
        state.apply_bytes(b"abcde");
        state.resize(3, 3);
        state.apply_bytes(b"XYZ");

        let captured = state.capture_text();
        assert!(captured.contains("abc"), "{captured:?}");
        assert!(captured.contains("XYZ"), "{captured:?}");
    }

    #[test]
    fn resize_shrink_preserves_rows_near_cursor() {
        let mut state = TerminalState::new(10, 4, 100);
        state.apply_bytes(b"one\r\ntwo\r\nthree\r\nfour");
        state.resize(10, 2);

        assert_eq!(state.capture_screen_text(), "three\nfour\n");
    }

    fn assert_screen_rows(state: &TerminalState, expected: &[&str]) {
        for (row, expected) in expected.iter().enumerate() {
            assert_eq!(
                state.active_screen().row_to_string(row),
                *expected,
                "row {row}"
            );
        }
    }
}
