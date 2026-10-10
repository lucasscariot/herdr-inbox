//! Screen geometry and the sidebar's rows. Computed after every update so the
//! renderer only draws, and mouse clicks map back to the rows that were drawn.

use ratatui::layout::Rect;

use crate::threads::{Group, Thread};

/// Lines a thread takes in the sidebar, plus one blank line after it.
pub const THREAD_LINES: u16 = 3;
/// Lines above the list: title, summary, New thread, search.
pub const HEADER_LINES: u16 = 4;
/// The header row holding the New thread button.
pub const NEW_BUTTON_ROW: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Heading {
        group: Group,
        count: usize,
    },
    /// Line `line` (0..THREAD_LINES) of the thread at `index`.
    Thread {
        index: usize,
        line: u16,
    },
    Blank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub width: u16,
    pub height: u16,
    pub sidebar: Rect,
    /// The New thread button in the sidebar header.
    pub new_button: Rect,
    /// The always-visible filter entry.
    pub search: Rect,
    /// The sidebar's scrolling part, below its header.
    pub list: Rect,
    pub terminal: Rect,
    pub bar: Rect,
    /// Every sidebar row, scrolled or not.
    pub rows: Vec<Row>,
    /// Index of the first row shown in `list`.
    pub offset: usize,
    cursor_index: Option<usize>,
    following_cursor: bool,
}

impl Layout {
    pub fn new(width: u16, height: u16) -> Self {
        let mut layout = Self {
            width: 0,
            height: 0,
            sidebar: Rect::default(),
            new_button: Rect::default(),
            search: Rect::default(),
            list: Rect::default(),
            terminal: Rect::default(),
            bar: Rect::default(),
            rows: Vec::new(),
            offset: 0,
            cursor_index: None,
            following_cursor: true,
        };
        layout.resize(width, height);
        layout
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        let body = height.saturating_sub(1);
        let sidebar_width = sidebar_width(width);
        self.sidebar = Rect::new(0, 0, sidebar_width, body);
        self.new_button =
            if body > NEW_BUTTON_ROW { Rect::new(0, NEW_BUTTON_ROW, sidebar_width, 1) } else { Rect::default() };
        self.search = if body > 3 { Rect::new(0, 3, sidebar_width, 1) } else { Rect::default() };
        self.list = Rect::new(0, HEADER_LINES.min(body), sidebar_width, body.saturating_sub(HEADER_LINES));
        // One column separates the sidebar from the terminal.
        let terminal_x = (sidebar_width + 1).min(width);
        self.terminal = Rect::new(terminal_x, 0, width.saturating_sub(terminal_x), body);
        self.bar = Rect::new(0, body, width, height.min(1));
        self.clamp_offset();
    }

    /// The size to request for a pane: never zero, which Herdr rejects.
    pub fn terminal_size(&self) -> (u16, u16) {
        (self.terminal.width.max(1), self.terminal.height.max(1))
    }

    pub fn cursor_index(&self) -> Option<usize> {
        self.cursor_index
    }

    /// Rebuilds rows, following the cursor only until the user scrolls manually.
    pub fn update(&mut self, threads: &[Thread], cursor: Option<&str>) {
        self.rows = rows(threads);
        self.cursor_index = cursor.and_then(|id| threads.iter().position(|t| t.id == id));
        if self.following_cursor
            && let Some(index) = self.cursor_index
        {
            let thread_first =
                self.rows.iter().position(|row| row.kind == RowKind::Thread { index, line: 0 }).unwrap_or(0);
            // Include the heading without losing the thread's last line.
            let first = match thread_first.checked_sub(1).map(|i| self.rows[i].kind) {
                Some(RowKind::Heading { .. }) => thread_first - 1,
                _ => thread_first,
            };
            let last = thread_first + THREAD_LINES as usize;
            let visible = self.list.height as usize;
            if first < self.offset {
                self.offset = first;
            } else if last > self.offset + visible {
                self.offset = last.saturating_sub(visible);
            }
        }
        self.clamp_offset();
    }

    fn clamp_offset(&mut self) {
        let max = self.rows.len().saturating_sub(self.list.height as usize);
        self.offset = self.offset.min(max);
    }

