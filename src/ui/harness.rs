//! One small glyph per harness, in its own colour, next to its readable name.
//! Ordinary single-width characters: no images, patched font or block art.

use ratatui::style::{Color, Style};

use crate::theme::Palette;

/// The glyph and colour that stand for a harness kind. Unknown harnesses get
/// a neutral prompt chevron and keep their name for recognition.
pub(super) fn mark(kind: &str, palette: &Palette) -> (&'static str, Color) {
    match kind.to_ascii_lowercase().as_str() {
        "claude" | "claude code" => ("✻", palette.peach),
        "codex" => ("◆", palette.teal),
        "pi" => ("π", palette.mauve),
        "opencode" | "open code" => ("◈", palette.blue),
        _ => ("❯", palette.overlay0),
    }
}

/// The mark ready to draw, followed by a space.
pub(super) fn label(kind: &str, palette: &Palette) -> (String, Style) {
    let (glyph, color) = mark(kind, palette);
    (format!("{glyph} "), Style::new().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn the_four_marks_are_distinct_single_width_and_theme_colored() {
        let palette = Palette::default();
        let kinds =
            [("claude", palette.peach), ("codex", palette.teal), ("pi", palette.mauve), ("opencode", palette.blue)];
        let mut glyphs = Vec::new();
        for (kind, color) in kinds {
            let (glyph, got) = mark(kind, &palette);
            assert_eq!(glyph.width(), 1, "{kind}'s mark must take one cell");
            assert_eq!(got, color);
            assert!(!glyphs.contains(&glyph), "{kind} needs its own recognizable mark");
            glyphs.push(glyph);
        }
        assert_eq!(mark("Claude Code", &palette), mark("claude", &palette));
        assert_eq!(mark("OpenCode", &palette), mark("opencode", &palette));
        let (generic, color) = mark("new-agent", &palette);
        assert_eq!(generic.width(), 1);
        assert_eq!(color, palette.overlay0);
        assert!(!glyphs.contains(&generic));
        assert_eq!(label("pi", &palette).0.width(), 2);
    }
}
