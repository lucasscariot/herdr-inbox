//! Composer geometry shared by drawing and mouse hit testing. The task stays
//! near the top, the preset strip right under it; decorative content never
//! moves the input or its choices.

use ratatui::layout::Rect;

use super::{Chip, ChipView, Composer, Context, Field};

pub struct ComposerLayout {
    pub content: Rect,
    pub task_box: Rect,
    pub task_inner: Rect,
    pub task_lines: Vec<String>,
    pub task_scroll: u16,
    pub task_cursor: (u16, u16),
    /// The preset strip's rows, label included.
    pub strip: Rect,
    /// Every chip that fits, flowing left to right and wrapping.
    pub chips: Vec<(ChipView, Rect)>,
    pub fields: Vec<(Field, Rect)>,
    pub send: Rect,
    pub details_y: u16,
    pub picker: Option<PickerLayout>,
}

pub struct PickerLayout {
    pub popup: Rect,
    pub query: Rect,
    pub rows: Rect,
    pub start: usize,
}

impl ComposerLayout {
    pub const LABEL_WIDTH: u16 = 11;
    /// Room kept for a row's key hint, `F7  ›`.
    const KEY_WIDTH: u16 = 7;
    /// Rows under the strip that stay with the fields: a blank line and the
    /// seven field rows.
    const RESERVED_ROWS: u16 = 8;

    pub fn new(area: Rect, composer: &Composer, ctx: &Context) -> Option<Self> {
        if area.width < 20 || area.height < 8 {
            return None;
        }
        let content = Rect::new(area.x + 2, area.y + 1, area.width - 4, area.height - 2);
        let inner_width = content.width.saturating_sub(4);
        let (task_lines, (row, col)) = composer.task.layout(inner_width);
        // Keep the fields and send action visible when a pasted task is long.
        let rows =
            (task_lines.len() as u16).clamp(3, 8).min(area.height.saturating_sub(16).max(2)).min(area.height - 6);
        let task_box = Rect::new(content.x, area.y + 4, content.width, rows + 2);
        let task_inner = Rect::new(task_box.x + 2, task_box.y + 1, inner_width, rows);
        let task_scroll = row.saturating_sub(rows - 1);
        let task_cursor = (task_inner.x + col.min(inner_width), task_inner.y + row - task_scroll);
        let limit = area.bottom().saturating_sub(2);
        // The strip: chips flow after the label and wrap under it, the first
        // row leaving room for the key hint on the right. It never grows
        // into the rows the fields need: chips past that are left out.
        let strip_end = limit.saturating_sub(Self::RESERVED_ROWS).max(task_box.bottom() + 1).min(limit);
        let mut y = task_box.bottom();
        // A chip's own padding takes the place of the space before a value.
        let chips_x = content.x + 1 + Self::LABEL_WIDTH;
        let mut chips = Vec::new();
        let mut x = chips_x;
        let mut strip_rows = 0;
        for chip in composer.chips(ctx) {
            let right =
                if y == task_box.bottom() { content.right().saturating_sub(Self::KEY_WIDTH) } else { content.right() };
            let room = right.saturating_sub(chips_x);
            if room == 0 || y >= limit {
                break;
            }
            let width = chip.width().min(room);
            if x + width > right && x > chips_x {
                y += 1;
                x = chips_x;
                if y >= strip_end {
                    break;
                }
            }
            chips.push((chip, Rect::new(x, y, width, 1)));
            x += width + 1;
            strip_rows = (y - task_box.bottom()) + 1;
        }
        let strip = Rect::new(content.x, task_box.bottom(), content.width, strip_rows);
        let mut y = strip.bottom() + 1;
        let mut fields = Vec::new();
        // Show unsupported thinking explicitly rather than making a row vanish.
        for field in Field::ORDER.into_iter().filter(|f| !matches!(f, Field::Task | Field::Preset)) {
            if y >= limit {
                break;
            }
            fields.push((field, Rect::new(content.x, y, content.width, 1)));
            y += 1;
        }
        let send = if y + 1 < area.bottom() { Rect::new(content.x, y + 1, 8, 1) } else { Rect::default() };
        let picker = composer.picker.as_ref().map(|picker| {
            let anchor = match picker.field {
                Field::Preset => strip.bottom(),
                field => fields.iter().find(|(f, _)| *f == field).map(|(_, r)| r.bottom()).unwrap_or(content.y + 1),
            };
            let choices = composer.choices_with_actions(ctx, picker.field, &picker.query);
            let height = (choices.len().clamp(1, 10) as u16 + 3).min(area.height - 2);
            let width = content.width.saturating_sub(Self::LABEL_WIDTH).max(20).min(content.width);
            let popup = Rect::new(content.right() - width, anchor.min(area.bottom() - height), width, height);
            let query = Rect::new(popup.x + 2, popup.y + 1, popup.width - 4, 1);
            let rows = Rect::new(popup.x + 1, popup.y + 2, popup.width - 2, height.saturating_sub(3));
            let start = picker.selected.saturating_sub(rows.height.saturating_sub(1) as usize);
            PickerLayout { popup, query, rows, start }
        });
        Some(Self {
            content,
            task_box,
            task_inner,
            task_lines,
            task_scroll,
            task_cursor,
            strip,
            chips,
            fields,
            send,
            details_y: y + 3,
            picker,
        })
    }

