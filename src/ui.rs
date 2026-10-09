//! Drawing. Pure: reads the app and the palette, writes a frame.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Connection, Focus, LAUNCH_PREFIX, NoticeKind, RowKind, StreamState, TAKEN_OVER};
use crate::herdr::types::AgentStatus;
use crate::orbit::{self, Ink, Link};
use crate::theme::Palette;
use crate::threads::{Group, Thread};

pub fn draw(frame: &mut Frame, app: &App, palette: &Palette, now: SystemTime) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    match app.needs_server_screen() {
        true => {
            fill(buf, area, Style::new());
            no_server(buf, area, app, palette);
        }
        false => {
            sidebar(buf, app, palette, now);
            separator(buf, app, palette);
            let cursor = if app.focus == Focus::Composer {
                composer::draw(buf, app, app.layout.terminal, palette, now).cursor
            } else {
                terminal(buf, app, palette, now);
                match &app.reply {
                    Some(reply) => reply_box(buf, app, reply, palette),
                    None => cursor(app),
                }
            };
            if let Some((x, y)) = cursor {
                frame.set_cursor_position((x, y));
            }
        }
    }
    if app.menu.is_some() {
        let area = frame.area();
        if let Some(cursor) = menu(frame.buffer_mut(), app, area, palette) {
            frame.set_cursor_position(cursor);
        }
    }
    match &app.dictation {
        Some(dictation) => dictation_bar(frame.buffer_mut(), app, dictation, palette, now),
        None => status_bar(frame.buffer_mut(), app, palette),
    }
}

/// The status bar while dictating: the live meter and what each key does.
fn dictation_bar(buf: &mut Buffer, app: &App, dictation: &crate::app::Dictation, palette: &Palette, now: SystemTime) {
    use crate::app::Phase;
    let area = app.layout.bar;
    if area.width == 0 || area.height == 0 {
        return;
    }
    fill(buf, area, Style::new());
    let key = Style::new().fg(palette.subtext0).add_modifier(Modifier::BOLD);
    let text = Style::new().fg(palette.overlay0);
    let mut left: Vec<(String, Style)> = Vec::new();
    match dictation.phase {
        Phase::Transcribing => {
            left.push(("⟳ Transcribing…".into(), Style::new().fg(palette.yellow).add_modifier(Modifier::BOLD)));
            left.push(("  ".into(), text));
            left.push(("esc".into(), key));
            left.push((" discard".into(), text));
        }
        Phase::Starting | Phase::Recording => {
            let secs = now.duration_since(dictation.started).map(|d| d.as_secs()).unwrap_or(0);
            left.push((
                format!("● REC {}:{:02}", secs / 60, secs % 60),
                Style::new().fg(palette.red).add_modifier(Modifier::BOLD),
            ));
            left.push((" ".into(), text));
            let levels = if dictation.levels.is_empty() {
                vec![0.0; crate::speech::meter::BANDS]
            } else {
                dictation.levels.clone()
            };
            for level in &levels {
                let color = match crate::speech::meter::shade(*level) {
                    2 => palette.red,
                    1 => palette.accent,
                    _ => palette.green,
                };
                left.push((crate::speech::meter::line(&[*level]), Style::new().fg(color)));
            }
            left.push(("  ".into(), text));
            let keys: &[(&str, &str)] = if dictation.held {
                &[("release", "type"), ("↵", "send"), ("esc", "discard")]
            } else {
                &[("↵", "send"), ("⌃T", "type"), ("esc", "discard")]
            };
            for (index, (k, label)) in keys.iter().enumerate() {
                if index > 0 {
                    left.push(("  ".into(), text));
                }
                left.push(((*k).into(), key));
                left.push((format!(" {label}"), text));
            }
        }
    }
    let target = match &dictation.target {
        crate::app::Target::Composer => "into the task".to_string(),
        crate::app::Target::Reply => "into the reply".to_string(),
        crate::app::Target::Thread(id) => app.thread(id).map(|t| format!("to “{}”", t.title)).unwrap_or_default(),
    };
    let right = if dictation.quiet && dictation.phase == Phase::Recording {
        vec![("no sound is reaching the microphone".to_string(), Style::new().fg(palette.yellow))]
    } else {
        vec![(target, text)]
    };
    split_line(buf, area.x + 1, area.y, area.width.saturating_sub(2), &left, &right);
}

