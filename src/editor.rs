//! A small multi-line text editor for the composer's task: a string and a
//! cursor, plus word-wrapped layout with the cursor's screen position.

use unicode_width::UnicodeWidthChar;

/// The longest task the composer accepts, like the legacy plugin.
pub const MAX_LEN: usize = 32_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    text: String,
    /// Byte offset, always on a character boundary.
    cursor: usize,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.chars().take(MAX_LEN).collect();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Inserts text at the cursor; carriage returns become newlines and the
    /// task never grows past [`MAX_LEN`] characters.
    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let room = MAX_LEN.saturating_sub(self.text.chars().count());
        let text: String = text.chars().filter(|c| *c == '\n' || !c.is_control()).take(room).collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn backspace(&mut self) {
        if let Some(previous) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= previous.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some(previous) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= previous.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.text[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// Moves to the same column of the previous line. Returns false on the
    /// first line, so the caller can use Up for something else.
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let column = self.text[start..self.cursor].chars().count();
        let previous_start = self.text[..start - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.cursor = advance(&self.text, previous_start, column, start - 1);
        true
    }

    /// Moves to the same column of the next line. Returns false on the last.
    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let column = self.text[self.line_start()..self.cursor].chars().count();
        let next_start = end + 1;
        let next_end = self.text[next_start..].find('\n').map(|i| next_start + i).unwrap_or(self.text.len());
        self.cursor = advance(&self.text, next_start, column, next_end);
        true
    }

    /// Deletes the word before the cursor, and the spaces after it.
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches([' ', '\t']);
        let start = trimmed.rfind([' ', '\t', '\n']).map(|i| i + 1).unwrap_or(0);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Places the cursor at a displayed cell, using the same wrapping as drawing.
    /// A click in a wide character lands before it; past a line lands at its end.
    pub fn place_cursor(&mut self, width: u16, row: u16, column: u16) {
        let lines = wrapped_lines(&self.text, width.max(1) as usize);
        let Some((line, start)) = lines.get(row as usize) else {
            self.cursor = self.text.len();
            return;
        };
        let mut used = 0;
        self.cursor = *start;
        for ch in line.chars() {
            let cells = ch.width().unwrap_or(0) as u16;
            if used + cells > column {
                break;
            }
            used += cells;
            self.cursor += ch.len_utf8();
        }
    }

    /// Word-wrapped lines for a box `width` columns wide, and the cursor's
    /// `(row, column)` in them.
    pub fn layout(&self, width: u16) -> (Vec<String>, (u16, u16)) {
        layout(&self.text, self.cursor, width.max(1) as usize)
    }
}

/// The byte offset `columns` characters after `from`, stopping at `limit`.
fn advance(text: &str, from: usize, columns: usize, limit: usize) -> usize {
    text[from..limit].char_indices().nth(columns).map(|(i, _)| from + i).unwrap_or(limit)
}

fn layout(text: &str, cursor: usize, width: usize) -> (Vec<String>, (u16, u16)) {
    let mut lines = Vec::new();
    let mut position = (0u16, 0u16);
    for (line, start) in wrapped_lines(text, width) {
        let end = start + line.len();
        // A cursor at a soft wrap belongs to the next row, but a newline or
        // the end of the text keeps it on this one.
        if cursor >= start && (cursor < end || (cursor == end && (end == text.len() || text[end..].starts_with('\n'))))
        {
            let column: usize = text[start..cursor].chars().map(|c| c.width().unwrap_or(0)).sum();
            position = (lines.len() as u16, column as u16);
        }
        lines.push(line);
    }
    (lines, position)
}

fn wrapped_lines(text: &str, width: usize) -> Vec<(String, usize)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for logical in text.split('\n') {
        lines.extend(wrap(logical, width).into_iter().map(|(line, start)| (line, offset + start)));
        offset += logical.len() + 1;
    }
    lines
}

