//! A single-line text input: cursor editing, a length cap, an optional mask and a prompt.
//!
//! Characters outside CP437 are refused so the screen never has to draw glyphs it cannot show.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::text::is_cp437;
use crate::ui::canvas::put_str;
use crate::ui::keys::{Key, classify};

/// What a key press did to the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// Nothing relevant (or an edit that changed nothing).
    None,
    /// The text or cursor changed.
    Changed,
    /// Enter was pressed; carries the text (the input keeps it; call `clear` to reset).
    Submit(String),
    /// Esc was pressed.
    Cancel,
}

#[derive(Debug, Clone)]
pub struct TextInput {
    chars: Vec<char>,
    cursor: usize,
    max_len: usize,
    mask: Option<char>,
    prompt: String,
}

impl TextInput {
    /// An empty input showing `prompt` before the text, accepting up to `max_len` characters.
    pub fn new(prompt: impl Into<String>, max_len: usize) -> Self {
        Self {
            chars: Vec::new(),
            cursor: 0,
            max_len,
            mask: None,
            prompt: prompt.into(),
        }
    }

    /// Draw every character as `mask` (for secrets).
    pub fn with_mask(mut self, mask: char) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn value(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// Cursor position in characters from the start.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn set_prompt(&mut self, prompt: impl Into<String>) {
        self.prompt = prompt.into();
    }

