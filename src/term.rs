use std::collections::VecDeque;

pub struct TerminalState {
    screen: TerminalScreen,
    alternate_screen: Option<TerminalScreen>,
    use_alternate_screen: bool,
    scrollback: Scrollback,
    parser: ParserState,
    style: CellStyle,
}

impl TerminalState {
    pub fn new(width: usize, height: usize, max_scrollback_lines: usize) -> Self {
        Self {
            screen: TerminalScreen::new(width, height),
            alternate_screen: None,
            use_alternate_screen: false,
            scrollback: Scrollback::new(max_scrollback_lines),
            parser: ParserState::Ground,
            style: CellStyle::default(),
        }
    }

    pub fn apply_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.apply_byte(*byte);
        }
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

    pub fn resize(&mut self, width: usize, height: usize) {
        self.screen.resize(width, height);
        if let Some(screen) = &mut self.alternate_screen {
            screen.resize(width, height);
        }
    }

    fn apply_byte(&mut self, byte: u8) {
        match &mut self.parser {
            ParserState::Ground => match byte {
                b'\x1b' => self.parser = ParserState::Escape,
                b'\r' => self.active_screen_mut().carriage_return(),
                b'\n' => self.line_feed(),
                b'\t' => self.tab(),
                b'\x08' => self.active_screen_mut().backspace(),
                0x20..=0x7e => self.put_char(byte as char),
                _ => {}
            },
            ParserState::Escape => match byte {
                b'[' => self.parser = ParserState::Csi(Vec::new()),
                b']' => self.parser = ParserState::Osc { saw_escape: false },
                _ => self.parser = ParserState::Ground,
            },
            ParserState::Csi(buffer) => {
                if (0x40..=0x7e).contains(&byte) {
                    let params = String::from_utf8_lossy(buffer).to_string();
                    self.apply_csi(&params, byte);
                    self.parser = ParserState::Ground;
                } else {
                    buffer.push(byte);
                }
            }
            ParserState::Osc { saw_escape } => {
                if *saw_escape {
                    if byte == b'\\' {
                        self.parser = ParserState::Ground;
                    } else {
                        *saw_escape = byte == b'\x1b';
                    }
                } else if byte == b'\x07' {
                    self.parser = ParserState::Ground;
                } else if byte == b'\x1b' {
                    *saw_escape = true;
                }
            }
        }
    }

    fn line_feed(&mut self) {
        if self.use_alternate_screen {
            self.active_screen_mut().line_feed_without_scrollback();
        } else {
            self.screen.line_feed(&mut self.scrollback);
        }
    }

    fn tab(&mut self) {
        let next_tab = ((self.active_screen().cursor_col / 8) + 1) * 8;
        while self.active_screen().cursor_col < next_tab {
            self.put_char(' ');
        }
    }

    fn apply_csi(&mut self, params: &str, final_byte: u8) {
        match final_byte {
            b'm' => self.apply_sgr(params),
            b'J' if params == "2" || params.is_empty() => self.active_screen_mut().clear(),
            b'K' => self.active_screen_mut().clear_line_from_cursor(),
            b'H' | b'f' => {
                let (row, col) = parse_cursor_position(params);
                self.active_screen_mut().move_cursor(row, col);
            }
            b'A' => self.active_screen_mut().move_up(parse_count(params)),
            b'B' => self.active_screen_mut().move_down(parse_count(params)),
            b'C' => self.active_screen_mut().move_right(parse_count(params)),
            b'D' => self.active_screen_mut().move_left(parse_count(params)),
            b'h' if params == "?1049" => self.enter_alternate_screen(),
            b'l' if params == "?1049" => self.exit_alternate_screen(),
            _ => {}
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
        self.alternate_screen = Some(TerminalScreen::new(self.screen.width, self.screen.height));
        self.use_alternate_screen = true;
    }

    fn exit_alternate_screen(&mut self) {
        self.use_alternate_screen = false;
    }

    fn apply_sgr(&mut self, params: &str) {
        let params = parse_sgr_params(params);
        let mut index = 0;
        while index < params.len() {
            match params[index] {
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
                30..=37 => self.style.fg = Some(Color::Ansi(params[index] as u8 - 30)),
                39 => self.style.fg = None,
                40..=47 => self.style.bg = Some(Color::Ansi(params[index] as u8 - 40)),
                49 => self.style.bg = None,
                90..=97 => self.style.fg = Some(Color::Ansi(params[index] as u8 - 90 + 8)),
                100..=107 => self.style.bg = Some(Color::Ansi(params[index] as u8 - 100 + 8)),
                38 | 48 => {
                    if let Some((color, consumed)) = parse_extended_color(&params[index + 1..]) {
                        if params[index] == 38 {
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
        }
    }

    fn put_char(&mut self, ch: char, style: CellStyle, scrollback: &mut Scrollback) {
        if self.cursor_col >= self.width {
            self.cursor_col = 0;
            self.line_feed(scrollback);
        }

        self.rows[self.cursor_row][self.cursor_col] = Cell { ch, style };
        self.cursor_col += 1;
    }

    fn put_char_without_scrollback(&mut self, ch: char, style: CellStyle) {
        if self.cursor_col >= self.width {
            self.cursor_col = 0;
            self.line_feed_without_scrollback();
        }

        self.rows[self.cursor_row][self.cursor_col] = Cell { ch, style };
        self.cursor_col += 1;
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    fn line_feed(&mut self, scrollback: &mut Scrollback) {
        if self.cursor_row + 1 >= self.height {
            let top = self.row_to_string(0);
            scrollback.push(top);
            self.rows.remove(0);
            self.rows.push(vec![Cell::blank(); self.width]);
        } else {
            self.cursor_row += 1;
        }
    }

    fn line_feed_without_scrollback(&mut self) {
        if self.cursor_row + 1 >= self.height {
            self.rows.remove(0);
            self.rows.push(vec![Cell::blank(); self.width]);
        } else {
            self.cursor_row += 1;
        }
    }

    fn backspace(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    fn clear(&mut self) {
        self.rows = vec![vec![Cell::blank(); self.width]; self.height];
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn resize(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let height = height.max(1);
        let old_cursor_col = self.cursor_col;
        let mut rows = vec![vec![Cell::blank(); width]; height];
        let copy_rows = self.height.min(height);
        let copy_cols = self.width.min(width);

        for (row_index, row) in rows.iter_mut().enumerate().take(copy_rows) {
            row[..copy_cols].copy_from_slice(&self.rows[row_index][..copy_cols]);
        }

        self.width = width;
        self.height = height;
        self.rows = rows;
        self.cursor_row = self.cursor_row.min(self.height - 1);
        if old_cursor_col >= self.width {
            self.cursor_col = 0;
            self.cursor_row = (self.cursor_row + 1).min(self.height - 1);
        } else {
            self.cursor_col = self.cursor_col.min(self.width - 1);
        }
    }

    fn clear_line_from_cursor(&mut self) {
        for col in self.cursor_col..self.width {
            self.rows[self.cursor_row][col] = Cell::blank();
        }
    }

    fn move_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.saturating_sub(1).min(self.height - 1);
        self.cursor_col = col.saturating_sub(1).min(self.width - 1);
    }

    fn move_up(&mut self, count: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(count);
    }

    fn move_down(&mut self, count: usize) {
        self.cursor_row = (self.cursor_row + count).min(self.height - 1);
    }

    fn move_right(&mut self, count: usize) {
        self.cursor_col = (self.cursor_col + count).min(self.width - 1);
    }

    fn move_left(&mut self, count: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(count);
    }

    fn non_empty_lines(&self) -> Vec<String> {
        let mut lines = self
            .rows
            .iter()
            .map(|row| trim_trailing_spaces(row.iter().map(|cell| cell.ch).collect::<String>()))
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
        let cells = row.into_iter().flat_map(|row| row.iter()).take(width);
        let mut visible_width = 0;

        for cell in cells {
            if cell.style != current_style {
                output.push_str(&style_transition_from(current_style, cell.style));
                current_style = cell.style;
            }
            if cell.style != CellStyle::default() {
                styled_text_emitted = true;
            }
            output.push(cell.ch);
            visible_width += 1;
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
        self.rows
            .iter()
            .rposition(|row| row.iter().any(|cell| cell.ch != ' '))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: CellStyle,
}

impl Cell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            style: CellStyle::default(),
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
}

enum ParserState {
    Ground,
    Escape,
    Csi(Vec<u8>),
    Osc { saw_escape: bool },
}

fn parse_count(params: &str) -> usize {
    params
        .split(';')
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn parse_cursor_position(params: &str) -> (usize, usize) {
    let mut parts = params.split(';');
    let row = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let col = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    (row, col)
}

fn parse_sgr_params(params: &str) -> Vec<usize> {
    if params.is_empty() {
        return vec![0];
    }

    params
        .split(';')
        .map(|part| {
            if part.is_empty() {
                0
            } else {
                part.parse::<usize>().unwrap_or(0)
            }
        })
        .collect()
}

fn parse_extended_color(params: &[usize]) -> Option<(Color, usize)> {
    match params {
        [5, value, ..] if *value <= u8::MAX as usize => Some((Color::Indexed(*value as u8), 2)),
        [2, red, green, blue, ..]
            if *red <= u8::MAX as usize
                && *green <= u8::MAX as usize
                && *blue <= u8::MAX as usize =>
        {
            Some((Color::Rgb(*red as u8, *green as u8, *blue as u8), 4))
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn last_non_blank_cell(row: &[Cell]) -> Option<usize> {
    row.iter().rposition(|cell| cell.ch != ' ')
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
    fn alternate_screen_control_chars_update_alternate_screen() {
        let mut state = TerminalState::new(20, 3, 100);
        state.apply_bytes(b"primary");
        state.apply_bytes(b"\x1b[?1049habc\rZ");

        assert_eq!(state.capture_screen_text(), "Zbc\n");

        state.apply_bytes(b"\x1b[2Jab\x08Z");
        assert_eq!(state.capture_screen_text(), "aZ\n");

        state.apply_bytes(b"\x1b[?1049l");
        assert_eq!(state.capture_screen_text(), "primary\n");
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
    fn resize_changes_wrap_width_for_future_output() {
        let mut state = TerminalState::new(5, 3, 100);
        state.apply_bytes(b"abcde");
        state.resize(3, 3);
        state.apply_bytes(b"XYZ");

        let captured = state.capture_text();
        assert!(captured.contains("abc"), "{captured:?}");
        assert!(captured.contains("XYZ"), "{captured:?}");
    }
}
