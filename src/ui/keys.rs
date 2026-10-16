//! Key-event helpers shared by every screen.
//!
//! Screens receive raw `KeyEvent`s (only `Press` events reach them). [`classify`] reduces an event
//! to a small [`Key`] enum so screens can `match` without repeating modifier checks, and
//! [`hotkey`] gives the case-insensitive menu hotkey of a plain letter or digit press.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A key press reduced to what the game cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character typed without Ctrl/Alt (case preserved).
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    PgUp,
    PgDn,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Home,
    End,
    /// Function key `F(n)`.
    F(u8),
    /// Ctrl plus a letter, lowercased (`Ctrl+L` is `Ctrl('l')`).
    Ctrl(char),
    /// Alt plus a character, lowercased.
    Alt(char),
    Other,
}

/// Reduce a key event to a [`Key`]. Space is `Char(' ')`.
pub fn classify(ev: &KeyEvent) -> Key {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    match ev.code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) if alt => Key::Alt(c.to_ascii_lowercase()),
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::PageUp => Key::PgUp,
        KeyCode::PageDown => Key::PgDn,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::F(n) => Key::F(n),
        _ => Key::Other,
    }
}

/// The lowercase hotkey of a plain letter or digit press (no Ctrl/Alt), else `None`.
pub fn hotkey(ev: &KeyEvent) -> Option<char> {
    match classify(ev) {
        Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// True for the hang-up chords: Ctrl+C, Ctrl+Q and Alt+X.
pub fn is_quit_chord(ev: &KeyEvent) -> bool {
    matches!(
        classify(ev),
        Key::Ctrl('c') | Key::Ctrl('q') | Key::Alt('x')
    )
}

/// True when this is a key press (the only kind the app handles).
pub fn is_press(ev: &KeyEvent) -> bool {
    ev.kind == KeyEventKind::Press
}

/// A plain press of `code` (for tests and synthetic input).
pub fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A plain press of the character `c`.
pub fn press_char(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

/// Ctrl plus the character `c`.
pub fn press_ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_separates_modifiers() {
        assert_eq!(classify(&press_char('a')), Key::Char('a'));
        assert_eq!(classify(&press_char('A')), Key::Char('A'));
        assert_eq!(classify(&press_ctrl('L')), Key::Ctrl('l'));
        assert_eq!(
            classify(&KeyEvent::new(KeyCode::Char('X'), KeyModifiers::ALT)),
            Key::Alt('x')
        );
        assert_eq!(classify(&press(KeyCode::PageDown)), Key::PgDn);
        assert_eq!(classify(&press(KeyCode::F(1))), Key::F(1));
    }

    #[test]
    fn hotkeys_are_case_insensitive_and_exclude_chords() {
        assert_eq!(hotkey(&press_char('M')), Some('m'));
        assert_eq!(hotkey(&press_char('3')), Some('3'));
        assert_eq!(hotkey(&press_char(' ')), None);
        assert_eq!(hotkey(&press_ctrl('m')), None);
        assert_eq!(hotkey(&press(KeyCode::Enter)), None);
    }

    #[test]
    fn quit_chords() {
        assert!(is_quit_chord(&press_ctrl('c')));
        assert!(is_quit_chord(&press_ctrl('q')));
        assert!(is_quit_chord(&KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::ALT
        )));
        assert!(!is_quit_chord(&press_char('q')));
        assert!(!is_quit_chord(&KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE
        )));
    }
}
