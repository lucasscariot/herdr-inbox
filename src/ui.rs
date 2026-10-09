//! Drawing. Pure: reads the app and the palette, writes a frame.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Connection, Focus, NoticeKind, RowKind, StreamState, TAKEN_OVER};
use crate::herdr::types::AgentStatus;
use crate::theme::Palette;
use crate::threads::{Group, Thread};

pub fn draw(frame: &mut Frame, app: &App, palette: &Palette, now: SystemTime) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    match app.connection {
        Connection::NoServer | Connection::Starting(_) => {
            fill(buf, area, Style::new());
            no_server(buf, area, app, palette);
        }
        _ => {
            sidebar(buf, app, palette, now);
            separator(buf, app, palette);
            terminal(buf, app, palette);
            if let Some((x, y)) = cursor(app) {
                frame.set_cursor_position((x, y));
            }
        }
    }
    status_bar(frame.buffer_mut(), app, palette);
}

/// Where the real terminal cursor goes: the agent's cursor, when the agent
/// has focus and shows one.
pub fn cursor(app: &App) -> Option<(u16, u16)> {
    if app.focus != Focus::Terminal {
        return None;
    }
    let open = app.open.as_ref()?;
    if !matches!(open.stream, StreamState::Live) {
        return None;
    }
    let (col, row) = open.screen.cursor()?;
    let area = app.layout.terminal;
    (col < area.width && row < area.height).then_some((area.x + col, area.y + row))
}

fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(style);
            }
        }
    }
}

pub fn status_color(status: AgentStatus, palette: &Palette) -> Color {
    match status {
        AgentStatus::Blocked => palette.red,
        AgentStatus::Done => palette.teal,
        AgentStatus::Working => palette.yellow,
        AgentStatus::Idle => palette.overlay1,
        AgentStatus::Unknown => palette.overlay0,
    }
}

pub fn badge(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "● input",
        AgentStatus::Done => "✓ ready",
        AgentStatus::Working => "◐ working",
        AgentStatus::Idle => "○ idle",
        AgentStatus::Unknown => "· unknown",
    }
}

