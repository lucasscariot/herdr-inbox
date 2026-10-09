//! Dictation in the app: what Ctrl+T records into, what Enter, Ctrl+T and
//! Esc do while recording, where the transcript goes, and the menu that
//! connects a transcription service.
//!
//! Words are never lost: a failed delivery keeps the transcript in its notice.

use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect, Focus, LAUNCH_PREFIX, NoticeKind};
use crate::editor::Editor;
use crate::speech::backends::{Credentials, SERVICES};
use crate::threads::ThreadId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Composer,
    Reply,
    Thread(ThreadId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The recorder is starting.
    Starting,
    Recording,
    Transcribing,
}

/// What happens to the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// Deliver it: launch, reply, or prompt the agent.
    Send,
    /// Put it where the cursor is, unsent.
    Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dictation {
    pub target: Target,
    pub phase: Phase,
    pub levels: Vec<f32>,
    pub quiet: bool,
    pub started: SystemTime,
    pub then: Option<Then>,
    /// Started by holding the space bar: letting go types the words.
    pub held: bool,
}

/// A hold let go sooner than this was a long space, not dictation.
pub const SHORTEST_HOLD: Duration = Duration::from_millis(600);

/// What the runtime found: the active transcription path, local tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechStatus {
    /// e.g. "Groq Whisper"; `None` when nothing can transcribe.
    pub ready: Option<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Service(&'static str),
    BuildWhisper,
    Command,
    Disconnect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A pasted API key, verified before it is saved.
    Key {
        service: &'static str,
        editor: Editor,
    },
    Command {
        editor: Editor,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Menu {
    pub selected: usize,
    pub entry: Option<Entry>,
    /// The latest progress or result line.
    pub status: Option<(String, bool)>,
}

pub fn menu_items() -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = SERVICES.iter().map(|s| MenuItem::Service(s.id)).collect();
    items.extend([MenuItem::BuildWhisper, MenuItem::Command, MenuItem::Disconnect]);
    items
}

impl App {
    /// Ctrl+T: starts recording into whatever has the keyboard.
    pub(crate) fn start_dictation(&mut self, now: SystemTime, effects: &mut Vec<Effect>) {
        if self.dictation.is_some() {
            return;
        }
        let target = match self.focus {
            Focus::Composer => Target::Composer,
            _ if self.reply.is_some() => Target::Reply,
            Focus::Terminal => match &self.open {
                Some(open) => Target::Thread(open.id.clone()),
                None => return,
            },
            Focus::List => match self.cursor.clone() {
                Some(id) if !id.starts_with(LAUNCH_PREFIX) => Target::Thread(id),
                _ => return,
            },
        };
        if self.speech.ready.is_none() {
            // Nothing can transcribe yet: recording would lose the words.
            self.menu = Some(Menu {
                status: Some(("Connect a transcription service to dictate.".into(), false)),
                ..Menu::default()
            });
            return;
        }
        self.dictation = Some(Dictation {
            target,
            phase: Phase::Starting,
            levels: Vec::new(),
            quiet: false,
            started: now,
            then: None,
            held: false,
        });
        effects.push(Effect::StartDictation);
    }

    /// Whether a space typed now could be the start of a held bar: wherever
    /// a space is text, and in the thread list, where it does nothing else.
    pub(crate) fn space_holds(&self) -> bool {
        self.config.speech.space_hold_enabled()
            && self.menu.is_none()
            && self.dictation.is_none()
            && !self.needs_server_screen()
            && match self.focus {
                Focus::Terminal => self.open.is_some(),
                Focus::Composer => self.composer.picker.is_none() && self.composer.field == super::Field::Task,
                Focus::List => !self.filtering,
            }
    }

    /// The space bar is held: dictate, as Ctrl+T would.
    pub(crate) fn hold_space(&mut self, now: SystemTime, effects: &mut Vec<Effect>) {
        self.start_dictation(now, effects);
        if let Some(dictation) = self.dictation.as_mut() {
            dictation.held = true;
        }
    }

    /// The held space bar was let go: type the words. Let go too soon, it was
    /// a long press on a space, so a space is what it types.
    pub(crate) fn release_space(&mut self, now: SystemTime, effects: &mut Vec<Effect>) {
        let Some(dictation) = self.dictation.as_mut().filter(|d| d.held) else {
            return;
        };
        if !matches!(dictation.phase, Phase::Starting | Phase::Recording) {
            return;
        }
        if now.duration_since(dictation.started).unwrap_or_default() < SHORTEST_HOLD {
            self.dictation = None;
            effects.push(Effect::CancelDictation);
            return super::input::type_spaces(self, 1, now, effects);
        }
        dictation.then = Some(Then::Type);
        dictation.phase = Phase::Transcribing;
        effects.push(Effect::StopDictation);
    }

    /// Keys while dictating. Returns true when the key was used.
    pub(crate) fn dictation_key(&mut self, key: KeyEvent, effects: &mut Vec<Effect>) -> bool {
        let Some(dictation) = self.dictation.as_mut() else {
            return false;
        };
        let ctrl_t = key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL);
        match (dictation.phase, key.code) {
            (_, KeyCode::Esc) => {
                self.dictation = None;
                effects.push(Effect::CancelDictation);
            }
            (Phase::Starting | Phase::Recording, KeyCode::Enter) => {
                dictation.then = Some(Then::Send);
                dictation.phase = Phase::Transcribing;
                effects.push(Effect::StopDictation);
            }
            (Phase::Starting | Phase::Recording, _) if ctrl_t => {
                dictation.then = Some(Then::Type);
                dictation.phase = Phase::Transcribing;
                effects.push(Effect::StopDictation);
            }
            // Nothing else reaches an agent or a box while the mic is open.
            _ => {}
        }
        true
    }

    pub(crate) fn on_dictation_started(&mut self, result: Result<(), String>, now: SystemTime) {
        match result {
            Ok(()) => {
                if let Some(d) = self.dictation.as_mut().filter(|d| d.phase == Phase::Starting) {
                    d.phase = Phase::Recording;
                }
            }
            Err(error) => {
                self.dictation = None;
                self.notify(error, NoticeKind::Error, now);
            }
        }
    }

    pub(crate) fn on_levels(&mut self, levels: Vec<f32>, quiet: bool) {
        if let Some(d) = self.dictation.as_mut() {
            d.levels = levels;
            d.quiet = quiet;
        }
    }

    pub(crate) fn on_transcribed(
        &mut self,
        result: Result<String, String>,
        now: SystemTime,
        effects: &mut Vec<Effect>,
    ) {
        // A cancelled dictation discards its late transcript.
        let Some(dictation) = self.dictation.take().filter(|d| d.phase == Phase::Transcribing) else {
            return;
        };
        let text = match result {
            Ok(text) => text,
            Err(error) => return self.notify(error, NoticeKind::Error, now),
        };
        let then = dictation.then.unwrap_or(Then::Type);
        match dictation.target {
            Target::Composer => {
                self.composer.field = super::Field::Task;
                self.composer.picker = None;
                insert_word(&mut self.composer.task, &text);
                if then == Then::Send {
                    self.send(false, now, effects);
                }
            }
            Target::Reply => {
                let Some(reply) = self.reply.as_mut() else {
                    return self.notify(format!("The reply closed. Your words: “{text}”"), NoticeKind::Error, now);
                };
                insert_word(&mut reply.editor, &text);
                if then == Then::Send {
                    let reply = self.reply.take().expect("checked above");
                    if let Some(thread) = self.thread(&reply.thread) {
                        effects.push(Effect::Prompt {
                            machine: thread.machine_id.clone(),
                            pane_id: thread.pane_id.clone(),
                            title: thread.title.clone(),
                            text: reply.message(),
                        });
                    }
                }
            }
            Target::Thread(id) => {
                let Some(thread) = self.thread(&id).cloned() else {
                    return self.notify(format!("The thread is gone. Your words: “{text}”"), NoticeKind::Error, now);
                };
                effects.push(match then {
                    Then::Send => Effect::Prompt {
                        machine: thread.machine_id,
                        pane_id: thread.pane_id,
                        title: thread.title,
                        text,
                    },
                    Then::Type => Effect::TypeText {
                        machine: thread.machine_id,
                        pane_id: thread.pane_id,
                        title: thread.title,
                        text,
                    },
                });
            }
        }
    }

    /// F10: the dictation menu.
    pub(crate) fn open_menu(&mut self) {
        self.menu = Some(Menu::default());
    }

    pub(crate) fn menu_key(&mut self, key: KeyEvent, effects: &mut Vec<Effect>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if let Some(entry) = menu.entry.as_mut() {
            let editor = match entry {
                Entry::Key { editor, .. } | Entry::Command { editor } => editor,
            };
            match key.code {
                KeyCode::Esc => menu.entry = None,
                KeyCode::Enter => {
                    let value = editor.text().trim().to_string();
                    if value.is_empty() {
                        return;
                    }
                    match menu.entry.take() {
                        Some(Entry::Key { service, .. }) => {
                            menu.status = Some(("Checking the key…".into(), false));
                            effects.push(Effect::VerifyKey { service, key: value });
                        }
                        Some(Entry::Command { .. }) => {
                            let credentials = Credentials {
                                backend: "command".into(),
                                command: value,
                                keys: self.credentials.keys.clone(),
                            };
                            self.save_credentials(credentials, "Custom command saved", effects);
                        }
                        None => {}
                    }
                }
                KeyCode::Backspace => editor.backspace(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => editor.clear(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let mut buffer = [0; 4];
                    editor.insert(c.encode_utf8(&mut buffer));
                }
                _ => {}
            }
            return;
        }
        let items = menu_items();
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => self.menu = None,
            KeyCode::Up | KeyCode::Char('k') => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => menu.selected = (menu.selected + 1).min(items.len() - 1),
            KeyCode::Enter => match items[menu.selected].clone() {
                MenuItem::Service(service) => {
                    menu.entry = Some(Entry::Key { service, editor: Editor::default() });
                    if let Some(url) = crate::speech::backends::service(service).map(|s| s.keys_url) {
                        effects.push(Effect::OpenUrl(url.to_string()));
                    }
                }
                MenuItem::BuildWhisper => {
                    menu.status = Some(("Starting the build…".into(), false));
                    effects.push(Effect::InstallWhisper);
                }
                MenuItem::Command => {
                    let mut editor = Editor::default();
                    editor.set(&self.credentials.command);
                    menu.entry = Some(Entry::Command { editor });
                }
                MenuItem::Disconnect => {
                    self.save_credentials(Credentials::default(), "Dictation disconnected", effects)
                }
            },
            _ => {}
        }
    }

    pub(crate) fn save_credentials(&mut self, credentials: Credentials, message: &str, effects: &mut Vec<Effect>) {
        self.credentials = credentials.clone();
        if let Some(menu) = self.menu.as_mut() {
            menu.status = Some((message.into(), false));
        }
        effects.push(Effect::SaveCredentials(credentials));
    }

    pub(crate) fn on_key_verified(
        &mut self,
        service: &'static str,
        key: String,
        result: Result<(), String>,
        effects: &mut Vec<Effect>,
    ) {
        match result {
            Ok(()) => {
                let mut credentials = self.credentials.clone();
                credentials.keys.insert(service.to_string(), key);
                credentials.backend = service.to_string();
                let label = crate::speech::backends::service(service).map(|s| s.label).unwrap_or(service);
                self.save_credentials(credentials, &format!("{label} connected. Ctrl+T dictates."), effects);
            }
            Err(error) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.status = Some((error, true));
                }
            }
        }
    }

    pub(crate) fn on_whisper(
        &mut self,
        update: Result<Option<String>, String>,
        progress: Option<String>,
        effects: &mut Vec<Effect>,
    ) {
        if let Some(text) = progress {
            if let Some(menu) = self.menu.as_mut() {
                menu.status = Some((text, false));
            }
            return;
        }
        match update {
            Ok(Some(command)) => {
                let credentials =
                    Credentials { backend: "command".into(), command, keys: self.credentials.keys.clone() };
                self.save_credentials(credentials, "Local whisper is ready. Ctrl+T dictates.", effects);
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.status = Some((error, true));
                }
            }
        }
    }
}

/// Inserts dictated words at the cursor, with a space when they would touch
/// the previous word.
fn insert_word(editor: &mut Editor, text: &str) {
    let before = editor.text()[..editor.cursor()].chars().last();
    if before.is_some_and(|c| !c.is_whitespace()) {
        editor.insert(" ");
    }
    editor.insert(text);
}
