//! A pane's screen, rebuilt from Herdr's ANSI frames and drawn into a
//! ratatui buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::herdr::terminal::Frame;
use crate::keys::Modes;

thread_local! {
    static RECOVERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the current thread is inside the emulator, where a panic is caught
/// and the screen reset. The panic hook uses it to keep the UI on screen.
pub fn recovering() -> bool {
    RECOVERING.with(std::cell::Cell::get)
}

/// vt100 underflows on a wrap in a screen narrower or shorter than two cells.
const MIN_SIZE: u16 = 2;

fn parser(cols: u16, rows: u16) -> vt100::Parser {
    vt100::Parser::new(rows.max(MIN_SIZE), cols.max(MIN_SIZE), 0)
}

pub struct Screen {
    parser: vt100::Parser,
    seq: Option<u64>,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { parser: parser(cols, rows), seq: None }
    }

    /// Applies one frame. A full frame starts from a blank screen so no state
    /// from a previous stream survives; a diff frame updates the current one.
    pub fn apply(&mut self, frame: &Frame) {
        let (cols, rows) = (frame.width.max(MIN_SIZE), frame.height.max(MIN_SIZE));
        if frame.full {
            self.parser = parser(cols, rows);
        } else if self.size() != (cols, rows) {
            self.parser.screen_mut().set_size(rows, cols);
        }
        let emulator = &mut self.parser;
        RECOVERING.with(|flag| flag.set(true));
        let processed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| emulator.process(&frame.bytes)));
        RECOVERING.with(|flag| flag.set(false));
        if processed.is_err() {
            // A frame the emulator cannot handle must not take the inbox down;
            // the next full frame repaints the screen.
            self.parser = parser(cols, rows);
        }
        self.seq = Some(frame.seq);
    }

    /// Whether a frame has arrived since the screen was created.
    pub fn has_content(&self) -> bool {
        self.seq.is_some()
    }

    /// `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (cols, rows)
    }

    pub fn modes(&self) -> Modes {
        let screen = self.parser.screen();
        Modes { application_cursor: screen.application_cursor(), bracketed_paste: screen.bracketed_paste() }
    }

    /// Whether the application asked for mouse events.
    pub fn wants_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// `(col, row)` of a visible cursor.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }
        let (row, col) = screen.cursor_position();
        Some((col, row))
    }

    pub fn text(&self) -> String {
        self.parser.screen().contents()
    }

    /// Draws the screen's top-left corner into `area`. Cells outside the
    /// screen are left as they are.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        for row in 0..rows.min(area.height) {
            for col in 0..cols.min(area.width) {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let Some(target) = buf.cell_mut((area.x + col, area.y + row)) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    // ratatui's convention: the cell under the second half of
                    // a wide character is reset, and its diff skips it.
                    target.reset();
                    continue;
                }
                if cell.has_contents() {
                    target.set_symbol(cell.contents());
                } else {
                    target.set_symbol(" ");
                }
                target.set_style(style(cell));
            }
        }
    }
}