/// "now", "42s", "5m", "3h", "2d": how long ago a status changed.
pub fn age(changed: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(changed).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Cuts `text` to `width` columns, ending with `…` when it had to cut.
pub fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Draws `left` and `right` on one line of `area`, clipping `left` so `right`
/// always fits.
fn split_line(buf: &mut Buffer, x: u16, y: u16, width: u16, left: &[(String, Style)], right: &[(String, Style)]) {
    let right_width: usize = right.iter().map(|(t, _)| t.width()).sum();
    let left_room = (width as usize).saturating_sub(if right_width > 0 { right_width + 1 } else { 0 });
    let mut cursor = x;
    let mut room = left_room;
    for (text, style) in left {
        if room == 0 {
            break;
        }
        let text = clip(text, room);
        let w = text.width();
        buf.set_stringn(cursor, y, &text, w, *style);
        cursor += w as u16;
        room -= w;
    }
    if right_width > 0 && right_width < width as usize {
        let mut rx = x + width - right_width as u16;
        for (text, style) in right {
            buf.set_stringn(rx, y, text, text.width(), *style);
            rx += text.width() as u16;
        }
    }
}

fn sidebar(buf: &mut Buffer, app: &App, palette: &Palette, now: SystemTime) {
    let area = app.layout.sidebar;
    if area.width == 0 || area.height == 0 {
        return;
    }
    fill(buf, area, Style::new().bg(palette.sidebar_bg).fg(palette.text));
    let inner_x = area.x + 1;
    let inner_w = area.width.saturating_sub(2);

    // Header: wordmark and a one-line summary.
    let title = [
        ("herdr ".to_string(), Style::new().fg(palette.overlay0)),
        ("inbox".to_string(), Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
    ];
    split_line(buf, inner_x, area.y, inner_w, &title, &[]);
    if area.height > 1 {
        split_line(buf, inner_x, area.y + 1, inner_w, &summary(app, palette), &[]);
    }

    let list = app.layout.list;
    let highlighted = highlighted(app);
    for (screen_row, row) in app.layout.rows.iter().skip(app.layout.offset).take(list.height as usize).enumerate() {
        let y = list.y + screen_row as u16;
        match row.kind {
            RowKind::Heading { group, count } => heading(buf, inner_x, y, inner_w, group, count, palette),
            RowKind::Blank => {}
            RowKind::Thread { index, line } => {
                let Some(thread) = app.threads.get(index) else {
                    continue;
                };
                let is_highlighted = highlighted.is_some_and(|id| id == thread.id);
                let is_open = app.open.as_ref().is_some_and(|o| o.id == thread.id);
                let confirming = app.confirm_archive.as_deref() == Some(thread.id.as_str());
                let row_area = Rect::new(area.x, y, area.width, 1);
                if is_highlighted {
                    fill(buf, row_area, Style::new().bg(palette.active_row_bg).fg(palette.text));
                }
                thread_line(buf, row_area, thread, line, is_open, confirming, palette, now);
            }
        }
    }
    if app.threads.is_empty() && list.height > 0 && app.connection == Connection::Live {
        let text = Paragraph::new(vec![
            Line::styled("No agent threads yet.", Style::new().fg(palette.subtext0)),
            Line::styled("Start one in Herdr; it shows up here.", Style::new().fg(palette.overlay0)),
        ])
        .wrap(Wrap { trim: true });
        ratatui::widgets::Widget::render(text, Rect::new(inner_x, list.y, inner_w, list.height), buf);
    }
}

fn summary(app: &App, palette: &Palette) -> Vec<(String, Style)> {
    let dim = Style::new().fg(palette.overlay0);
    match &app.connection {
        Connection::Connecting => return vec![("connecting…".into(), dim)],
        Connection::Lost(_) => return vec![("reconnecting…".into(), Style::new().fg(palette.red))],
        _ => {}
    }
    let count = |group| app.threads.iter().filter(|t| t.group() == group).count();
    let needs = count(Group::NeedsInput);
    let ready = count(Group::Ready);
    if needs == 0 && ready == 0 {
        let total = app.threads.len();
        return vec![(format!("{total} thread{}", if total == 1 { "" } else { "s" }), dim)];
    }
    // When something wants attention, that is the summary.
    let mut parts = Vec::new();
    if needs > 0 {
        parts.push((format!("{needs} need{} input", if needs == 1 { "s" } else { "" }), Style::new().fg(palette.red)));
    }
    if ready > 0 {
        if !parts.is_empty() {
            parts.push((" · ".into(), dim));
        }
        parts.push((format!("{ready} ready"), Style::new().fg(palette.teal)));
    }
    parts
}

fn heading(buf: &mut Buffer, x: u16, y: u16, width: u16, group: Group, count: usize, palette: &Palette) {
    let color = match group {
        Group::NeedsInput => palette.red,
        Group::Ready => palette.teal,
        Group::Working => palette.yellow,
        Group::Idle => palette.overlay1,
        Group::Unknown => palette.overlay0,
    };
    let parts = [
        (group.heading().to_uppercase(), Style::new().fg(color).add_modifier(Modifier::BOLD)),
        (format!("  {count}"), Style::new().fg(palette.overlay0)),
    ];
    split_line(buf, x, y, width, &parts, &[]);
}

/// The id highlighted in the list: the cursor while the list has focus, the
/// open thread otherwise.
fn highlighted(app: &App) -> Option<&str> {
    match app.focus {
        Focus::List => app.cursor.as_deref(),
        Focus::Terminal => app.open.as_ref().map(|o| o.id.as_str()),
    }
}

#[allow(clippy::too_many_arguments)] // One row of one thread needs all of these.
fn thread_line(
    buf: &mut Buffer,
    area: Rect,
    thread: &Thread,
    line: u16,
    is_open: bool,
    confirming: bool,
    palette: &Palette,
    now: SystemTime,
) {
    let color = status_color(thread.status, palette);
    buf.set_string(area.x, area.y, "▎", Style::new().fg(color));
    let x = area.x + 1;
    let width = area.width.saturating_sub(2);
    let dim = Style::new().fg(palette.overlay0);
    match line {
        0 => split_line(
            buf,
            x,
            area.y,
            width,
            &[(thread.project.clone(), Style::new().fg(palette.subtext0))],
            &[(badge(thread.status).to_string(), Style::new().fg(color))],
        ),
        1 => {
            let mut style = Style::new().fg(palette.text).add_modifier(Modifier::BOLD);
            if is_open {
                style = style.fg(palette.accent);
            }
            split_line(buf, x, area.y, width, &[(thread.title.clone(), style)], &[]);
        }
        _ if confirming => split_line(
            buf,
            x,
            area.y,
            width,
            &[
                ("Archive this thread? ".into(), Style::new().fg(palette.red).add_modifier(Modifier::BOLD)),
                ("y".into(), Style::new().fg(palette.text).add_modifier(Modifier::BOLD)),
                (" / n".into(), dim),
            ],
            &[],
        ),
        _ => {
            let mut left = Vec::new();
            if let Some(branch) = &thread.branch {
                left.push((format!("⎇ {branch}"), Style::new().fg(palette.mauve)));
                left.push((" · ".into(), dim));
            }
            left.push((thread.harness.clone(), dim));
            let right = thread.changed_at.map(|changed| vec![(age(changed, now), dim)]).unwrap_or_default();
            split_line(buf, x, area.y, width, &left, &right);
        }
    }
}

fn separator(buf: &mut Buffer, app: &App, palette: &Palette) {
    let x = app.layout.sidebar.right();
    if x >= app.layout.width {
        return;
    }
    for y in 0..app.layout.sidebar.height {
        buf.set_string(x, y, "│", Style::new().fg(palette.surface1));
    }
}

fn terminal(buf: &mut Buffer, app: &App, palette: &Palette) {
    let area = app.layout.terminal;
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Agent output uses the terminal's own background, so the area does too.
    fill(buf, area, Style::new());
    let Some(open) = &app.open else {
        let message = match app.connection {
            Connection::Connecting => "Connecting to Herdr…",
            Connection::Lost(_) => "Lost the connection to Herdr. Retrying…",
            _ if app.threads.is_empty() => "Agent threads you start in Herdr appear on the left.",
            _ => "Pick a thread on the left and press Enter.",
        };
        centered(buf, area, &[Line::styled(message, Style::new().fg(palette.overlay0))]);
        return;
    };
    open.screen.render(area, buf);
    match &open.stream {
        StreamState::Attaching if !open.screen.has_content() => {
            let title = app.thread(&open.id).map(|t| t.title.as_str()).unwrap_or("thread");
            centered(buf, area, &[Line::styled(format!("Opening {title}…"), Style::new().fg(palette.overlay0))]);
        }
        StreamState::Closed { reason } => closed_card(buf, area, reason.as_deref(), palette),
        _ => {}
    }
}

fn closed_card(buf: &mut Buffer, area: Rect, reason: Option<&str>, palette: &Palette) {
    let taken = reason == Some(TAKEN_OVER);
    let headline =
        if taken { "This thread is open in another window." } else { "The live view of this thread stopped." };
    let mut lines = vec![Line::styled(headline, Style::new().fg(palette.text).add_modifier(Modifier::BOLD))];
    if let Some(reason) = reason.filter(|_| !taken) {
        lines.push(Line::styled(reason.to_string(), Style::new().fg(palette.overlay0)));
    }
    lines.push(Line::from(vec![
        Span::styled("Enter", Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
        Span::styled(" to bring it back here", Style::new().fg(palette.subtext0)),
    ]));
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
    let height = lines.len() as u16 + 2;
    let card = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );
    fill(buf, card, Style::new().bg(palette.surface0));
    centered(buf, card, &lines);
}

/// Draws lines centred in `area`, word-wrapping any that are too wide.
fn centered(buf: &mut Buffer, area: Rect, lines: &[Line]) {
    let width = area.width.saturating_sub(2).max(1) as usize;
    let height: usize = lines.iter().map(|line| line.width().div_ceil(width).max(1)).sum();
    let height = height as u16;
    let top = area.y + area.height.saturating_sub(height) / 2;
    let paragraph = Paragraph::new(lines.to_vec()).alignment(Alignment::Center).wrap(Wrap { trim: true });
    let inner = Rect::new(area.x + 1.min(area.width), top, width as u16, height.min(area.height));
    ratatui::widgets::Widget::render(paragraph, inner, buf);
}

fn no_server(buf: &mut Buffer, area: Rect, app: &App, palette: &Palette) {
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
    let lines = match app.connection {
        Connection::Starting(_) => vec![Line::styled("Starting Herdr…", Style::new().fg(palette.subtext0))],
        _ => vec![
            Line::from(vec![
                Span::styled("herdr ", Style::new().fg(palette.overlay0)),
                Span::styled("inbox", Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
            ]),
            Line::default(),
            Line::styled("No Herdr server is running.", Style::new().fg(palette.text)),
            Line::default(),
            Line::from(vec![
                Span::styled("Enter", Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" start one   ", Style::new().fg(palette.subtext0)),
                Span::styled("q", Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" quit", Style::new().fg(palette.subtext0)),
            ]),
        ],
    };
    centered(buf, body, &lines);
}

fn status_bar(buf: &mut Buffer, app: &App, palette: &Palette) {
    let area = app.layout.bar;
    if area.width == 0 || area.height == 0 {
        return;
    }
    fill(buf, area, Style::new());
    let key = Style::new().fg(palette.subtext0).add_modifier(Modifier::BOLD);
    let text = Style::new().fg(palette.overlay0);
    // The mode badge is accent text, not a filled block: terminals extend the
    // last row's background into their padding.
    let (badge, hints): (&str, Vec<(&str, &str)>) = match (&app.connection, app.focus) {
        (Connection::NoServer | Connection::Starting(_), _) => ("", vec![]),
        (_, _) if app.confirm_archive.is_some() => ("THREADS", vec![("y", "archive"), ("n", "keep")]),
        (_, Focus::List) => {
            ("THREADS", vec![("↵", "open"), ("j/k", "move"), ("x", "archive"), ("tab", "agent"), ("q", "quit")])
        }
        (_, Focus::Terminal) => ("AGENT", vec![("tab", "threads")]),
    };
    let mut left: Vec<(String, Style)> = Vec::new();
    if !badge.is_empty() {
        left.push((format!(" {badge} "), Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)));
        left.push((" ".into(), text));
    }
    for (index, (k, label)) in hints.iter().enumerate() {
        if index > 0 {
            left.push(("  ".into(), text));
        }
        left.push(((*k).into(), key));
        left.push((format!(" {label}"), text));
    }
    let right = match (&app.notice, &app.connection) {
        (Some(notice), _) => {
            let color = match notice.kind {
                NoticeKind::Info => palette.green,
                NoticeKind::Error => palette.red,
            };
            vec![(notice.text.clone(), Style::new().fg(color))]
        }
        (None, Connection::Lost(reason)) => vec![(format!("✗ {reason}"), Style::new().fg(palette.red))],
        (None, Connection::Connecting) => vec![("◌ connecting".into(), text)],
        _ => vec![],
    };
    // A long notice takes the room of the hints rather than being cut.
    let right_width: usize = right.iter().map(|(t, _)| t.width()).sum();
    let right = match right.first() {
        Some((text, style)) if right_width + 2 > area.width as usize => {
            vec![(clip(text, area.width.saturating_sub(2) as usize), *style)]
        }
        _ => right,
    };
    split_line(buf, area.x + 1, area.y, area.width.saturating_sub(2), &left, &right);
}

#[cfg(test)]
mod tests;
