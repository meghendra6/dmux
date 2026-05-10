use std::collections::VecDeque;

pub struct TerminalState {
    screen: TerminalScreen,
    scrollback: Scrollback,
    parser: ParserState,
}

impl TerminalState {
    pub fn new(width: usize, height: usize, max_scrollback_lines: usize) -> Self {
        Self {
            screen: TerminalScreen::new(width, height),
            scrollback: Scrollback::new(max_scrollback_lines),
            parser: ParserState::Ground,
        }
    }

    pub fn apply_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.apply_byte(*byte);
        }
    }

    pub fn capture_text(&self) -> String {
        let mut lines = self.scrollback.lines.iter().cloned().collect::<Vec<_>>();
        lines.extend(self.screen.non_empty_lines());

        if lines.is_empty() {
            String::new()
        } else {
            let mut text = lines.join("\n");
            text.push('\n');
            text
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.screen.resize(width, height);
    }

    fn apply_byte(&mut self, byte: u8) {
        match &mut self.parser {
            ParserState::Ground => match byte {
                b'\x1b' => self.parser = ParserState::Escape,
                b'\r' => self.screen.carriage_return(),
                b'\n' => self.line_feed(),
                b'\t' => self.tab(),
                b'\x08' => self.screen.backspace(),
                0x20..=0x7e => self.screen.put_char(byte as char, &mut self.scrollback),
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
        self.screen.line_feed(&mut self.scrollback);
    }

    fn tab(&mut self) {
        let next_tab = ((self.screen.cursor_col / 8) + 1) * 8;
        while self.screen.cursor_col < next_tab {
            self.screen.put_char(' ', &mut self.scrollback);
        }
    }

    fn apply_csi(&mut self, params: &str, final_byte: u8) {
        match final_byte {
            b'm' => {}
            b'J' if params == "2" || params.is_empty() => self.screen.clear(),
            b'K' => self.screen.clear_line_from_cursor(),
            b'H' | b'f' => {
                let (row, col) = parse_cursor_position(params);
                self.screen.move_cursor(row, col);
            }
            b'A' => self.screen.move_up(parse_count(params)),
            b'B' => self.screen.move_down(parse_count(params)),
            b'C' => self.screen.move_right(parse_count(params)),
            b'D' => self.screen.move_left(parse_count(params)),
            _ => {}
        }
    }
}

struct TerminalScreen {
    width: usize,
    height: usize,
    rows: Vec<Vec<char>>,
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
            rows: vec![vec![' '; width]; height],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    fn put_char(&mut self, ch: char, scrollback: &mut Scrollback) {
        if self.cursor_col >= self.width {
            self.cursor_col = 0;
            self.line_feed(scrollback);
        }

        self.rows[self.cursor_row][self.cursor_col] = ch;
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
            self.rows.push(vec![' '; self.width]);
        } else {
            self.cursor_row += 1;
        }
    }

    fn backspace(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    fn clear(&mut self) {
        self.rows = vec![vec![' '; self.width]; self.height];
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn resize(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let height = height.max(1);
        let old_cursor_col = self.cursor_col;
        let mut rows = vec![vec![' '; width]; height];
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
            self.rows[self.cursor_row][col] = ' ';
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
            .map(|row| trim_trailing_spaces(row.iter().collect::<String>()))
            .collect::<Vec<_>>();

        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }

        lines
    }

    fn row_to_string(&self, row: usize) -> String {
        trim_trailing_spaces(self.rows[row].iter().collect::<String>())
    }
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
    fn scrollback_keeps_lines_that_leave_the_screen() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"1\n2\n3\n4\n");
        let captured = state.capture_text();
        assert!(captured.contains("1"), "{captured:?}");
        assert!(captured.contains("4"), "{captured:?}");
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
