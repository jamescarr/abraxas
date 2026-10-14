//! PLACEHOLDER(wave C): replaced by the screen's owner; keep the struct name and `new` signature.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl, render_placeholder};
use crate::ui::app::Ctx;
use crate::ui::keys::{Key, classify};

#[derive(Debug)]
pub struct BoardScreen;

impl BoardScreen {
    pub fn new(_ctx: &mut Ctx) -> Self {
        Self
    }
}

impl ScreenImpl for BoardScreen {
    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Nav {
        match classify(&key) {
            Key::Esc => Nav::Back,
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, _ctx: &Ctx) {
        render_placeholder(frame, area, "board");
    }
}
