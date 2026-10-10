//! The composer, drawn in place of the agent's terminal. Mouse input uses
//! the same geometry, so every visible field and choice is clickable.

use std::time::SystemTime;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::UnicodeWidthStr;

use super::{clip, fill, frame, orbit_in, split_line};
use crate::app::{App, ComposerLayout, Field, LaunchState, Pick};
use crate::theme::Palette;

const LABEL_WIDTH: u16 = crate::app::ComposerLayout::LABEL_WIDTH;

pub struct Drawn {
    pub cursor: Option<(u16, u16)>,
}

pub fn draw(buf: &mut Buffer, app: &App, area: Rect, palette: &Palette, now: SystemTime) -> Drawn {
    let base = Style::new().fg(palette.text).bg(palette.panel_bg);
    let dim = base.fg(palette.overlay0);
    fill(buf, area, base);
    let ctx = app.composer_context();
    let composer = &app.composer;
    let Some(layout) = ComposerLayout::new(area, composer, &ctx) else {
        if area.height > 0 {
            split_line(buf, area.x, area.y, area.width, &[("Window too small".into(), dim)], &[]);
        }
        return Drawn { cursor: None };
    };
    let x = layout.content.x;
    let width = layout.content.width;
    let scanning: Vec<String> =
        app.machines.iter().filter(|m| app.discovering.contains(&m.id)).map(|m| m.label.clone()).collect();
    let status = if scanning.is_empty() { "esc close".into() } else { format!("scanning {}…", scanning.join(", ")) };
    split_line(
        buf,
        x,
        layout.content.y,
        width,
        &[("New thread".into(), base.fg(palette.accent).add_modifier(Modifier::BOLD))],
        &[(status, dim)],
    );
    split_line(
        buf,
        x,
        layout.task_box.y - 1,
        width,
        &[("What should we build?".into(), base.fg(palette.subtext0))],
        &[],
    );

    let focused = composer.field == Field::Task && composer.picker.is_none();
    let dictating = app.dictation.as_ref().is_some_and(|d| d.target == crate::app::Target::Composer);
    let border = if dictating { base.fg(palette.red) } else { frame(palette, focused).bg(palette.panel_bg) };
    Block::new()
        .style(base)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .render(layout.task_box, buf);
    for (index, line) in
        layout.task_lines.iter().skip(layout.task_scroll as usize).take(layout.task_inner.height as usize).enumerate()
    {
        buf.set_stringn(
            layout.task_inner.x,
            layout.task_inner.y + index as u16,
            line,
            layout.task_inner.width as usize,
            base,
        );
    }
    if composer.task.text().is_empty() {
        buf.set_stringn(
            layout.task_inner.x,
            layout.task_inner.y,
            "Describe the task. Enter sends, Shift+Enter adds a line.",
            layout.task_inner.width as usize,
            dim,
        );
    }
    let mut cursor = (focused && !dictating).then_some(layout.task_cursor);
    if let Some(error) = &composer.error
        && layout.task_box.bottom() < area.bottom()
    {
        split_line(buf, x, layout.task_box.bottom(), width, &[(error.clone(), base.fg(palette.red))], &[]);
    }

    let enabled_fields = composer.fields(&ctx);
    for (field, row) in &layout.fields {
        let enabled = enabled_fields.contains(field);
        let active = composer.field == *field;
        let bg = if active { palette.active_row_bg } else { palette.panel_bg };
        let style = base.bg(bg);
        fill(buf, *row, style);
        let marker = if active { "▸ " } else { "  " };
        let mut left = vec![
            (marker.to_string(), style.fg(palette.accent)),
            (format!("{:<width$}", field.label(), width = LABEL_WIDTH as usize), style.fg(palette.subtext0)),
            (
                composer.value(&ctx, *field),
                if enabled { style.add_modifier(Modifier::BOLD) } else { style.fg(palette.overlay0) },
            ),
        ];
        if *field == Field::Workspace
            && let Some(branch) = composer.branch_preview(&ctx)
        {
            left.push((format!("  ⎇ {branch}"), style.fg(palette.mauve)));
        }
        let key = field.key().filter(|_| enabled).map(|n| format!("F{n}  ›")).unwrap_or_default();
        split_line(buf, row.x, row.y, row.width, &left, &[(key, style.fg(palette.overlay0))]);
    }
    if layout.send.height > 0 {
        let key = base.fg(palette.subtext0).add_modifier(Modifier::BOLD);
        let dictate = if app.config.speech.space_hold_enabled() { "hold ␣" } else { "⌃T" };
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
        split_line(buf, x, layout.send.y, width, &parts, &[]);
    }

    let mut y = layout.details_y;
    for (machine, error) in &app.discovery_errors {
        if y >= area.bottom() {
            break;
        }
        let label = app.machine(machine).map(|m| m.label.clone()).unwrap_or_default();
        split_line(buf, x, y, width, &[(format!("✗ {label}: {error}"), base.fg(palette.red))], &[]);
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
            let left = [
                (format!("{mark} "), base.fg(color)),
                (format!("{} · {}  ", launch.project, launch.harness), base.fg(palette.subtext0)),
                (launch.title.clone(), base),
            ];
            split_line(buf, x, y, width, &left, &[(clip(&text, (width as usize / 2).max(10)), base.fg(color))]);
            y += 1;
        }
    }
    // Decoration uses only spare room below the controls. It never shifts them.
    if y + 2 < area.bottom() {
        orbit_in(buf, app, Rect::new(area.x, y + 1, area.width, (area.bottom() - y - 2).min(14)), palette, now);
    }

    if let Some(picker) = &composer.picker
        && let Some(geometry) = &layout.picker
    {
        let popup = geometry.popup;
        let popup_style = base.bg(palette.surface0);
        fill(buf, popup, popup_style);
        Block::new()
            .style(popup_style)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(frame(palette, true).bg(palette.surface0))
            .title_style(popup_style.fg(palette.subtext0))
            .title(format!(" {} ", picker.field.label()))
            .render(popup, buf);
        let query = format!("› {}", picker.query);
        split_line(buf, geometry.query.x, geometry.query.y, geometry.query.width, &[(query.clone(), popup_style)], &[]);
        cursor = Some((
            geometry.query.x + (query.width() as u16).min(geometry.query.width.saturating_sub(1)),
            geometry.query.y,
        ));
        let choices = composer.choices_with_actions(&ctx, picker.field, &picker.query);
        for (index, choice) in choices.iter().enumerate().skip(geometry.start).take(geometry.rows.height as usize) {
            let row_y = geometry.rows.y + (index - geometry.start) as u16;
            let selected = index == picker.selected;
            let bg = if selected { palette.selection_bg } else { palette.surface0 };
            let row_style = popup_style.bg(bg);
            fill(buf, Rect::new(geometry.rows.x, row_y, geometry.rows.width, 1), row_style);
            let label_style = if !choice.enabled {
                row_style.fg(palette.overlay0)
            } else if selected {
                row_style.fg(palette.accent).add_modifier(Modifier::BOLD)
            } else {
                row_style
            };
            let marker = if selected { "▸ " } else { "  " };
            let mut left = vec![(marker.into(), label_style)];
            let kind = match &choice.pick {
                Pick::Harness(kind) => Some(kind.as_str()),
                Pick::CompareWith(contender) => Some(contender.harness.as_str()),
                _ => None,
            };
            if let Some(kind) = kind {
                let (mark, style) = super::harness::label(kind, palette);
                left.push((mark, style.bg(bg)));
            }
            left.push((choice.label.clone(), label_style));
            split_line(
                buf,
                geometry.query.x,
                row_y,
                geometry.query.width,
                &left,
                &[(choice.detail.clone(), row_style.fg(palette.overlay0))],
            );
        }
        if choices.is_empty() {
            split_line(
                buf,
                geometry.query.x,
                geometry.rows.y,
                geometry.query.width,
                &[("Nothing matches".into(), popup_style.fg(palette.overlay0))],
                &[],
            );
        }
    }
    Drawn { cursor }
}