fn color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => match index {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            other => Color::Indexed(other),
        },
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn style(cell: &vt100::Cell) -> Style {
    let mut modifier = Modifier::empty();
    if cell.bold() {
        modifier |= Modifier::BOLD;
    }
    if cell.dim() {
        modifier |= Modifier::DIM;
    }
    if cell.italic() {
        modifier |= Modifier::ITALIC;
    }
    if cell.underline() {
        modifier |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        modifier |= Modifier::REVERSED;
    }
    Style::new().fg(color(cell.fgcolor())).bg(color(cell.bgcolor())).add_modifier(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64, full: bool, w: u16, h: u16, bytes: &[u8]) -> Frame {
        Frame { seq, width: w, height: h, full, bytes: bytes.to_vec() }
    }

    fn line(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    #[test]
    fn a_new_screen_is_empty_until_the_first_frame() {
        let mut screen = Screen::new(10, 2);
        assert!(!screen.has_content());
        screen.apply(&frame(1, true, 10, 2, b"hi"));
        assert!(screen.has_content());
        assert_eq!(screen.text().trim_end(), "hi");
    }

    #[test]
    fn a_full_frame_resets_size_and_state_and_a_diff_updates_in_place() {
        let mut screen = Screen::new(80, 24);
        screen.apply(&frame(1, true, 20, 3, b"\x1b[?1h\x1b[1;1Hold text"));
        assert_eq!(screen.size(), (20, 3));
        assert!(screen.modes().application_cursor);
        screen.apply(&frame(2, false, 20, 3, b"\x1b[1;1HNEW"));
        assert_eq!(screen.text().lines().next(), Some("NEW text"));
        screen.apply(&frame(3, true, 30, 4, b"fresh"));
        assert_eq!(screen.size(), (30, 4));
        assert!(!screen.modes().application_cursor, "a full frame drops old modes");
        assert_eq!(screen.text().trim_end(), "fresh");
    }

    #[test]
    fn a_diff_with_a_new_size_resizes_without_losing_content() {
        let mut screen = Screen::new(10, 2);
        screen.apply(&frame(1, true, 10, 2, b"abc"));
        screen.apply(&frame(2, false, 12, 3, b""));
        assert_eq!(screen.size(), (12, 3));
        assert!(screen.text().starts_with("abc"));
    }

    #[test]
    fn modes_come_from_the_application() {
        let mut screen = Screen::new(10, 2);
        screen.apply(&frame(1, true, 10, 2, b"\x1b[?2004h\x1b[?1000h"));
        assert_eq!(screen.modes(), Modes { application_cursor: false, bracketed_paste: true });
        assert!(screen.wants_mouse());
        screen.apply(&frame(2, false, 10, 2, b"\x1b[?2004l\x1b[?1000l"));
        assert!(!screen.modes().bracketed_paste);
        assert!(!screen.wants_mouse());
    }

    #[test]
    fn the_cursor_is_reported_only_when_visible() {
        let mut screen = Screen::new(10, 3);
        screen.apply(&frame(1, true, 10, 3, b"\x1b[2;4H"));
        assert_eq!(screen.cursor(), Some((3, 1)));
        screen.apply(&frame(2, false, 10, 3, b"\x1b[?25l"));
        assert_eq!(screen.cursor(), None);
    }

    #[test]
    fn render_copies_text_colors_and_attributes() {
        let mut screen = Screen::new(6, 1);
        screen.apply(&frame(1, true, 6, 1, b"\x1b[1;38;2;215;119;87;48;5;236mAB\x1b[0;3;4;7mC"));
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        screen.render(buf.area, &mut buf);
        assert_eq!(line(&buf, 0), "ABC   ");
        let a = &buf[(0, 0)];
        assert_eq!(a.fg, Color::Rgb(215, 119, 87));
        assert_eq!(a.bg, Color::Indexed(236));
        assert!(a.modifier.contains(Modifier::BOLD));
        let c = &buf[(2, 0)];
        assert_eq!(c.fg, Color::Reset);
        assert!(c.modifier.contains(Modifier::ITALIC | Modifier::UNDERLINED | Modifier::REVERSED));
        assert!(!c.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn the_sixteen_ansi_colors_map_to_named_colors() {
        let mut screen = Screen::new(4, 1);
        screen.apply(&frame(1, true, 4, 1, b"\x1b[31ma\x1b[92mb\x1b[47mc\x1b[39;49md"));
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        screen.render(buf.area, &mut buf);
        assert_eq!(buf[(0, 0)].fg, Color::Red);
        assert_eq!(buf[(1, 0)].fg, Color::LightGreen);
        assert_eq!(buf[(2, 0)].bg, Color::Gray);
        assert_eq!(buf[(3, 0)].fg, Color::Reset);
        assert_eq!(buf[(3, 0)].bg, Color::Reset);
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let mut screen = Screen::new(4, 1);
        screen.apply(&frame(1, true, 4, 1, "界x".as_bytes()));
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        screen.render(buf.area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "界");
        assert_eq!(buf[(1, 0)].symbol(), " ");
        assert_eq!(buf[(2, 0)].symbol(), "x");
    }

    #[test]
    fn render_clips_to_the_area_and_honours_its_offset() {
        let mut screen = Screen::new(5, 2);
        screen.apply(&frame(1, true, 5, 2, b"12345\r\nabcde"));
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 2));
        screen.render(Rect::new(2, 1, 3, 1), &mut buf);
        assert_eq!(line(&buf, 0), "      ");
        assert_eq!(line(&buf, 1), "  123 ");
    }

    #[test]
    fn a_zero_sized_frame_never_panics() {
        let mut screen = Screen::new(0, 0);
        screen.apply(&frame(1, true, 0, 0, b"x"));
        screen.apply(&frame(2, false, 0, 0, "y界z界".as_bytes()));
        screen.apply(&frame(3, true, 1, 1, "界界界\r\n\r\n".as_bytes()));
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        screen.render(buf.area, &mut buf);
    }
}
