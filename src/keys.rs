//! Encodes key presses as the bytes a terminal application expects, the way
//! xterm does. Herdr's terminal session takes raw input, so the inbox does the
//! encoding itself.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// The pane state that changes how keys are encoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: arrows and Home/End send `ESC O x` instead of `ESC [ x`.
    pub application_cursor: bool,
    pub bracketed_paste: bool,
}

/// xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
fn modifier_param(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(mods.contains(KeyModifiers::ALT))
        + 4 * u8::from(mods.contains(KeyModifiers::CONTROL))
}

fn csi_letter(letter: char, mods: KeyModifiers, application: bool) -> Vec<u8> {
    let param = modifier_param(mods);
    if param > 1 {
        format!("\x1b[1;{param}{letter}").into_bytes()
    } else if application {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[{letter}").into_bytes()
    }
}

fn csi_tilde(number: u8, mods: KeyModifiers) -> Vec<u8> {
    let param = modifier_param(mods);
    if param > 1 {
        format!("\x1b[{number};{param}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn alt(mut bytes: Vec<u8>, mods: KeyModifiers) -> Vec<u8> {
    if mods.contains(KeyModifiers::ALT) {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// The control byte for Ctrl+`c`, if the key has one.
fn control_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Bytes for one key event, or `None` for keys a terminal never receives
/// (releases, bare modifiers, media keys).
pub fn encode(key: KeyEvent, modes: Modes) -> Option<Vec<u8>> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => alt(vec![control_byte(c)?], mods),
        KeyCode::Char(c) => {
            let mut buffer = [0; 4];
            alt(c.encode_utf8(&mut buffer).as_bytes().to_vec(), mods)
        }
        // Shift+Enter and Alt+Enter insert a newline in agent CLIs that
        // accept ESC CR, which is how most of them read it without the kitty
        // keyboard protocol.
        KeyCode::Enter if mods.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => b"\x1b\r".to_vec(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Tab => alt(b"\t".to_vec(), mods),
        KeyCode::Backspace if ctrl => alt(vec![0x08], mods),
        KeyCode::Backspace => alt(vec![0x7f], mods),
        KeyCode::Esc => alt(vec![0x1b], mods),
        KeyCode::Up => csi_letter('A', mods, modes.application_cursor),
        KeyCode::Down => csi_letter('B', mods, modes.application_cursor),
        KeyCode::Right => csi_letter('C', mods, modes.application_cursor),
        KeyCode::Left => csi_letter('D', mods, modes.application_cursor),
        KeyCode::Home => csi_letter('H', mods, modes.application_cursor),
        KeyCode::End => csi_letter('F', mods, modes.application_cursor),
        KeyCode::Insert => csi_tilde(2, mods),
        KeyCode::Delete => csi_tilde(3, mods),
        KeyCode::PageUp => csi_tilde(5, mods),
        KeyCode::PageDown => csi_tilde(6, mods),
        KeyCode::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            let param = modifier_param(mods);
            if param > 1 {
                format!("\x1b[1;{param}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        KeyCode::F(n) => {
            let number = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            csi_tilde(number, mods)
        }
        _ => return None,
    };
    Some(bytes)
}

/// Bytes for pasted text: bracketed when the application asked for it, with
/// any end marker inside the text removed so a paste cannot end itself early.
pub fn paste(text: &str, modes: Modes) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        let inner = text.replace("\x1b[201~", "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventState;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    fn plain(code: KeyCode) -> Vec<u8> {
        encode(key(code, KeyModifiers::NONE), Modes::default()).unwrap()
    }

    fn with(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode(key(code, mods), Modes::default()).unwrap()
    }

    #[test]
    fn printable_characters_are_utf8() {
        assert_eq!(plain(KeyCode::Char('a')), b"a");
        assert_eq!(with(KeyCode::Char('A'), KeyModifiers::SHIFT), b"A");
        assert_eq!(plain(KeyCode::Char('é')), "é".as_bytes());
        assert_eq!(plain(KeyCode::Char('🙂')), "🙂".as_bytes());
    }

    #[test]
    fn control_letters_map_to_c0_codes_regardless_of_case() {
        assert_eq!(with(KeyCode::Char('a'), KeyModifiers::CONTROL), [0x01]);
        assert_eq!(with(KeyCode::Char('c'), KeyModifiers::CONTROL), [0x03]);
        assert_eq!(with(KeyCode::Char('Z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), [0x1a]);
    }

    #[test]
    fn control_punctuation_follows_xterm() {
        assert_eq!(with(KeyCode::Char(' '), KeyModifiers::CONTROL), [0x00]);
        assert_eq!(with(KeyCode::Char('['), KeyModifiers::CONTROL), [0x1b]);
        assert_eq!(with(KeyCode::Char('\\'), KeyModifiers::CONTROL), [0x1c]);
        assert_eq!(with(KeyCode::Char(']'), KeyModifiers::CONTROL), [0x1d]);
        assert_eq!(with(KeyCode::Char('_'), KeyModifiers::CONTROL), [0x1f]);
        assert_eq!(with(KeyCode::Char('?'), KeyModifiers::CONTROL), [0x7f]);
    }

    #[test]
    fn control_with_a_key_that_has_no_code_sends_nothing() {
        assert_eq!(encode(key(KeyCode::Char('1'), KeyModifiers::CONTROL), Modes::default()), None);
        assert_eq!(encode(key(KeyCode::Char('é'), KeyModifiers::CONTROL), Modes::default()), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(with(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(with(KeyCode::Char('x'), KeyModifiers::ALT | KeyModifiers::CONTROL), [0x1b, 0x18]);
        assert_eq!(with(KeyCode::Backspace, KeyModifiers::ALT), [0x1b, 0x7f]);
    }

    #[test]
    fn enter_tab_backspace_escape() {
        assert_eq!(plain(KeyCode::Enter), b"\r");
        assert_eq!(with(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r");
        assert_eq!(with(KeyCode::Enter, KeyModifiers::ALT), b"\x1b\r");
        assert_eq!(plain(KeyCode::Tab), b"\t");
        assert_eq!(with(KeyCode::Tab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(with(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(plain(KeyCode::Backspace), [0x7f]);
        assert_eq!(with(KeyCode::Backspace, KeyModifiers::CONTROL), [0x08]);
        assert_eq!(plain(KeyCode::Esc), [0x1b]);
    }

    #[test]
    fn arrows_respect_application_cursor_mode_unless_modified() {
        assert_eq!(plain(KeyCode::Up), b"\x1b[A");
        assert_eq!(plain(KeyCode::Left), b"\x1b[D");
        let app = Modes { application_cursor: true, ..Modes::default() };
        assert_eq!(encode(key(KeyCode::Up, KeyModifiers::NONE), app).unwrap(), b"\x1bOA");
        assert_eq!(encode(key(KeyCode::End, KeyModifiers::NONE), app).unwrap(), b"\x1bOF");
        assert_eq!(encode(key(KeyCode::Right, KeyModifiers::CONTROL), app).unwrap(), b"\x1b[1;5C");
    }

    #[test]
    fn modifier_parameters_combine() {
        assert_eq!(with(KeyCode::Up, KeyModifiers::SHIFT), b"\x1b[1;2A");
        assert_eq!(with(KeyCode::Up, KeyModifiers::ALT), b"\x1b[1;3A");
        assert_eq!(with(KeyCode::Up, KeyModifiers::CONTROL), b"\x1b[1;5A");
        assert_eq!(with(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::ALT), b"\x1b[1;8A");
        assert_eq!(with(KeyCode::Delete, KeyModifiers::CONTROL), b"\x1b[3;5~");
    }

    #[test]
    fn editing_and_paging_keys() {
        assert_eq!(plain(KeyCode::Home), b"\x1b[H");
        assert_eq!(plain(KeyCode::End), b"\x1b[F");
        assert_eq!(plain(KeyCode::Insert), b"\x1b[2~");
        assert_eq!(plain(KeyCode::Delete), b"\x1b[3~");
        assert_eq!(plain(KeyCode::PageUp), b"\x1b[5~");
        assert_eq!(plain(KeyCode::PageDown), b"\x1b[6~");
    }

    #[test]
    fn function_keys_follow_xterm() {
        assert_eq!(plain(KeyCode::F(1)), b"\x1bOP");
        assert_eq!(plain(KeyCode::F(4)), b"\x1bOS");
        assert_eq!(with(KeyCode::F(2), KeyModifiers::SHIFT), b"\x1b[1;2Q");
        assert_eq!(plain(KeyCode::F(5)), b"\x1b[15~");
        assert_eq!(plain(KeyCode::F(6)), b"\x1b[17~");
        assert_eq!(plain(KeyCode::F(10)), b"\x1b[21~");
        assert_eq!(plain(KeyCode::F(11)), b"\x1b[23~");
        assert_eq!(plain(KeyCode::F(12)), b"\x1b[24~");
        assert_eq!(with(KeyCode::F(5), KeyModifiers::CONTROL), b"\x1b[15;5~");
        assert_eq!(encode(key(KeyCode::F(13), KeyModifiers::NONE), Modes::default()), None);
    }

    #[test]
    fn releases_and_unencodable_keys_send_nothing() {
        let mut release = key(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(encode(release, Modes::default()), None);
        assert_eq!(encode(key(KeyCode::CapsLock, KeyModifiers::NONE), Modes::default()), None);
        assert_eq!(encode(key(KeyCode::Null, KeyModifiers::NONE), Modes::default()), None);
    }

    #[test]
    fn repeats_are_encoded_like_presses() {
        let mut repeat = key(KeyCode::Char('j'), KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(encode(repeat, Modes::default()).unwrap(), b"j");
    }

    #[test]
    fn paste_is_bracketed_only_when_asked_and_cannot_close_itself() {
        assert_eq!(paste("a\nb", Modes::default()), b"a\rb");
        assert_eq!(paste("a\r\nb", Modes::default()), b"a\rb");
        let bracketed = Modes { bracketed_paste: true, ..Modes::default() };
        assert_eq!(paste("hi", bracketed), b"\x1b[200~hi\x1b[201~");
        assert_eq!(paste("x\x1b[201~rm -rf", bracketed), b"\x1b[200~xrm -rf\x1b[201~");
    }
}