/// Breaks one logical line into rows, preferring spaces. Returns each row
/// with its byte offset in the line.
fn wrap(line: &str, width: usize) -> Vec<(String, usize)> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let rest = &line[start..];
        let mut used = 0;
        let mut end = start;
        let mut last_space = None;
        for (index, ch) in rest.char_indices() {
            let w = ch.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            used += w;
            end = start + index + ch.len_utf8();
            if ch == ' ' {
                last_space = Some(end);
            }
        }
        if end == line.len() {
            rows.push((rest.to_string(), start));
            break;
        }
        // Break after the last space when there is one, else mid-word.
        let cut =
            last_space.filter(|&s| s > start).unwrap_or(end.max(start + rest.chars().next().map_or(1, char::len_utf8)));
        rows.push((line[start..cut].to_string(), start));
        start = cut;
    }
    if rows.is_empty() {
        rows.push((String::new(), 0));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    #[test]
    fn typing_and_deleting_move_the_cursor_by_characters() {
        let mut e = Editor::default();
        e.insert("héllo");
        assert_eq!((e.text(), e.cursor()), ("héllo", 6));
        e.left();
        e.left();
        e.insert("X");
        assert_eq!(e.text(), "hélXlo");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "hélo");
        e.home();
        e.delete();
        assert_eq!(e.text(), "élo");
        e.end();
        e.delete();
        assert_eq!(e.text(), "élo", "delete at the end does nothing");
        e.home();
        e.backspace();
        e.left();
        assert_eq!(e.cursor(), 0, "nothing before the start");
    }

    #[test]
    fn pasted_newlines_are_normalized_and_controls_dropped() {
        let mut e = Editor::default();
        e.insert("a\r\nb\rc\u{7}d\te");
        assert_eq!(e.text(), "a\nb\ncde", "tabs and bells are control characters");
    }

    #[test]
    fn the_task_is_bounded() {
        let mut e = editor(&"x".repeat(MAX_LEN + 10));
        assert_eq!(e.text().len(), MAX_LEN);
        e.insert("more");
        assert_eq!(e.text().len(), MAX_LEN);
    }

    #[test]
    fn up_and_down_keep_the_column_and_report_the_edges() {
        let mut e = editor("first line\nab\nthird line");
        e.home();
        assert_eq!(e.cursor(), "first line\nab\n".len());
        e.right();
        e.right();
        e.right();
        assert!(e.up());
        assert_eq!(e.cursor(), "first line\nab".len(), "clamped to the shorter line");
        assert!(e.up());
        assert_eq!(e.cursor(), 2);
        assert!(!e.up(), "already on the first line");
        assert!(e.down());
        assert!(e.down());
        assert_eq!(e.cursor(), "first line\nab\nth".len());
        assert!(!e.down(), "already on the last line");
    }

    #[test]
    fn home_and_end_stay_on_the_current_line() {
        let mut e = editor("one\ntwo");
        e.left();
        e.home();
        assert_eq!(e.cursor(), 4);
        e.end();
        assert_eq!(e.cursor(), 7);
    }

    #[test]
    fn delete_word_removes_the_previous_word_and_trailing_spaces() {
        let mut e = editor("fix the login  ");
        e.delete_word();
        assert_eq!(e.text(), "fix the ");
        e.delete_word();
        e.delete_word();
        assert_eq!(e.text(), "");
        e.delete_word();
        let mut e = editor("line one\nsecond");
        e.delete_word();
        assert_eq!(e.text(), "line one\n");
    }

    #[test]
    fn layout_wraps_at_spaces_and_places_the_cursor() {
        let e = editor("fix the login redirect loop");
        let (lines, cursor) = e.layout(10);
        assert_eq!(lines, ["fix the ", "login ", "redirect ", "loop"]);
        assert_eq!(cursor, (3, 4), "after the last character");
    }

    #[test]
    fn layout_breaks_long_words_and_keeps_empty_lines() {
        let mut e = editor("abcdefghij\n\nend");
        let (lines, _) = e.layout(4);
        assert_eq!(lines, ["abcd", "efgh", "ij", "", "end"]);
        e.up();
        let (_, cursor) = e.layout(4);
        assert_eq!(cursor, (3, 0), "the empty line holds the cursor");
    }

    #[test]
    fn layout_cursor_at_a_wrap_point_starts_the_next_row() {
        let mut e = editor("abcd efgh");
        for _ in 0..4 {
            e.left();
        }
        let (lines, cursor) = e.layout(5);
        assert_eq!(lines, ["abcd ", "efgh"]);
        assert_eq!(cursor, (1, 0));
    }

    #[test]
    fn layout_measures_wide_characters() {
        let e = editor("日本語です");
        let (lines, cursor) = e.layout(4);
        assert_eq!(lines, ["日本", "語で", "す"]);
        assert_eq!(cursor, (2, 2));
    }

    #[test]
    fn an_empty_editor_has_one_empty_row() {
        let (lines, cursor) = Editor::default().layout(20);
        assert_eq!(lines, [""]);
        assert_eq!(cursor, (0, 0));
        assert!(Editor::default().is_blank());
        assert!(editor(" \n ").is_blank());
    }

    #[test]
    fn clicks_follow_wrapping_newlines_and_unicode_cell_widths() {
        let mut e = editor("fix the login\n日本語\n\ne\u{301}nd");
        e.place_cursor(8, 1, 3);
        assert_eq!(e.cursor(), "fix the log".len());
        e.place_cursor(8, 2, 3);
        assert_eq!(e.cursor(), "fix the login\n日".len(), "inside a wide cell lands before it");
        e.place_cursor(8, 3, 5);
        assert_eq!(e.cursor(), "fix the login\n日本語\n".len(), "blank line");
        e.place_cursor(8, 4, 1);
        assert_eq!(e.cursor(), "fix the login\n日本語\n\ne\u{301}".len());
        e.place_cursor(8, 50, 50);
        assert_eq!(e.cursor(), e.text().len());
    }

    #[test]
    fn a_zero_width_box_does_not_loop_forever() {
        let (lines, _) = editor("abc").layout(0);
        assert_eq!(lines, ["a", "b", "c"]);
    }
}
