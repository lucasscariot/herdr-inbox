//! The composer, drawn in place of the agent's terminal.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::UnicodeWidthStr;

use std::time::SystemTime;

use super::{clip, fill, frame, orbit_in, split_line};
use crate::app::{App, Field, LaunchState};
use crate::orbit;
use crate::theme::Palette;

/// Rows of the task box's text area, before it grows with the task.
const TASK_MIN_ROWS: u16 = 3;
const TASK_MAX_ROWS: u16 = 8;
const LABEL_WIDTH: u16 = 11;
/// The orbit above the task stays small enough to keep the task near the top.
const ORBIT_MAX_ROWS: u16 = 14;

/// Where things landed, for the cursor.
pub struct Drawn {
    pub cursor: Option<(u16, u16)>,
}

pub fn draw(buf: &mut Buffer, app: &App, area: Rect, palette: &Palette, now: SystemTime) -> Drawn {
    fill(buf, area, Style::new());
    if area.width < 20 || area.height < 8 {
        split_line(
            buf,
            area.x,
            area.y,
            area.width,
            &[("Window too small".into(), Style::new().fg(palette.overlay0))],
            &[],
        );
        return Drawn { cursor: None };
    }
    let x = area.x + 2;
    let width = area.width.saturating_sub(4);
    let dim = Style::new().fg(palette.overlay0);
    let y = area.y + 1;

    // Title and what discovery is doing.
    let scanning: Vec<String> =
        app.machines.iter().filter(|m| app.discovering.contains(&m.id)).map(|m| m.label.clone()).collect();
    let status = if scanning.is_empty() {
        vec![("esc close".into(), dim)]
    } else {
        vec![(format!("scanning {}…", scanning.join(", ")), dim)]
    };
    split_line(
        buf,
        x,
        y,
        width,
        &[("New thread".into(), Style::new().fg(palette.accent).add_modifier(Modifier::BOLD))],
        &status,
    );

    // The orbit sits between the title and the task, in whatever room the
    // rest leaves: lay the rest out once off screen to measure it.
    let top = y + 2;
    let mut scratch = Buffer::empty(area);
    let height = body(&mut scratch, app, area, top, palette).bottom - top;
    let spare = area.bottom().saturating_sub(top + height);
    // A blank row under the orbit, and one at the bottom of the screen.
    let top = match orbit::fit(area.width, spare.saturating_sub(2).min(ORBIT_MAX_ROWS)) {
        Some((_, rows)) => {
            orbit_in(buf, app, Rect::new(area.x, top, area.width, rows), palette, now);
            top + rows + 1
        }
        None => top,
    };
    body(buf, app, area, top, palette).drawn
}

/// The composer under its title, from row `y` down, and the row it ended on.
struct Body {
    drawn: Drawn,
    bottom: u16,
}

