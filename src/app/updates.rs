//! The update dialog. Checking and installing are runtime effects; closing
//! the dialog never cancels them or changes the draft underneath it.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

use super::Effect;
use crate::editor::Editor;
use crate::update::{Installed, RELEASES_URL, Release};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Ready(Release),
    Installing(Release),
    Installed(Installed),
    Failed {
        release: Option<Release>,
        error: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Updates {
    pub visible: bool,
    pub phase: UpdatePhase,
    pub scroll: u16,
}

/// Shared geometry for drawing and keeping the notes' scroll in bounds.
pub fn dialog(width: u16, height: u16) -> Rect {
    let w = 76.min(width.saturating_sub(2));
    let h = 24.min(height.saturating_sub(2));
    Rect::new((width - w) / 2, (height - h) / 2, w, h)
}

impl Updates {
    pub fn release(&self) -> Option<&Release> {
        match &self.phase {
            UpdatePhase::Ready(release) | UpdatePhase::Installing(release) => Some(release),
            UpdatePhase::Failed { release, .. } => release.as_ref(),
            _ => None,
        }
    }

    pub fn can_install(&self) -> bool {
        matches!(self.phase, UpdatePhase::Ready(_) | UpdatePhase::Failed { .. })
            && self.release().is_some_and(Release::can_install)
    }

    fn check(&mut self, effects: &mut Vec<Effect>) {
        if matches!(self.phase, UpdatePhase::Checking | UpdatePhase::Installing(_) | UpdatePhase::Installed(_)) {
            return;
        }
        self.phase = UpdatePhase::Checking;
        self.scroll = 0;
        effects.push(Effect::CheckUpdate);
    }

    pub(super) fn open(&mut self, effects: &mut Vec<Effect>) {
        if !self.visible {
            self.check(effects);
        }
        self.visible = true;
    }

    pub(super) fn checked(&mut self, result: Result<Release, String>) {
        if self.phase != UpdatePhase::Checking {
            return;
        }
        self.phase = match result {
            Ok(release) => UpdatePhase::Ready(release),
            Err(error) => UpdatePhase::Failed { release: None, error },
        };
    }

    pub(super) fn installed(&mut self, result: Result<Installed, String>) {
        let UpdatePhase::Installing(release) = &self.phase else {
            return;
        };
        self.phase = match result {
            Ok(installed) => UpdatePhase::Installed(installed),
            Err(error) => UpdatePhase::Failed { release: Some(release.clone()), error },
        };
    }

    pub fn notes(&self, width: u16) -> Vec<String> {
        let notes = match &self.phase {
            UpdatePhase::Failed { error, release } => {
                format!("{error}\n\n{}", release.as_ref().map(|r| r.notes.as_str()).unwrap_or(""))
            }
            UpdatePhase::Installed(installed) => format!("Updated binary: {}", installed.path.display()),
            _ => {
                self.release().map(|r| r.notes.clone()).filter(|n| !n.is_empty()).unwrap_or("No release notes.".into())
            }
        };
        let mut editor = Editor::default();
        editor.set(&notes);
        editor.layout(width.max(1)).0
    }

    pub(super) fn key(&mut self, key: KeyEvent, width: u16, height: u16, effects: &mut Vec<Effect>) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.visible = false,
            KeyCode::Char('b') => {
                let url = self.release().map(Release::url).unwrap_or_else(|| RELEASES_URL.into());
                effects.push(Effect::OpenUrl(url));
            }
            KeyCode::Char('r') => self.check(effects),
            KeyCode::Enter if self.can_install() => {
                if let Some(release) = self.release().cloned() {
                    self.phase = UpdatePhase::Installing(release.clone());
                    effects.push(Effect::InstallUpdate(release));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = u16::MAX,
            _ => {}
        }
        let area = dialog(width, height);
        let rows = area.height.saturating_sub(10).max(1) as usize;
        let max = self.notes(area.width.saturating_sub(4)).len().saturating_sub(rows);
        self.scroll = self.scroll.min(max.min(u16::MAX as usize) as u16);
    }
}
