//! Small logo-inspired pixel marks, made from ordinary half-block cells.
//! Five columns by three rows, with no images or patched font required.

use ratatui::style::Style;

use crate::theme::Palette;

pub(super) const WIDTH: u16 = 5;

pub(super) fn row(kind: &str, line: u16, palette: &Palette) -> (String, Style) {
    let (pixels, color) = match kind.to_ascii_lowercase().as_str() {
        "claude" | "claude code" => ([0x04, 0x15, 0x0e, 0x1f, 0x15, 0x04], palette.peach),
        "codex" => ([0x0e, 0x11, 0x17, 0x1d, 0x11, 0x0e], palette.teal),
        "pi" => ([0x00, 0x1f, 0x0a, 0x0a, 0x0a, 0x0b], palette.mauve),
        "opencode" | "open code" => ([0x1f, 0x11, 0x15, 0x13, 0x11, 0x1f], palette.blue),
        _ => ([0x08, 0x04, 0x02, 0x04, 0x08, 0x03], palette.overlay0),
    };
    let mut text = String::new();
    if line < 3 {
        let top = pixels[(line * 2) as usize];
        let bottom = pixels[(line * 2 + 1) as usize];
        for column in 0..WIDTH {
            // One horizontal pixel and two vertical pixels per cell.
            let mask = 0x10 >> column;
            text.push(match (top & mask != 0, bottom & mask != 0) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
    }
    (text, Style::new().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn the_four_marks_are_distinct_fixed_width_and_theme_colored() {
        let palette = Palette::default();
        let kinds =
            [("claude", palette.peach), ("codex", palette.teal), ("pi", palette.mauve), ("opencode", palette.blue)];
        let mut marks = Vec::new();
        for (kind, color) in kinds {
            let mut mark = String::new();
            for line in 0..3 {
                let (text, style) = row(kind, line, &palette);
                assert_eq!(text.width(), WIDTH as usize);
                assert_eq!(style.fg, Some(color));
                mark.push_str(&text);
            }
            assert!(!marks.contains(&mark), "{kind} needs its own recognizable mark");
            marks.push(mark);
        }
        assert_eq!(row("Claude Code", 0, &palette), row("claude", 0, &palette));
        assert_eq!(row("OpenCode", 0, &palette), row("opencode", 0, &palette));
        assert_eq!(row("new-agent", 1, &palette).0.width(), WIDTH as usize);
        assert!(row("pi", 3, &palette).0.is_empty());
    }
}