    /// Replace the text (truncated to the cap) and put the cursor at the end.
    pub fn set_value(&mut self, text: &str) {
        self.chars = text
            .chars()
            .filter(|c| is_cp437(*c))
            .take(self.max_len)
            .collect();
        self.cursor = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> InputEvent {
        match classify(ev) {
            Key::Enter => InputEvent::Submit(self.value()),
            Key::Esc => InputEvent::Cancel,
            Key::Left => self.move_to(self.cursor.saturating_sub(1)),
            Key::Right => self.move_to((self.cursor + 1).min(self.chars.len())),
            Key::Home => self.move_to(0),
            Key::End => self.move_to(self.chars.len()),
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
                InputEvent::Changed
            }
            Key::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
                InputEvent::Changed
            }
            Key::Char(c) if !c.is_control() && is_cp437(c) && self.chars.len() < self.max_len => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                InputEvent::Changed
            }
            _ => InputEvent::None,
        }
    }

    fn move_to(&mut self, to: usize) -> InputEvent {
        if to == self.cursor {
            InputEvent::None
        } else {
            self.cursor = to;
            InputEvent::Changed
        }
    }

    /// The text as drawn (masked when a mask is set).
    fn shown(&self) -> Vec<char> {
        match self.mask {
            Some(m) => vec![m; self.chars.len()],
            None => self.chars.clone(),
        }
    }

    /// Draw `prompt + text` on the first row of `area` with an inverse block cursor. Long text
    /// scrolls horizontally so the cursor stays visible.
    pub fn render(&self, buf: &mut Buffer, area: Rect, style: Style) {
        if area.is_empty() {
            return;
        }
        let prompt_w = u16::try_from(self.prompt.chars().count()).unwrap_or(u16::MAX);
        let used = put_str(buf, area.x, area.y, &self.prompt, area.width, style);
        let room = usize::from(area.width.saturating_sub(prompt_w.min(area.width)));
        if room == 0 {
            return;
        }
        let shown = self.shown();
        // Keep the cursor cell inside `room`: one extra cell is needed when it sits at the end.
        let first = (self.cursor + 1).saturating_sub(room);
        let text: String = shown.iter().skip(first).take(room).collect();
        put_str(buf, area.x + used, area.y, &text, area.width, style);
        let cx = area.x + used + u16::try_from(self.cursor - first).unwrap_or(0);
        if cx < area.right()
            && let Some(cell) = buf.cell_mut((cx, area.y))
        {
            let fg = style.fg;
            let bg = style.bg;
            cell.set_style(style.fg(bg.unwrap_or_default()).bg(fg.unwrap_or_default()));
            if self.cursor >= shown.len() {
                cell.set_symbol(" ");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    fn type_str(i: &mut TextInput, s: &str) {
        for c in s.chars() {
            i.handle_key(&press_char(c));
        }
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_respects_the_cap() {
        let mut i = TextInput::new("Handle: ", 5);
        type_str(&mut i, "abcdefg");
        assert_eq!(i.value(), "abcde");
        i.handle_key(&press(KeyCode::Left));
        i.handle_key(&press(KeyCode::Left));
        assert_eq!(
            i.handle_key(&press_char('x')),
            InputEvent::None,
            "at the cap"
        );
        i.handle_key(&press(KeyCode::Backspace));
        type_str(&mut i, "X");
        assert_eq!(i.value(), "abXde");
        assert_eq!(i.cursor(), 3);
    }

    #[test]
    fn home_end_delete_and_backspace() {
        let mut i = TextInput::new("", 20);
        type_str(&mut i, "hello");
        i.handle_key(&press(KeyCode::Home));
        assert_eq!(i.cursor(), 0);
        assert_eq!(i.handle_key(&press(KeyCode::Backspace)), InputEvent::None);
        i.handle_key(&press(KeyCode::Delete));
        assert_eq!(i.value(), "ello");
        i.handle_key(&press(KeyCode::End));
        assert_eq!(i.handle_key(&press(KeyCode::Delete)), InputEvent::None);
        i.handle_key(&press(KeyCode::Backspace));
        assert_eq!(i.value(), "ell");
        assert_eq!(i.handle_key(&press(KeyCode::Right)), InputEvent::None);
    }

    #[test]
    fn enter_submits_and_esc_cancels() {
        let mut i = TextInput::new("", 20);
        type_str(&mut i, "tester");
        assert_eq!(
            i.handle_key(&press(KeyCode::Enter)),
            InputEvent::Submit("tester".into())
        );
        assert_eq!(i.value(), "tester");
        assert_eq!(i.handle_key(&press(KeyCode::Esc)), InputEvent::Cancel);
        i.clear();
        assert!(i.is_empty());
    }

    #[test]
    fn refuses_controls_and_non_cp437() {
        let mut i = TextInput::new("", 20);
        assert_eq!(i.handle_key(&press_char('\u{4e2d}')), InputEvent::None);
        assert_eq!(i.handle_key(&press_char('\n')), InputEvent::None);
        assert_eq!(i.handle_key(&press_char('é')), InputEvent::Changed);
        i.set_value("a\u{4e2d}b");
        assert_eq!(i.value(), "ab");
    }

    #[test]
    fn masked_input_never_draws_the_text() {
        let mut i = TextInput::new("Pw: ", 10).with_mask('*');
        type_str(&mut i, "secret");
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        i.render(
            &mut buf,
            area,
            Style::new().fg(Color::White).bg(Color::Black),
        );
        assert_eq!(row_text(&buf, area, 0).trim_end(), "Pw: ******");
        assert_eq!(i.value(), "secret");
    }

    #[test]
    fn long_text_scrolls_to_keep_the_cursor_visible() {
        let mut i = TextInput::new("> ", 50);
        type_str(&mut i, "0123456789abcdef");
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        i.render(
            &mut buf,
            area,
            Style::new().fg(Color::White).bg(Color::Black),
        );
        // 8 cells for text, the last holding the cursor: the tail "9abcdef" + cursor.
        assert_eq!(row_text(&buf, area, 0), "> 9abcdef ");
        // The cursor cell is inverse.
        assert_eq!(buf[(9, 0)].bg, Color::White);
        i.handle_key(&press(KeyCode::Home));
        let mut buf = Buffer::empty(area);
        i.render(
            &mut buf,
            area,
            Style::new().fg(Color::White).bg(Color::Black),
        );
        assert_eq!(row_text(&buf, area, 0), "> 01234567");
    }
}