/// The dictation menu, centered. Returns the cursor of a key or command entry.
fn menu(buf: &mut Buffer, app: &App, area: Rect, palette: &Palette) -> Option<(u16, u16)> {
    use crate::app::{Entry, MenuItem};
    use crate::speech::backends::service;
    let menu = app.menu.as_ref()?;
    let items = crate::app::menu_items();
    let width = 64.min(area.width.saturating_sub(2));
    let height = (items.len() as u16 + 9).min(area.height.saturating_sub(2));
    if width < 30 || height < 8 {
        return None;
    }
    let rect =
        Rect::new(area.x + (area.width - width) / 2, area.y + area.height.saturating_sub(height) / 2, width, height);
    let bg = Style::new().bg(palette.surface0);
    fill(buf, rect, bg);
    ratatui::widgets::Widget::render(
        ratatui::widgets::Block::new()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::new().fg(palette.accent).bg(palette.surface0))
            .title(" Dictation "),
        rect,
        buf,
    );
    let x = rect.x + 2;
    let inner = rect.width.saturating_sub(4);
    let dim = Style::new().fg(palette.overlay0).bg(palette.surface0);
    let mut y = rect.y + 1;
    let using = match &app.speech.ready {
        Some(label) => vec![
            ("Transcribing with ".to_string(), dim),
            (label.clone(), Style::new().fg(palette.green).bg(palette.surface0)),
        ],
        None => {
            vec![("No transcription yet. Connect one:".to_string(), Style::new().fg(palette.text).bg(palette.surface0))]
        }
    };
    split_line(buf, x, y, inner, &using, &[]);
    y += 2;
    for (index, item) in items.iter().enumerate() {
        if y + 1 >= rect.bottom() {
            break;
        }
        let selected = index == menu.selected && menu.entry.is_none();
        let row_bg = if selected { palette.selection_bg } else { palette.surface0 };
        fill(buf, Rect::new(rect.x + 1, y, rect.width.saturating_sub(2), 1), Style::new().bg(row_bg));
        let (label, detail) = match item {
            MenuItem::Service(id) => {
                let service = service(id);
                let connected = app.credentials.keys.get(*id).is_some_and(|k| !k.is_empty());
                let label = service.map(|s| s.label).unwrap_or(id).to_string();
                let detail = if connected {
                    "connected ✓".to_string()
                } else {
                    service.map(|s| s.detail.to_string()).unwrap_or_default()
                };
                (format!("Connect {label}"), detail)
            }
            MenuItem::BuildWhisper => ("Build whisper.cpp here".to_string(), "offline, about 500 MB".to_string()),
            MenuItem::Command => ("Custom command…".to_string(), "{file} is the recording".to_string()),
            MenuItem::Disconnect => ("Disconnect".to_string(), String::new()),
        };
        split_line(
            buf,
            x,
            y,
            inner,
            &[(label, Style::new().fg(palette.text).bg(row_bg))],
            &[(detail, Style::new().fg(palette.overlay0).bg(row_bg))],
        );
        y += 1;
    }
    let mut cursor = None;
    let entry_y = rect.bottom().saturating_sub(3);
    match &menu.entry {
        Some(Entry::Key { service: id, editor }) => {
            let label = service(id).map(|s| s.label).unwrap_or(id);
            // A key is a secret: show only how much was typed.
            let masked: String = "•".repeat(editor.text().chars().count());
            let prompt = format!("{label} key › ");
            split_line(
                buf,
                x,
                entry_y,
                inner,
                &[(prompt.clone(), dim), (masked.clone(), Style::new().fg(palette.text).bg(palette.surface0))],
                &[],
            );
            cursor = Some((x + prompt.width() as u16 + masked.chars().count() as u16, entry_y));
        }
        Some(Entry::Command { editor }) => {
            let prompt = "Command › ";
            split_line(
                buf,
                x,
                entry_y,
                inner,
                &[
                    (prompt.into(), dim),
                    (editor.text().to_string(), Style::new().fg(palette.text).bg(palette.surface0)),
                ],
                &[],
            );
            cursor = Some((x + prompt.width() as u16 + editor.text().width() as u16, entry_y));
        }
        None => {}
    }
    if let Some((status, error)) = &menu.status {
        let color = if *error { palette.red } else { palette.subtext0 };
        split_line(
            buf,
            x,
            rect.bottom().saturating_sub(2),
            inner,
            &[(status.clone(), Style::new().fg(color).bg(palette.surface0))],
            &[],
        );
    }
    cursor.map(|(cx, cy)| (cx.min(rect.right().saturating_sub(2)), cy))
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