    pub fn field_at(&self, x: u16, y: u16) -> Option<Field> {
        if contains(self.strip, x, y) {
            return Some(Field::Preset);
        }
        self.fields.iter().find(|(_, rect)| contains(*rect, x, y)).map(|(field, _)| *field)
    }

    /// The chip under a point in the strip.
    pub fn chip_at(&self, x: u16, y: u16) -> Option<Chip> {
        self.chips.iter().find(|(_, rect)| contains(*rect, x, y)).map(|(chip, _)| chip.chip)
    }
}

pub fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    #[test]
    fn every_drawn_control_and_picker_stays_inside_the_terminal_area() {
        for width in (0..150).step_by(7) {
            for height in 0..45 {
                let mut app = App::new(width, height, vec![]);
                app.composer.task.set(&"日本語 and a long wrapped task\n".repeat(12));
                for field in Field::ORDER {
                    app.composer.open_picker(field, "a long query");
                    let area = app.layout.terminal;
                    let Some(layout) = ComposerLayout::new(area, &app.composer, &app.composer_context()) else {
                        continue;
                    };
                    let mut rects = vec![layout.content, layout.task_box, layout.task_inner, layout.send];
                    rects.extend(layout.fields.iter().map(|(_, rect)| *rect));
                    rects.push(layout.strip);
                    rects.extend(layout.chips.iter().map(|(_, rect)| *rect));
                    if let Some(picker) = layout.picker {
                        rects.extend([picker.popup, picker.query, picker.rows]);
                    }
                    for rect in rects.into_iter().filter(|r| r.height > 0) {
                        assert!(contains(area, rect.x, rect.y), "{rect:?} outside {area:?}");
                        assert!(
                            rect.right() <= area.right() && rect.bottom() <= area.bottom(),
                            "{rect:?} outside {area:?}"
                        );
                    }
                    assert!(contains(area, layout.task_cursor.0, layout.task_cursor.1));
                }
            }
        }
    }

    #[test]
    fn a_crowded_strip_leaves_the_fields_and_send_their_rows() {
        let presets = (0..14)
            .map(|i| crate::presets::Preset {
                name: format!("A rather long preset name {i}"),
                harness: "claude".into(),
                model: "opus".into(),
                thinking: String::new(),
            })
            .collect();
        let mut app = App::new(100, 24, vec![]).with_memory(presets, vec![]);
        app.composer.task.set(&"A long task with several lines\n".repeat(12));
        let layout = ComposerLayout::new(app.layout.terminal, &app.composer, &app.composer_context()).unwrap();
        assert!(layout.chips.len() < 15, "not every chip fits in 24 rows");
        assert!(layout.strip.height >= 1);
        assert!(layout.fields.iter().any(|(f, _)| *f == Field::Workspace), "{:?}", layout.fields);
        assert!(layout.send.height > 0, "send stays reachable");
        assert_eq!(layout.fields[0].1.y, layout.strip.bottom() + 1);
    }
}
