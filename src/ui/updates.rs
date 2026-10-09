//! The self-update dialog, drawn without I/O.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap};

use super::{fill, split_line};
use crate::app::{App, UpdatePhase, update_dialog};
use crate::theme::Palette;
use crate::update::CURRENT_VERSION;

pub(super) fn draw(buf: &mut Buffer, app: &App, area: Rect, palette: &Palette) {
    let panel = &app.updates;
    let rect = update_dialog(area.width, area.height);
    let style = Style::new().fg(palette.text).bg(palette.surface0);
    fill(buf, rect, style);
    if rect.width < 32 || rect.height < 12 {
        Paragraph::new("Updates: enlarge the terminal to see the release. Esc goes back.")
            .style(style)
            .wrap(Wrap { trim: true })
            .render(rect, buf);
        return;
    }
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(palette.accent).bg(palette.surface0))
        .title(" Update Herdr Inbox ")
        .render(rect, buf);
    let x = rect.x + 2;
    let width = rect.width - 4;
    let dim = style.fg(palette.subtext0);
    split_line(buf, x, rect.y + 1, width, &[(format!("Running {CURRENT_VERSION}"), dim)], &[]);
    let latest = match &panel.phase {
        UpdatePhase::Checking => "Checking GitHub for the latest stable release…".into(),
        UpdatePhase::Installed(installed) => format!("Installed {}", installed.version),
        _ => panel.release().map(|r| format!("Latest stable {}", r.version)).unwrap_or_default(),
    };
    split_line(buf, x, rect.y + 2, width, &[(latest, style.add_modifier(Modifier::BOLD))], &[]);
    let (status, color) = match &panel.phase {
        UpdatePhase::Ready(release) if !release.newer => ("No newer stable release is available.", palette.green),
        UpdatePhase::Ready(release) if release.asset.is_none() => {
            ("This release has no binary and checksum for your platform. Browse it with b.", palette.yellow)
        }
        UpdatePhase::Ready(_) => ("Enter installs this release. Only the running Inbox binary changes.", palette.text),
        UpdatePhase::Installing(_) => {
            ("Downloading and checking SHA-256. Keep Inbox open until this finishes.", palette.yellow)
        }
        UpdatePhase::Installed(_) => ("Restart Inbox to use the update. Your agents keep running.", palette.green),
        UpdatePhase::Failed { release: Some(_), .. } => {
            ("Update failed. Enter retries; r checks GitHub again.", palette.red)
        }
        UpdatePhase::Failed { .. } => {
            ("Could not check GitHub. Press r to retry, or b to browse releases.", palette.red)
        }
        _ => ("Nothing installs until you confirm with Enter.", palette.subtext0),
    };
    Paragraph::new(status)
        .style(style.fg(color))
        .wrap(Wrap { trim: true })
        .render(Rect::new(x, rect.y + 3, width, 2), buf);
    let heading = if matches!(panel.phase, UpdatePhase::Ready(_) | UpdatePhase::Installing(_)) {
        "Release notes"
    } else {
        "Details"
    };
    split_line(buf, x, rect.y + 5, width, &[(heading.into(), dim)], &[]);
    let rows = rect.height.saturating_sub(10);
    let notes = panel.notes(width);
    let scroll = (panel.scroll as usize).min(notes.len().saturating_sub(rows as usize));
    for (index, line) in notes.iter().skip(scroll).take(rows as usize).enumerate() {
        buf.set_stringn(x, rect.y + 6 + index as u16, line, width as usize, style);
    }
    let action = if panel.can_install() { "Enter install  b GitHub" } else { "b GitHub" };
    split_line(buf, x, rect.bottom() - 3, width, &[(action.into(), dim)], &[]);
    let hint = if matches!(panel.phase, UpdatePhase::Checking | UpdatePhase::Installing(_) | UpdatePhase::Installed(_))
    {
        "↑↓ notes  Esc back"
    } else {
        "↑↓ notes  r recheck  Esc back"
    };
    split_line(buf, x, rect.bottom() - 2, width, &[(hint.into(), dim)], &[]);
}