fn body(buf: &mut Buffer, app: &App, area: Rect, mut y: u16, palette: &Palette) -> Body {
    let ctx = app.composer_context();
    let composer = &app.composer;
    let x = area.x + 2;
    let width = area.width.saturating_sub(4);
    let dim = Style::new().fg(palette.overlay0);
    split_line(buf, x, y, width, &[("What should we build?".into(), Style::new().fg(palette.subtext0))], &[]);
    y += 1;

    // The task box grows with the task, within bounds.
    let inner_width = width.saturating_sub(4);
    let (lines, (cursor_row, cursor_col)) = composer.task.layout(inner_width);
    let rows = (lines.len() as u16).clamp(TASK_MIN_ROWS, TASK_MAX_ROWS);
    let focused = composer.field == Field::Task && composer.picker.is_none();
    let dictating = app.dictation.as_ref().is_some_and(|d| d.target == crate::app::Target::Composer);
    // Recording is the one state loud enough for a full-strength frame.
    let border = if dictating { Style::new().fg(palette.red) } else { frame(palette, focused) };
    let task_box = Rect::new(x, y, width, rows + 2);
    Block::new().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(border).render(task_box, buf);
    // Keep the cursor's row visible when the task is longer than the box.
    let scroll = cursor_row.saturating_sub(rows - 1);
    for (index, line) in lines.iter().skip(scroll as usize).take(rows as usize).enumerate() {
        buf.set_stringn(x + 2, y + 1 + index as u16, line, inner_width as usize, Style::new().fg(palette.text));
    }
    if composer.task.text().is_empty() {
        buf.set_stringn(
            x + 2,
            y + 1,
            "Describe the task. Enter sends, Shift+Enter adds a line.",
            inner_width as usize,
            dim,
        );
    }
    let mut cursor =
        (focused && !dictating).then_some((x + 2 + cursor_col.min(inner_width), y + 1 + cursor_row - scroll));
    y += rows + 2;
    if let Some(error) = &composer.error {
        split_line(buf, x, y, width, &[(error.clone(), Style::new().fg(palette.red))], &[]);
    }
    y += 1;

    // One row per field.
    let mut field_rows = Vec::new();
    for field in composer.fields(&ctx).into_iter().filter(|f| *f != Field::Task) {
        if y >= area.bottom() {
            break;
        }
        let active = composer.field == field;
        let row = Rect::new(x, y, width, 1);
        if active {
            fill(buf, row, Style::new().bg(palette.active_row_bg));
        }
        let marker = if active { "▸ " } else { "  " };
        let mut left = vec![
            (marker.to_string(), Style::new().fg(palette.accent)),
            (format!("{:<width$}", field.label(), width = LABEL_WIDTH as usize), Style::new().fg(palette.subtext0)),
            (composer.value(&ctx, field), Style::new().fg(palette.text).add_modifier(Modifier::BOLD)),
        ];
        if field == Field::Workspace
            && let Some(branch) = composer.branch_preview(&ctx)
        {
            left.push(("  ".into(), dim));
            left.push((format!("⎇ {branch}"), Style::new().fg(palette.mauve)));
        }
        let key = field.key().map(|n| format!("F{n}")).unwrap_or_default();
        split_line(buf, x, y, width, &left, &[(key, dim)]);
        field_rows.push((field, y));
        y += 1;
    }
    y += 1;
    if y < area.bottom() {
        let key = Style::new().fg(palette.subtext0).add_modifier(Modifier::BOLD);
        let dictate = if app.config.speech.space_hold == Some(false) { "⌃T" } else { "hold ␣" };
        let hints = [
            ("↵", "send"),
            ("⌃S", "send & keep"),
            (dictate, "dictate"),
            ("⇥", "fields"),
            ("F7", "presets"),
            ("F10", "voice"),
        ];
        let mut parts = Vec::new();
        for (index, (k, label)) in hints.iter().enumerate() {
            if index > 0 {
                parts.push(("   ".to_string(), dim));
            }
            parts.push((k.to_string(), key));
            parts.push((format!(" {label}"), dim));
        }
        split_line(buf, x, y, width, &parts, &[]);
        y += 2;
    }

    // Discovery problems, then the launches.
    for (machine, error) in &app.discovery_errors {
        if y >= area.bottom() {
            break;
        }
        let label = app.machine(machine).map(|m| m.label.clone()).unwrap_or_default();
        split_line(buf, x, y, width, &[(format!("✗ {label}: {error}"), Style::new().fg(palette.red))], &[]);
        y += 1;
    }
    if !app.launches.is_empty() && y + 1 < area.bottom() {
        split_line(buf, x, y, width, &[("LAUNCHES".into(), dim.add_modifier(Modifier::BOLD))], &[]);
        y += 1;
        for launch in app.launches.iter().rev() {
            if y >= area.bottom() {
                break;
            }
            let (mark, color, text) = match &launch.state {
                LaunchState::Running(text) => ("⟳", palette.yellow, text.clone()),
                LaunchState::Sent { unverified: false } => ("✓", palette.green, "sent".into()),
                LaunchState::Sent { unverified: true } => ("✓", palette.yellow, "sent, not confirmed yet".into()),
                LaunchState::Waiting => ("●", palette.yellow, "answer the agent's startup prompt".into()),
                LaunchState::Failed(error) => ("✗", palette.red, error.clone()),
            };
            let where_ = format!("{} · {}", launch.project, launch.harness);
            let left = [
                (format!("{mark} "), Style::new().fg(color)),
                (format!("{where_}  "), Style::new().fg(palette.subtext0)),
                (launch.title.clone(), Style::new().fg(palette.text)),
            ];
            let room = (width as usize / 2).max(10);
            split_line(buf, x, y, width, &left, &[(clip(&text, room), Style::new().fg(color))]);
            y += 1;
        }
    }

    // The picker floats over the fields, under its own row.
    if let Some(picker) = &composer.picker {
        let anchor = field_rows.iter().find(|(f, _)| *f == picker.field).map(|(_, y)| *y + 1).unwrap_or(area.y + 2);
        let choices = composer.choices_with_actions(&ctx, picker.field, &picker.query);
        let visible = choices.len().clamp(1, 10) as u16;
        let height = (visible + 3).min(area.bottom().saturating_sub(area.y + 1));
        let top = anchor.min(area.bottom().saturating_sub(height));
        let popup = Rect::new(x + LABEL_WIDTH, top, width.saturating_sub(LABEL_WIDTH).max(20).min(width), height);
        fill(buf, popup, Style::new().bg(palette.surface0));
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(palette.accent).bg(palette.surface0))
            .title(format!(" {} ", picker.field.label()))
            .render(popup, buf);
        let inner_x = popup.x + 2;
        let inner_w = popup.width.saturating_sub(4);
        let query = format!("› {}", picker.query);
        split_line(
            buf,
            inner_x,
            popup.y + 1,
            inner_w,
            &[(query.clone(), Style::new().fg(palette.text).bg(palette.surface0))],
            &[],
        );
        cursor = Some((inner_x + query.width() as u16, popup.y + 1));
        let rows = height.saturating_sub(3) as usize;
        let start = picker.selected.saturating_sub(rows.saturating_sub(1));
        for (index, choice) in choices.iter().enumerate().skip(start).take(rows) {
            let row_y = popup.y + 2 + (index - start) as u16;
            let selected = index == picker.selected;
            let bg = if selected { palette.selection_bg } else { palette.surface0 };
            fill(buf, Rect::new(popup.x + 1, row_y, popup.width.saturating_sub(2), 1), Style::new().bg(bg));
            let label_style =
                if choice.enabled { Style::new().fg(palette.text) } else { Style::new().fg(palette.overlay0) };
            split_line(
                buf,
                inner_x,
                row_y,
                inner_w,
                &[(choice.label.clone(), label_style.bg(bg))],
                &[(choice.detail.clone(), Style::new().fg(palette.overlay0).bg(bg))],
            );
        }
        if choices.is_empty() {
            split_line(
                buf,
                inner_x,
                popup.y + 2,
                inner_w,
                &[("Nothing matches".into(), dim.bg(palette.surface0))],
                &[],
            );
        }
    }
    Body { drawn: Drawn { cursor }, bottom: y.min(area.bottom()) }
}
