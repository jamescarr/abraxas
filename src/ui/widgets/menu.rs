//! The BBS hotkey menu: `[M] Message bases` entries in one or more columns.
//!
//! Items fill row-major (`columns = 2`: item 0 left, item 1 right, item 2 below item 0, ...).
//! Arrow keys move the highlight, Enter activates it, and a letter or digit activates the item
//! with that hotkey (case-insensitive). A disabled item is drawn dark gray and reports
//! [`MenuEvent::Disabled`] instead of `Activated`, so the screen can explain why it is locked.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::palette::{Dos, Palette};
use crate::ui::canvas::put_str;
use crate::ui::keys::{Key, classify, hotkey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// The hotkey, stored lowercase.
    pub key: char,
    pub label: String,
    pub enabled: bool,
}

impl MenuItem {
    pub fn new(key: char, label: impl Into<String>) -> Self {
        Self {
            key: key.to_ascii_lowercase(),
            label: label.into(),
            enabled: true,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// What a key press did to the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEvent {
    /// Not a menu key.
    None,
    /// The highlight moved.
    Moved,
    /// An enabled item was chosen; carries its (lowercase) hotkey.
    Activated(char),
    /// A disabled item was chosen; carries its (lowercase) hotkey.
    Disabled(char),
}

#[derive(Debug, Clone)]
pub struct Menu {
    items: Vec<MenuItem>,
    selected: usize,
    columns: usize,
    row_height: u16,
}

impl Menu {
    /// A one-column menu, one row per item.
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            selected: 0,
            columns: 1,
            row_height: 1,
        }
    }

    pub fn with_columns(mut self, columns: usize) -> Self {
        self.columns = columns.max(1);
        self
    }

