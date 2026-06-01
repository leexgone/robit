//! Input editor — handles text editing, cursor movement, and history.

pub struct InputEditor {
    content: String,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
    pub multi_line: bool,
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
            draft: String::new(),
            multi_line: false,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    #[allow(dead_code)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Take the current content and clear the editor. Returns None if empty.
    pub fn take(&mut self) -> Option<String> {
        let content = self.content.trim().to_string();
        if content.is_empty() {
            return None;
        }
        self.history.push(content.clone());
        self.content.clear();
        self.cursor = 0;
        self.history_index = None;
        self.draft.clear();
        Some(content)
    }

    pub fn insert_char(&mut self, c: char) {
        self.content.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.content[..self.cursor]
                .chars()
                .next_back()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
            self.content.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.content.len() {
            self.content.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.content[..self.cursor]
                .chars()
                .next_back()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.content.len() {
            let next = self.content[self.cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor += next;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.content.len();
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_index {
            Some(i) => {
                if i > 0 {
                    i - 1
                } else {
                    return;
                }
            }
            None => {
                self.draft = self.content.clone();
                self.history.len() - 1
            }
        };
        self.history_index = Some(idx);
        self.content = self.history[idx].clone();
        self.cursor = self.content.len();
    }

    pub fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.content = self.history[new_idx].clone();
                self.cursor = self.content.len();
            } else {
                self.history_index = None;
                self.content = self.draft.clone();
                self.cursor = self.content.len();
            }
        }
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor = 0;
    }

    /// Compute the visual cursor column (offset within the current line).
    pub fn cursor_col(&self) -> u16 {
        use unicode_width::UnicodeWidthStr;
        let line_start = self.content[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        UnicodeWidthStr::width(&self.content[line_start..self.cursor]) as u16
    }

    /// Compute the visual cursor row (0-based line number within the input).
    pub fn cursor_row(&self) -> u16 {
        self.content[..self.cursor].chars().filter(|&c| c == '\n').count() as u16
    }

    /// Count total lines in the input.
    pub fn line_count(&self) -> usize {
        if self.content.is_empty() {
            1
        } else {
            self.content.chars().filter(|&c| c == '\n').count() + 1
        }
    }
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}
