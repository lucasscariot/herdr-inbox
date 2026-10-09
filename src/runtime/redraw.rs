//! Redraw scheduling shared by the event loop and clock-driven latency tests.

use std::time::{Duration, Instant, SystemTime};

use crate::app::{App, Effect, Focus, Input};

/// The shortest time between two draws while events stream in.
const FRAME: Duration = Duration::from_millis(16);

pub(super) struct Redraw {
    dirty: bool,
    last_draw: Instant,
}

impl Redraw {
    pub(super) fn new(now: Instant) -> Self {
        Self { dirty: true, last_draw: now - FRAME }
    }

    pub(super) fn update(&mut self, app: &mut App, input: Input, now: SystemTime) -> Vec<Effect> {
        let terminal_key = matches!(&input, Input::Key(_))
            && app.focus == Focus::Terminal
            && app.menu.is_none()
            && app.dictation.is_none();
        let effects = app.update(input, now);
        // Forwarding a key changes the agent, not this screen. Redrawing now
        // would spend the frame budget before the agent's echo arrives.
        // Settling an opt-in held space can also send input while Tab changes
        // focus or Ctrl+T opens a menu, so those still need a draw.
        let forwarded_only = terminal_key
            && app.focus == Focus::Terminal
            && app.menu.is_none()
            && app.dictation.is_none()
            && !effects.is_empty()
            && effects.iter().all(|e| matches!(e, Effect::Send { .. }));
        self.dirty |= !forwarded_only;
        effects
    }

    /// None when nothing changed, zero when a draw is due, otherwise its wait.
    pub(super) fn draw_in(&self, now: Instant) -> Option<Duration> {
        self.dirty.then(|| FRAME.saturating_sub(now.duration_since(self.last_draw)))
    }

    pub(super) fn drawn(&mut self, now: Instant) {
        self.last_draw = now;
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Connection, Focus, OpenThread, StreamState};
    use crate::herdr::terminal::{Control, Frame, Message};
    use crate::screen::Screen;
    use crate::threads::LOCAL;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn at(ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn app(focus: Focus) -> App {
        let mut app = App::new(120, 40, vec![]);
        app.update(Input::Connection { machine: LOCAL.into(), connection: Connection::Live }, at(0));
        let (cols, rows) = app.layout.terminal_size();
        app.open = Some(OpenThread {
            id: format!("{LOCAL}/w1:p1"),
            machine: LOCAL.into(),
            pane_id: "w1:p1".into(),
            generation: 1,
            screen: Screen::new(cols, rows),
            stream: StreamState::Live,
        });
        app.focus = focus;
        app
    }

    fn key(code: KeyCode) -> Input {
        Input::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn frame(seq: u64, bytes: &[u8]) -> Input {
        Input::Terminal {
            generation: 1,
            message: Message::Frame(Frame { seq, width: 79, height: 39, full: false, bytes: bytes.to_vec() }),
        }
    }

    #[test]
    fn a_paused_composer_space_is_visible_without_a_tick() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Composer);
        app.composer.task.set("a");
        redraw.drawn(start);

        redraw.update(&mut app, key(KeyCode::Char(' ')), at(100));

        assert_eq!(app.composer.task.text(), "a ");
        assert!(!app.hold.busy());
        assert_eq!(redraw.draw_in(start + Duration::from_millis(100)), Some(Duration::ZERO));
    }

    #[test]
    fn an_agent_echo_does_not_wait_behind_an_unchanged_key_redraw() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        redraw.drawn(start);
        let pressed = start + Duration::from_millis(100);
        let echoed = pressed + Duration::from_millis(1);

        let effects = redraw.update(&mut app, key(KeyCode::Char('a')), at(100));
        assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b"a".to_vec()) }]);
        // Exactly what the event loop does between receiving a key and its echo.
        if redraw.draw_in(pressed) == Some(Duration::ZERO) {
            redraw.drawn(pressed);
        }
        redraw.update(&mut app, frame(1, b"a"), at(101));

        assert_eq!(app.open.as_ref().unwrap().screen.text(), "a");
        assert_eq!(redraw.draw_in(echoed), Some(Duration::ZERO), "the echo must not wait another 15 ms");
    }

    #[test]
    fn a_paused_agent_space_is_forwarded_immediately_without_redrawing() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        redraw.drawn(start);

        let effects = redraw.update(&mut app, key(KeyCode::Char(' ')), at(100));

        assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b" ".to_vec()) }]);
        assert!(!app.hold.busy());
        assert_eq!(redraw.draw_in(start + Duration::from_millis(100)), None);
    }

    #[test]
    fn forwarded_keys_preserve_a_pending_frame_and_streams_keep_the_frame_limit() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        redraw.drawn(start);
        let mut draws = 0;
        for ms in 100..=300 {
            let now = start + Duration::from_millis(ms);
            let bytes = format!("\x1b[1;1Hframe {ms}\x1b[K");
            redraw.update(&mut app, frame(ms, bytes.as_bytes()), at(ms));
            let effects = redraw.update(&mut app, key(KeyCode::Char('a')), at(ms));
            assert_eq!(effects.len(), 1);
            assert!(redraw.draw_in(now).is_some(), "forwarding a key must not clear a pending frame");
            if redraw.draw_in(now) == Some(Duration::ZERO) {
                redraw.drawn(now);
                draws += 1;
            }
        }
        assert_eq!(app.open.as_ref().unwrap().screen.text(), "frame 300", "no ANSI frame may be dropped");
        assert_eq!(draws, 13, "at most one draw per 16 ms, even with keys between frames");
        assert_eq!(redraw.draw_in(start + Duration::from_millis(300)), Some(Duration::from_millis(8)));
    }

    #[test]
    fn settling_a_space_does_not_hide_a_focus_change() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        app.config.speech.space_hold = Some(true);
        redraw.drawn(start);
        redraw.update(&mut app, key(KeyCode::Char(' ')), at(100));
        redraw.drawn(start + Duration::from_millis(100));

        let effects = redraw.update(&mut app, key(KeyCode::Tab), at(120));

        assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b" ".to_vec()) }]);
        assert_eq!(app.focus, Focus::List);
        assert_eq!(redraw.draw_in(start + Duration::from_millis(120)), Some(Duration::ZERO));
    }

    #[test]
    fn settling_a_space_does_not_hide_the_dictation_menu() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        app.config.speech.space_hold = Some(true);
        redraw.drawn(start);
        redraw.update(&mut app, key(KeyCode::Char(' ')), at(100));
        redraw.drawn(start + Duration::from_millis(100));
        let ctrl_t = Input::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));

        let effects = redraw.update(&mut app, ctrl_t, at(120));

        assert_eq!(effects, vec![Effect::Send { generation: 1, control: Control::Input(b" ".to_vec()) }]);
        assert!(app.menu.is_some());
        assert_eq!(redraw.draw_in(start + Duration::from_millis(120)), Some(Duration::ZERO));
    }

    #[test]
    fn focus_changes_and_resizes_still_redraw() {
        let start = Instant::now();
        let mut redraw = Redraw::new(start);
        let mut app = app(Focus::Terminal);
        redraw.drawn(start);

        redraw.update(&mut app, Input::Resize { width: 140, height: 50 }, at(100));
        assert_eq!(redraw.draw_in(start + Duration::from_millis(100)), Some(Duration::ZERO));
        redraw.drawn(start + Duration::from_millis(100));
        redraw.update(&mut app, key(KeyCode::Tab), at(120));
        assert_eq!(app.focus, Focus::List);
        assert_eq!(redraw.draw_in(start + Duration::from_millis(120)), Some(Duration::ZERO));
    }
}