    /// Rows of screen height per menu row (2 leaves a blank line between rows).
    pub fn with_row_height(mut self, rows: u16) -> Self {
        self.row_height = rows.max(1);
        self
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// Index of the highlighted item.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the highlight to the item with `key`, if there is one.
    pub fn select_key(&mut self, key: char) -> bool {
        let key = key.to_ascii_lowercase();
        match self.items.iter().position(|i| i.key == key) {
            Some(i) => {
                self.selected = i;
                true
            }
            None => false,
        }
    }

    pub fn set_enabled(&mut self, key: char, enabled: bool) {
        let key = key.to_ascii_lowercase();
        if let Some(i) = self.items.iter_mut().find(|i| i.key == key) {
            i.enabled = enabled;
        }
    }

    pub fn set_label(&mut self, key: char, label: impl Into<String>) {
        let key = key.to_ascii_lowercase();
        if let Some(i) = self.items.iter_mut().find(|i| i.key == key) {
            i.label = label.into();
        }
    }

    fn step(&mut self, delta: isize) -> bool {
        let len = self.items.len() as isize;
        let next = self.selected as isize + delta;
        if (0..len).contains(&next) {
            self.selected = next as usize;
            true
        } else {
            false
        }
    }

    fn event_for(&self, i: usize) -> MenuEvent {
        match self.items.get(i) {
            Some(item) if item.enabled => MenuEvent::Activated(item.key),
            Some(item) => MenuEvent::Disabled(item.key),
            None => MenuEvent::None,
        }
    }

    pub fn handle_key(&mut self, ev: &KeyEvent) -> MenuEvent {
        if self.items.is_empty() {
            return MenuEvent::None;
        }
        let cols = self.columns as isize;
        let moved = |m: bool| if m { MenuEvent::Moved } else { MenuEvent::None };
        match classify(ev) {
            Key::Up => moved(self.step(-cols)),
            Key::Down => moved(self.step(cols)),
            Key::Left if cols > 1 => moved(self.step(-1)),
            Key::Right if cols > 1 => moved(self.step(1)),
            Key::Enter => self.event_for(self.selected),
            _ => match hotkey(ev) {
                Some(c) => match self.items.iter().position(|i| i.key == c) {
                    Some(i) => {
                        self.selected = i;
                        self.event_for(i)
                    }
                    None => MenuEvent::None,
                },
                None => MenuEvent::None,
            },
        }
    }

    /// Draw the items into `area`. `[K]` is drawn in `accent`, labels light gray, the highlighted
    /// item inverse, disabled items dark gray.
    pub fn render(&self, buf: &mut Buffer, area: Rect, palette: &Palette, accent: Color) {
        if self.items.is_empty() || area.is_empty() {
            return;
        }
        let col_w = area.width / u16::try_from(self.columns).unwrap_or(1).max(1);
        for (i, item) in self.items.iter().enumerate() {
            let (row, col) = (i / self.columns, i % self.columns);
            let y = area.y + u16::try_from(row).unwrap_or(0) * self.row_height;
            if y >= area.bottom() {
                break;
            }
            let x = area.x + u16::try_from(col).unwrap_or(0) * col_w;
            let key = format!("[{}]", item.key.to_ascii_uppercase());
            let (key_style, label_style) = if !item.enabled {
                let s = palette.style(Dos::DarkGray, Dos::Black);
                (s, s)
            } else if i == self.selected {
                let s = palette.style(Dos::Black, Dos::LightGray);
                (s, s)
            } else {
                (
                    Style::new().fg(accent).bg(palette.color(Dos::Black)),
                    palette.style(Dos::LightGray, Dos::Black),
                )
            };
            let text = format!(" {}", item.label);
            let label_w = col_w.saturating_sub(3);
            put_str(buf, x, y, &key, col_w, key_style);
            // Pad the label so the inverse highlight is a solid bar.
            let padded = format!("{text:<width$}", width = usize::from(label_w));
            put_str(buf, x + 3, y, &padded, label_w, label_style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::crossterm::event::KeyCode;

    fn bbs() -> Menu {
        Menu::new(vec![
            MenuItem::new('m', "Message bases"),
            MenuItem::new('f', "File library"),
            MenuItem::new('c', "Page the SysOp"),
            MenuItem::new('u', "User list"),
            MenuItem::new('g', "Goodbye").disabled(),
        ])
        .with_columns(2)
    }

    #[test]
    fn hotkeys_are_case_insensitive_and_move_the_highlight() {
        let mut m = bbs();
        assert_eq!(m.handle_key(&press_char('F')), MenuEvent::Activated('f'));
        assert_eq!(m.selected(), 1);
        assert_eq!(m.handle_key(&press_char('c')), MenuEvent::Activated('c'));
        assert_eq!(m.handle_key(&press_char('z')), MenuEvent::None);
        assert_eq!(m.selected(), 2);
    }

    #[test]
    fn disabled_items_report_disabled_instead_of_activating() {
        let mut m = bbs();
        assert_eq!(m.handle_key(&press_char('g')), MenuEvent::Disabled('g'));
        assert_eq!(m.selected(), 4);
        assert_eq!(
            m.handle_key(&press(KeyCode::Enter)),
            MenuEvent::Disabled('g')
        );
        m.set_enabled('G', true);
        assert_eq!(
            m.handle_key(&press(KeyCode::Enter)),
            MenuEvent::Activated('g')
        );
    }

    #[test]
    fn arrows_move_by_row_and_column_within_bounds() {
        let mut m = bbs();
        assert_eq!(m.handle_key(&press(KeyCode::Right)), MenuEvent::Moved);
        assert_eq!(m.selected(), 1);
        assert_eq!(m.handle_key(&press(KeyCode::Down)), MenuEvent::Moved);
        assert_eq!(m.selected(), 3);
        // Down from item 3 would be item 5: out of range.
        assert_eq!(m.handle_key(&press(KeyCode::Down)), MenuEvent::None);
        assert_eq!(m.selected(), 3);
        assert_eq!(m.handle_key(&press(KeyCode::Up)), MenuEvent::Moved);
        assert_eq!(m.selected(), 1);
        assert_eq!(m.handle_key(&press(KeyCode::Up)), MenuEvent::None);
        assert_eq!(
            m.handle_key(&press(KeyCode::Enter)),
            MenuEvent::Activated('f')
        );
    }

    #[test]
    fn single_column_arrows_ignore_left_and_right() {
        let mut m = Menu::new(vec![MenuItem::new('a', "A"), MenuItem::new('b', "B")]);
        assert_eq!(m.handle_key(&press(KeyCode::Right)), MenuEvent::None);
        assert_eq!(m.handle_key(&press(KeyCode::Down)), MenuEvent::Moved);
        assert_eq!(m.selected(), 1);
    }

    #[test]
    fn renders_two_columns_row_major_with_row_height() {
        let m = bbs().with_row_height(2);
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        m.render(&mut buf, area, &Palette::new(false), Color::Yellow);
        let r0 = row_text(&buf, area, 0);
        assert!(r0.starts_with("[M] Message bases"), "{r0:?}");
        assert_eq!(&r0[40..58], "[F] File library  ");
        assert!(row_text(&buf, area, 1).trim().is_empty());
        assert!(row_text(&buf, area, 2).starts_with("[C] Page the SysOp"));
        assert!(row_text(&buf, area, 4).starts_with("[G] Goodbye"));
        // Disabled item is dark gray; the highlighted one is inverse.
        assert_eq!(buf[(1, 4)].fg, Palette::new(false).color(Dos::DarkGray));
        assert_eq!(buf[(5, 0)].bg, Palette::new(false).color(Dos::LightGray));
        assert_eq!(buf[(45, 0)].bg, Palette::new(false).color(Dos::Black));
    }
}