/// A status as text: its colour, faint when there is nothing to act on. An
/// idle thread should be the quietest thing on screen, and `overlay1` is pure
/// white in the terminal theme.
pub fn status_style(status: AgentStatus, palette: &Palette) -> Style {
    let style = Style::new().fg(status_color(status, palette));
    match status {
        AgentStatus::Idle | AgentStatus::Unknown => style.add_modifier(Modifier::DIM),
        _ => style,
    }
}

/// Lines that divide without shouting: the sidebar separator and a frame that
/// does not have focus. Faint, because `surface1` is a plain ANSI grey in the
/// terminal theme and renders as a bright line in many terminal palettes.
pub fn hairline(palette: &Palette) -> Style {
    Style::new().fg(palette.surface1).add_modifier(Modifier::DIM)
}

/// The frame of a text box: the accent while it has focus, softened so the
/// text inside stays the brightest thing; a hairline otherwise.
pub fn frame(palette: &Palette, focused: bool) -> Style {
    match focused {
        true => Style::new().fg(palette.accent).add_modifier(Modifier::DIM),
        false => hairline(palette),
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
    let button = app.layout.new_button;
    if button.width > 0 {
        let style = if app.focus == Focus::Composer {
            Style::new().fg(palette.accent).bg(palette.selection_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(palette.accent).bg(palette.surface0).add_modifier(Modifier::BOLD)
        };
        fill(buf, Rect::new(button.x + 1, button.y, button.width.saturating_sub(2), 1), style);
        split_line(
            buf,
            inner_x + 1,
            button.y,
            inner_w.saturating_sub(2),
            &[("+  New thread".into(), style)],
            &[("n".into(), Style::new().fg(palette.overlay0).bg(style.bg.unwrap_or_default()))],
        );
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
    if app.threads.is_empty() && list.height > 0 && app.overall_connection().is_live() {
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
    if app.filtering || !app.filter.is_empty() {
        let caret = if app.filtering { "▏" } else { "" };
        return vec![
            ("/ ".into(), Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)),
            (format!("{}{caret}", app.filter), Style::new().fg(palette.text)),
            (format!("  {} shown", app.threads.len()), dim),
        ];
    }
    match app.overall_connection() {
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
    let style = match group {
        Group::NeedsInput => Style::new().fg(palette.red),
        Group::Ready => Style::new().fg(palette.teal),
        Group::Working => Style::new().fg(palette.yellow),
        Group::Idle => status_style(AgentStatus::Idle, palette),
        Group::Unknown => status_style(AgentStatus::Unknown, palette),
    };
    let parts = [
        (group.heading().to_uppercase(), style.add_modifier(Modifier::BOLD)),
        (format!("  {count}"), Style::new().fg(palette.overlay0)),
    ];
    split_line(buf, x, y, width, &parts, &[]);
}

/// The id highlighted in the list: the cursor while the list has focus, the
/// open thread otherwise.
fn highlighted(app: &App) -> Option<&str> {
    match app.focus {
        Focus::List => app.cursor.as_deref(),
        Focus::Terminal | Focus::Composer => app.open.as_ref().map(|o| o.id.as_str()),
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
    let status = status_style(thread.status, palette);
    buf.set_string(area.x, area.y, "▎", status);
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
            &[(badge(thread.status).to_string(), status)],
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
        _ if note_line(thread).is_some() => {
            let (text, color) = note_line(thread).unwrap_or_default();
            let color = match color {
                NoteColor::Red => palette.red,
                NoteColor::Yellow => palette.yellow,
                NoteColor::Dim => palette.overlay0,
            };
            split_line(buf, x, area.y, width, &[(text, Style::new().fg(color))], &[]);
        }
        _ => {
            let mut left = Vec::new();
            if let Some(branch) = &thread.branch {
                left.push((format!("⎇ {branch}"), Style::new().fg(palette.mauve)));
                left.push((" · ".into(), dim));
            }
            if let Some(machine) = &thread.machine_label {
                left.push((machine.clone(), Style::new().fg(palette.subtext0)));
                left.push((" · ".into(), dim));
            }
            left.push((thread.harness.clone(), dim));
            let right = thread.changed_at.map(|changed| vec![(age(changed, now), dim)]).unwrap_or_default();
            split_line(buf, x, area.y, width, &left, &right);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum NoteColor {
    Red,
    Yellow,
    #[default]
    Dim,
}

/// The third row of a thread whose launch has something to say.
fn note_line(thread: &Thread) -> Option<(String, NoteColor)> {
    use crate::threads::LaunchNote;
    match thread.note.as_ref()? {
        LaunchNote::Failed(error) => Some((format!("failed: {error}"), NoteColor::Red)),
        LaunchNote::Waiting => Some(("waiting: answer its prompt".into(), NoteColor::Yellow)),
        // Once the agent reacts, the doubt is gone.
        LaunchNote::Unverified if matches!(thread.status, AgentStatus::Idle | AgentStatus::Unknown) => {
            Some(("sent, not confirmed yet".into(), NoteColor::Dim))
        }
        LaunchNote::Unverified => None,
    }
}

/// The reply box, over the bottom of the terminal area. Returns the cursor.
fn reply_box(buf: &mut Buffer, app: &App, reply: &crate::app::Reply, palette: &Palette) -> Option<(u16, u16)> {
    let area = app.layout.terminal;
    if area.width < 12 || area.height < 6 {
        return None;
    }
    let inner_width = area.width.saturating_sub(6);
    let (lines, (row, col)) = reply.editor.layout(inner_width);
    let rows = (lines.len() as u16).clamp(1, 6);
    let height = rows + 3;
    let rect = Rect::new(area.x + 1, area.bottom().saturating_sub(height), area.width.saturating_sub(2), height);
    fill(buf, rect, Style::new().bg(palette.surface0));
    let title = app.thread(&reply.thread).map(|t| t.title.clone()).unwrap_or_default();
    ratatui::widgets::Widget::render(
        ratatui::widgets::Block::new()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(frame(palette, true).bg(palette.surface0))
            .title(format!(" Reply to “{}” ", clip(&title, inner_width.saturating_sub(12) as usize))),
        rect,
        buf,
    );
    let scroll = row.saturating_sub(rows - 1);
    for (index, line) in lines.iter().skip(scroll as usize).take(rows as usize).enumerate() {
        buf.set_stringn(
            rect.x + 2,
            rect.y + 1 + index as u16,
            line,
            inner_width as usize,
            Style::new().fg(palette.text).bg(palette.surface0),
        );
    }
    let hint = "↵ send · ⇧↵ new line · esc cancel";
    split_line(
        buf,
        rect.x + 2,
        rect.bottom().saturating_sub(2),
        inner_width,
        &[],
        &[(hint.into(), Style::new().fg(palette.overlay0).bg(palette.surface0))],
    );
    Some((rect.x + 2 + col.min(inner_width), rect.y + 1 + row - scroll))
}

fn separator(buf: &mut Buffer, app: &App, palette: &Palette) {
    let x = app.layout.sidebar.right();
    if x >= app.layout.width {
        return;
    }
    for y in 0..app.layout.sidebar.height {
        buf.set_string(x, y, "│", hairline(palette));
    }
}

fn terminal(buf: &mut Buffer, app: &App, palette: &Palette, now: SystemTime) {
    let area = app.layout.terminal;
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Agent output uses the terminal's own background, so the area does too.
    fill(buf, area, Style::new());
    let Some(open) = &app.open else {
        let message = match app.overall_connection() {
            Connection::Connecting => "Connecting to Herdr…",
            Connection::Lost(_) => "Lost the connection to Herdr. Retrying…",
            _ if app.threads.is_empty() => "Agent threads you start in Herdr appear on the left.",
            _ => "Pick a thread on the left and press Enter.",
        };
        let message = [Line::styled(message, Style::new().fg(palette.overlay0))];
        // The orbit above the message, both centred, when there is room.
        let room = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(3));
        match orbit_in(buf, app, room, palette, now) {
            Some(drawn) => centered(buf, Rect::new(area.x, drawn.bottom(), area.width, 1), &message),
            None => centered(buf, area, &message),
        }
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

/// The orbit as the app sees it: a ring per machine, a bead per thread.
pub fn orbit_fleet(app: &App) -> orbit::Fleet {
    let rings = app
        .machines
        .iter()
        .map(|machine| {
            let link = match machine.connection {
                Connection::Live => Link::Live,
                Connection::Connecting | Connection::Starting(_) => Link::Connecting,
                Connection::NoServer | Connection::Lost(_) => Link::Down,
            };
            // Ordered by id, so a bead keeps its place when its status, and
            // with it the list order, changes.
            let mut threads: Vec<&Thread> =
                app.threads.iter().filter(|t| t.machine_id == machine.id && !t.id.starts_with(LAUNCH_PREFIX)).collect();
            threads.sort_by(|a, b| a.id.cmp(&b.id));
            orbit::Ring { link, beads: threads.iter().map(|t| t.status).collect() }
        })
        .collect();
    orbit::Fleet { rings }
}

/// Draws the orbit centred in `area` if it fits there, and says where.
fn orbit_in(buf: &mut Buffer, app: &App, area: Rect, palette: &Palette, now: SystemTime) -> Option<Rect> {
    let (cols, rows) = orbit::fit(area.width, area.height)?;
    let at = Rect::new(area.x + (area.width - cols) / 2, area.y + (area.height - rows) / 2, cols, rows);
    let millis = now.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let frame = orbit::render(&orbit_fleet(app), millis, cols, rows);
    for (index, cell) in frame.iter().enumerate() {
        let Some(cell) = cell else { continue };
        let (x, y) = (at.x + index as u16 % cols, at.y + index as u16 / cols);
        let style = match cell.ink {
            Ink::Core => Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
            Ink::Bead(status) => status_style(status, palette),
            Ink::Ring { front: false, .. } => Style::new().fg(palette.surface1),
            Ink::Ring { link: Link::Down, front: true } => Style::new().fg(palette.red),
            Ink::Ring { front: true, .. } => Style::new().fg(palette.overlay0),
        };
        if let Some(target) = buf.cell_mut((x, y)) {
            target.set_char(cell.glyph).set_style(style);
        }
    }
    Some(at)
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
    let lines = match app.local().connection {
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

fn running_launches(app: &App) -> usize {
    app.launches.iter().filter(|l| matches!(l.state, crate::app::LaunchState::Running(_))).count()
}

/// One mark per machine: live, connecting, no server, unreachable.
fn machine_strip(app: &App, palette: &Palette) -> Vec<(String, Style)> {
    let mut parts = Vec::new();
    for (index, machine) in app.machines.iter().enumerate() {
        if index > 0 {
            parts.push(("  ".into(), Style::new()));
        }
        let (mark, color) = match &machine.connection {
            Connection::Live => ("●", palette.green),
            Connection::Connecting | Connection::Starting(_) => ("◌", palette.overlay0),
            Connection::NoServer => ("○", palette.red),
            Connection::Lost(_) => ("✗", palette.red),
        };
        parts.push((format!("{mark} "), Style::new().fg(color)));
        parts.push((machine.label.clone(), Style::new().fg(palette.subtext0)));
        if machine.connection == Connection::NoServer {
            parts.push((" s start".into(), Style::new().fg(palette.overlay0)));
        }
    }
    parts
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
    let (badge, hints): (&str, Vec<(&str, &str)>) = match (app.needs_server_screen(), app.focus) {
        (true, _) => ("", vec![]),
        (_, _) if app.confirm_archive.is_some() => ("THREADS", vec![("y", "archive"), ("n", "keep")]),
        (_, _) if app.reply.is_some() => ("REPLY", vec![("↵", "send"), ("esc", "cancel")]),
        (_, Focus::List) if app.filtering => ("FILTER", vec![("↵", "keep"), ("esc", "clear")]),
        (_, Focus::List) => (
            "THREADS",
            vec![("↵", "open"), ("j/k", "move"), ("n", "new"), ("x", "archive"), ("tab", "agent"), ("q", "quit")],
        ),
        (_, Focus::Terminal) if app.config.speech.space_hold_enabled() => {
            ("AGENT", vec![("tab", "threads"), ("hold ␣", "dictate")])
        }
        (_, Focus::Terminal) => ("AGENT", vec![("tab", "threads"), ("⌃T", "dictate")]),
        (_, Focus::Composer) => ("NEW THREAD", vec![("esc", "back")]),
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
    let right = match &app.notice {
        Some(notice) => {
            let color = match notice.kind {
                NoticeKind::Info => palette.green,
                NoticeKind::Error => palette.red,
            };
            vec![(notice.text.clone(), Style::new().fg(color))]
        }
        None if running_launches(app) > 0 && app.focus != Focus::Composer => {
            let count = running_launches(app);
            vec![(format!("⟳ {count} launch{}", if count == 1 { "" } else { "es" }), Style::new().fg(palette.yellow))]
        }
        None if !app.local_only() => machine_strip(app, palette),
        None => match &app.local().connection {
            Connection::Lost(reason) => vec![(format!("✗ {reason}"), Style::new().fg(palette.red))],
            Connection::Connecting => vec![("◌ connecting".into(), text)],
            _ => vec![],
        },
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

mod composer;

#[cfg(test)]
mod tests;
