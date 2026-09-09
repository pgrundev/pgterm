//! A minimal multi-line text editor for the SQL tab. Deliberately small: the
//! established ratatui editor crate still targets ratatui 0.29, and a query
//! box needs insert, delete, and cursor movement — not a modal editor.

/// Text plus a cursor, addressed in characters (not bytes), so multi-byte
/// input behaves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    lines: Vec<String>,
    /// Cursor line and column, both in characters.
    pub row: usize,
    pub col: usize,
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    pub fn from_text(text: &str) -> Self {
        let mut e = Editor {
            lines: text.split('\n').map(str::to_string).collect(),
            row: 0,
            col: 0,
        };
        if e.lines.is_empty() {
            e.lines.push(String::new());
        }
        e.row = e.lines.len() - 1;
        e.col = e.lines[e.row].chars().count();
        e
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map(|l| l.chars().count()).unwrap_or(0)
    }

    fn byte_at(&self, row: usize, col: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[row].len())
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte_at(self.row, self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.newline();
            } else if !c.is_control() {
                self.insert(c);
            }
        }
    }

    pub fn newline(&mut self) {
        let at = self.byte_at(self.row, self.col);
        let rest = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let at = self.byte_at(self.row, self.col - 1);
            self.lines[self.row].remove(at);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.line_len(self.row) {
            let at = self.byte_at(self.row, self.col);
            self.lines[self.row].remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len(self.row);
        }
    }

    pub fn right(&mut self) {
        if self.col < self.line_len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.line_len(self.row));
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.line_len(self.row));
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.line_len(self.row);
    }

    pub fn clear(&mut self) {
        *self = Editor::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Editor {
        let mut e = Editor::new();
        e.insert_str(s);
        e
    }

    #[test]
    fn typing_and_newlines_build_the_text() {
        let e = typed("SELECT 1\nFROM t");
        assert_eq!(e.text(), "SELECT 1\nFROM t");
        assert_eq!(e.lines().len(), 2);
        assert_eq!((e.row, e.col), (1, 6));
        assert!(!e.is_empty());
        assert!(Editor::new().is_empty());
        assert!(typed("  \n \t ").is_empty());
    }

    #[test]
    fn backspace_joins_lines_and_stops_at_the_start() {
        let mut e = typed("ab\ncd");
        e.home();
        e.backspace();
        assert_eq!(e.text(), "abcd");
        assert_eq!((e.row, e.col), (0, 2));
        // Backspace deletes what is BEFORE the cursor, so "cd" survives.
        for _ in 0..10 {
            e.backspace();
        }
        assert_eq!(e.text(), "cd");
        assert_eq!((e.row, e.col), (0, 0));
    }

    #[test]
    fn delete_pulls_the_next_line_up() {
        let mut e = typed("ab\ncd");
        e.up();
        e.end();
        e.delete();
        assert_eq!(e.text(), "abcd");
        e.home();
        e.delete();
        assert_eq!(e.text(), "bcd");
    }

    #[test]
    fn movement_wraps_between_lines_and_clamps_columns() {
        let mut e = typed("long line\nx");
        e.up();
        e.end();
        assert_eq!((e.row, e.col), (0, 9));
        e.down();
        assert_eq!((e.row, e.col), (1, 1), "column clamps to the shorter line");
        e.right();
        assert_eq!((e.row, e.col), (1, 1), "right at the end stays put");
        e.home();
        e.left();
        assert_eq!((e.row, e.col), (0, 9), "left at the start wraps up");
    }

    #[test]
    fn multibyte_text_is_addressed_by_character() {
        let mut e = typed("héllo");
        assert_eq!(e.col, 5, "five characters, not six bytes");
        e.left();
        e.backspace();
        assert_eq!(e.text(), "hélo", "the char before the cursor, not a byte");
        let mut e = typed("日本語");
        e.home();
        e.right();
        e.insert('x');
        assert_eq!(e.text(), "日x本語");
    }

    #[test]
    fn pasted_control_characters_never_reach_the_buffer() {
        let mut e = Editor::new();
        e.insert_str("SELECT\t1\r\nFROM t\x07");
        assert_eq!(e.text(), "SELECT1\nFROM t", "tabs, CR and BEL are dropped");
    }

    #[test]
    fn from_text_puts_the_cursor_at_the_end() {
        let e = Editor::from_text("a\nbb");
        assert_eq!((e.row, e.col), (1, 2));
        assert_eq!(Editor::from_text("").text(), "");
    }
}