    /// The thread drawn at a screen position, if any.
    pub fn thread_at(&self, x: u16, y: u16) -> Option<usize> {
        if !contains(self.list, x, y) {
            return None;
        }
        let row = self.offset + (y - self.list.y) as usize;
        match self.rows.get(row)?.kind {
            RowKind::Thread { index, .. } => Some(index),
            _ => None,
        }
    }

    pub fn in_terminal(&self, x: u16, y: u16) -> bool {
        contains(self.terminal, x, y)
    }

    pub fn in_list(&self, x: u16, y: u16) -> bool {
        contains(self.list, x, y)
    }

    pub fn on_new_button(&self, x: u16, y: u16) -> bool {
        contains(self.new_button, x, y)
    }

    /// Keyboard navigation resumes following the selected thread.
    pub fn follow_cursor(&mut self) {
        self.following_cursor = true;
    }

    /// Scrolls the list by `delta` rows, within bounds, independently of focus.
    pub fn scroll(&mut self, delta: isize) {
        self.following_cursor = false;
        self.offset = self.offset.saturating_add_signed(delta);
        self.clamp_offset();
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// A third of the width, kept between 28 and 42 columns, and never more than
/// half the screen so the terminal stays usable.
fn sidebar_width(width: u16) -> u16 {
    (width / 3).clamp(28, 42).min(width / 2).max(width.min(20))
}

/// Headings precede each non-empty group; each thread is followed by a blank
/// line, except the last one of the list.
fn rows(threads: &[Thread]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut index = 0;
    for group in Group::ALL {
        let count = threads.iter().filter(|t| t.group() == group).count();
        if count == 0 {
            continue;
        }
        rows.push(Row { kind: RowKind::Heading { group, count } });
        for _ in 0..count {
            for line in 0..THREAD_LINES {
                rows.push(Row { kind: RowKind::Thread { index, line } });
            }
            rows.push(Row { kind: RowKind::Blank });
            index += 1;
        }
    }
    if rows.last().is_some_and(|row| row.kind == RowKind::Blank) {
        rows.pop();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::types::AgentStatus;

    fn thread(id: &str, status: AgentStatus) -> Thread {
        Thread {
            id: id.into(),
            machine_id: crate::threads::LOCAL.into(),
            machine_label: None,
            pane_id: id.into(),
            workspace_id: "w".into(),
            status,
            title: id.into(),
            project: "p".into(),
            branch: None,
            harness: "Claude".into(),
            kind: None,
            note: None,
            changed_at: None,
            change_seq: 0,
        }
    }

    fn sorted(threads: Vec<Thread>) -> Vec<Thread> {
        let mut threads = threads;
        crate::threads::sort(&mut threads);
        threads
    }

    #[test]
    fn geometry_splits_sidebar_separator_terminal_and_bar() {
        let layout = Layout::new(120, 40);
        assert_eq!(layout.sidebar, Rect::new(0, 0, 40, 39));
        assert_eq!(layout.list, Rect::new(0, 4, 40, 35));
        assert_eq!(layout.new_button, Rect::new(0, 2, 40, 1));
        assert_eq!(layout.terminal, Rect::new(41, 0, 79, 39));
        assert_eq!(layout.bar, Rect::new(0, 39, 120, 1));
        assert_eq!(layout.terminal_size(), (79, 39));
    }

    #[test]
    fn sidebar_width_is_bounded() {
        assert_eq!(sidebar_width(300), 42);
        assert_eq!(sidebar_width(90), 30);
        assert_eq!(sidebar_width(60), 28);
        assert_eq!(sidebar_width(40), 20);
        assert_eq!(sidebar_width(10), 10);
        assert_eq!(sidebar_width(0), 0);
    }

    #[test]
    fn tiny_and_empty_screens_never_underflow() {
        for (w, h) in [(0, 0), (1, 1), (5, 2), (30, 3)] {
            let mut layout = Layout::new(w, h);
            layout.update(&[thread("a", AgentStatus::Idle)], Some("a"));
            let (cols, rows) = layout.terminal_size();
            assert!(cols >= 1 && rows >= 1);
            for x in 0..=w {
                for y in 0..=h {
                    let _ = layout.thread_at(x, y);
                }
            }
        }
    }

    #[test]
    fn rows_have_a_heading_per_group_and_no_trailing_blank() {
        let threads = sorted(vec![
            thread("a", AgentStatus::Working),
            thread("b", AgentStatus::Blocked),
            thread("c", AgentStatus::Working),
        ]);
        let kinds: Vec<RowKind> = rows(&threads).into_iter().map(|r| r.kind).collect();
        use RowKind::*;
        assert_eq!(
            kinds,
            vec![
                Heading { group: Group::NeedsInput, count: 1 },
                Thread { index: 0, line: 0 },
                Thread { index: 0, line: 1 },
                Thread { index: 0, line: 2 },
                Blank,
                Heading { group: Group::Working, count: 2 },
                Thread { index: 1, line: 0 },
                Thread { index: 1, line: 1 },
                Thread { index: 1, line: 2 },
                Blank,
                Thread { index: 2, line: 0 },
                Thread { index: 2, line: 1 },
                Thread { index: 2, line: 2 },
            ]
        );
        assert!(rows(&[]).is_empty());
    }

    #[test]
    fn clicks_map_to_the_thread_drawn_there() {
        let threads = sorted(vec![thread("a", AgentStatus::Blocked), thread("b", AgentStatus::Idle)]);
        let mut layout = Layout::new(100, 30);
        layout.update(&threads, None);
        let top = layout.list.y;
        assert_eq!(layout.thread_at(5, top), None, "heading");
        assert_eq!(layout.thread_at(5, top + 1), Some(0));
        assert_eq!(layout.thread_at(5, top + 3), Some(0));
        assert_eq!(layout.thread_at(5, top + 4), None, "blank");
        assert_eq!(layout.thread_at(5, top + 6), Some(1));
        assert_eq!(layout.thread_at(layout.sidebar.width, top + 1), None, "outside the sidebar");
        assert_eq!(layout.thread_at(5, 0), None, "header");
    }

    #[test]
    fn the_list_scrolls_to_keep_the_cursor_visible_and_back() {
        let threads: Vec<Thread> = (0..10).map(|i| thread(&format!("t{i}"), AgentStatus::Idle)).collect();
        let threads = sorted(threads);
        // 1 heading + 10 threads * 4 lines - 1 trailing blank = 40 rows; 11 visible.
        let mut layout = Layout::new(100, 16);
        assert_eq!(layout.list.height, 11);
        layout.update(&threads, Some(&threads[0].id));
        assert_eq!(layout.offset, 0);
        layout.update(&threads, Some(&threads[5].id));
        // Thread 5 spans rows 21..24, so the offset puts row 23 at the bottom.
        assert_eq!(layout.offset, 24 - 11);
        assert_eq!(layout.thread_at(1, layout.list.y + layout.list.height - 1), Some(5));
        layout.update(&threads, Some(&threads[2].id));
        assert_eq!(layout.offset, 9, "scrolling up aligns the thread's first line to the top");
        layout.update(&threads, Some(&threads[0].id));
        assert_eq!(layout.offset, 0, "the first thread brings its heading back");
    }

    #[test]
    fn a_heading_does_not_push_the_selected_threads_last_line_off_screen() {
        let mut layout = Layout::new(100, 9);
        layout.update(&[thread("a", AgentStatus::Working)], Some("a"));
        assert_eq!(layout.offset, 0);
        layout.resize(100, 8);
        layout.update(&[thread("a", AgentStatus::Working)], Some("a"));
        assert_eq!(layout.offset, 1);
        let last = layout.offset + layout.list.height as usize - 1;
        assert_eq!(layout.rows[last].kind, RowKind::Thread { index: 0, line: 2 });
    }

    #[test]
    fn manual_scrolling_is_clamped() {
        let threads = sorted((0..10).map(|i| thread(&format!("t{i}"), AgentStatus::Idle)).collect());
        let mut layout = Layout::new(100, 16);
        layout.update(&threads, None);
        layout.scroll(-5);
        assert_eq!(layout.offset, 0);
        layout.scroll(1000);
        assert_eq!(layout.offset, 40 - 11);
        layout.resize(100, 100);
        assert_eq!(layout.offset, 0, "everything fits again");
    }
}
