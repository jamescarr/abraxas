//! The TERMINUS 3.5 dialing directory.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{DialingScreen, Nav, Screen, ScreenImpl};
use crate::palette::Dos;
use crate::ui::app::{Ctx, Overlay};
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};

/// Who the player can dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialTarget {
    /// Wren's board.
    NightShift,
    /// The board that should not exist; dials `5550000`.
    Abraxas,
    /// Always busy.
    Library,
}

struct Entry {
    /// The number key (`'1'`..`'3'`).
    key: char,
    name: &'static str,
    number: &'static str,
    baud: &'static str,
    last_call: &'static str,
    target: DialTarget,
}

const ENTRIES: [Entry; 3] = [
    Entry {
        key: '1',
        name: "NIGHT SHIFT BBS (Wren)",
        number: "555-0187",
        baud: "28800",
        last_call: "12/10/99",
        target: DialTarget::NightShift,
    },
    Entry {
        key: '2',
        name: "ABRAXAS",
        number: "???-????",
        baud: "28800",
        last_call: "??/??/??",
        target: DialTarget::Abraxas,
    },
    Entry {
        key: '3',
        name: "Port Hecate Public Library",
        number: "555-0100",
        baud: "14400",
        last_call: "--/--/--",
        target: DialTarget::Library,
    },
];

const READY: &str = "HALCYON 28.8 FAXMODEM on COM2. Modem ready.";

pub struct DialerScreen {
    /// Indices into `ENTRIES` that are listed.
    visible: Vec<usize>,
    selected: usize,
    message: String,
}

impl DialerScreen {
    /// Entry 2 (ABRAXAS) is listed only when a save or a caller record exists.
    pub fn new(ctx: &mut Ctx) -> Self {
        let show_abraxas = ctx.store.has_any_save_or_caller();
        let visible = ENTRIES
            .iter()
            .enumerate()
            .filter(|(_, e)| show_abraxas || e.target != DialTarget::Abraxas)
            .map(|(i, _)| i)
            .collect();
        Self {
            visible,
            selected: 0,
            message: READY.to_string(),
        }
    }

    /// The listed targets, top to bottom.
    pub fn targets(&self) -> Vec<DialTarget> {
        self.visible.iter().map(|&i| ENTRIES[i].target).collect()
    }

    /// The highlighted target.
    pub fn selected_target(&self) -> DialTarget {
        ENTRIES[self.visible[self.selected]].target
    }

    /// The modem line shown in the panel.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn dial(&mut self, index: usize, ctx: &mut Ctx) -> Nav {
        self.selected = index;
        match ENTRIES[self.visible[index]].target {
            DialTarget::Library => {
                self.message = "ATDT5550100  BUSY".to_string();
                Nav::Stay
            }
            target => Nav::To(Box::new(Screen::Dialing(DialingScreen::new(ctx, target)))),
        }
    }
}

impl ScreenImpl for DialerScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match classify(&key) {
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                Nav::Stay
            }
            Key::Down => {
                self.selected = (self.selected + 1).min(self.visible.len() - 1);
                Nav::Stay
            }
            Key::Enter => {
                let i = self.selected;
                self.dial(i, ctx)
            }
            Key::Esc => {
                ctx.overlay_request = Some(Overlay::ConfirmQuit);
                Nav::Stay
            }
            Key::Char(c) => match self.visible.iter().position(|&i| ENTRIES[i].key == c) {
                Some(i) => self.dial(i, ctx),
                None => Nav::Stay,
            },
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let w = area.width;
        let title_bar = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), title_bar);
        put_str(
            buf,
            area.x,
            area.y,
            " TERMINUS 3.5 ─ Dialing Directory",
            w,
            title_bar,
        );
        put_str(
            buf,
            area.x + w - 30,
            area.y,
            "HALCYON 28.8 FAXMODEM  COM2 ",
            30,
            title_bar,
        );

        let head = p.style(Dos::LightCyan, Dos::Black);
        put_str(
            buf,
            area.x,
            area.y + 2,
            " #  Name                        Number      Baud    Last call",
            w,
            head,
        );
        put_str(
            buf,
            area.x,
            area.y + 3,
            &"─".repeat(usize::from(w)),
            w,
            ctx.fg(Dos::DarkGray),
        );
        for (row, &i) in self.visible.iter().enumerate() {
            let e = &ENTRIES[i];
            let y = area.y + 4 + u16::try_from(row).unwrap_or(0) * 2;
            let line = format!(
                " {}  {:<28}{:<12}{:<8}{}",
                e.key, e.name, e.number, e.baud, e.last_call
            );
            let style = if row == self.selected {
                p.style(Dos::Black, Dos::LightGray)
            } else if e.target == DialTarget::Library {
                p.style(Dos::LightGray, Dos::Black)
            } else {
                p.style(Dos::White, Dos::Black)
            };
            let line = format!("{line:<width$}", width = usize::from(w));
            put_str(buf, area.x, y, &line, w, style);
        }

        let panel = Rect::new(area.x + 2, area.y + 12, w - 4, 5);
        let inner = boxed(
            buf,
            panel,
            Some("Modem"),
            BoxKind::Single,
            p.style(Dos::LightGray, Dos::Black),
        );
        put_str(
            buf,
            inner.x + 1,
            inner.y + 1,
            &self.message,
            inner.width.saturating_sub(2),
            ctx.fg(Dos::LightGreen),
        );

        let bar = p.style(Dos::Black, Dos::Cyan);
        let hint = " ↑↓ Select   Enter Dial   1-3 Quick dial   F1 Help   Esc Hang up";
        fill(buf, Rect::new(area.x, area.y + 24, w, 1), bar);
        put_str(buf, area.x, area.y + 24, hint, w, bar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::save::{CallerRecord, CallerStatus};
    use crate::ui::app::test_ctx;
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn abraxas_is_listed_only_when_a_save_or_caller_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let fresh = DialerScreen::new(&mut ctx);
        assert_eq!(
            fresh.targets(),
            vec![DialTarget::NightShift, DialTarget::Library]
        );
        ctx.store
            .record_caller(CallerRecord {
                handle: "tester".into(),
                depth: 1,
                clock: "23:04:00".into(),
                status: CallerStatus::Online,
            })
            .unwrap();
        let known = DialerScreen::new(&mut ctx);
        assert_eq!(
            known.targets(),
            vec![
                DialTarget::NightShift,
                DialTarget::Abraxas,
                DialTarget::Library
            ]
        );
    }

    #[test]
    fn hotkey_two_does_nothing_while_abraxas_is_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut d = DialerScreen::new(&mut ctx);
        assert!(matches!(d.handle_key(press_char('2'), &mut ctx), Nav::Stay));
        assert_eq!(d.selected_target(), DialTarget::NightShift);
        let nav = d.handle_key(press_char('1'), &mut ctx);
        assert!(matches!(nav, Nav::To(s) if s.name() == "Dialing"));
    }

    #[test]
    fn library_is_busy_and_stays_on_the_dialer() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut d = DialerScreen::new(&mut ctx);
        assert!(matches!(d.handle_key(press_char('3'), &mut ctx), Nav::Stay));
        assert_eq!(d.message(), "ATDT5550100  BUSY");
        assert_eq!(d.selected_target(), DialTarget::Library);
    }

    #[test]
    fn arrows_and_enter_dial_the_highlighted_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.store
            .record_caller(CallerRecord {
                handle: "tester".into(),
                depth: 0,
                clock: "23:00:00".into(),
                status: CallerStatus::Online,
            })
            .unwrap();
        let mut d = DialerScreen::new(&mut ctx);
        d.handle_key(press(KeyCode::Up), &mut ctx);
        assert_eq!(d.selected_target(), DialTarget::NightShift);
        d.handle_key(press(KeyCode::Down), &mut ctx);
        assert_eq!(d.selected_target(), DialTarget::Abraxas);
        d.handle_key(press(KeyCode::Down), &mut ctx);
        d.handle_key(press(KeyCode::Down), &mut ctx);
        assert_eq!(d.selected_target(), DialTarget::Library);
        d.handle_key(press(KeyCode::Up), &mut ctx);
        match d.handle_key(press(KeyCode::Enter), &mut ctx) {
            Nav::To(s) => match *s {
                Screen::Dialing(dialing) => assert_eq!(dialing.target, DialTarget::Abraxas),
                other => panic!("went to {}", other.name()),
            },
            _ => panic!("Enter should dial"),
        }
    }

    #[test]
    fn esc_asks_before_hanging_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut d = DialerScreen::new(&mut ctx);
        assert!(matches!(
            d.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Stay
        ));
        assert_eq!(ctx.overlay_request, Some(Overlay::ConfirmQuit));
    }

    #[test]
    fn renders_the_directory_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let d = DialerScreen::new(&mut ctx);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| d.render(f, f.area(), &ctx)).unwrap();
        let buf = term.backend().buffer();
        let a = buf.area;
        assert!(row_text(buf, a, 0).starts_with(" TERMINUS 3.5 ─ Dialing Directory"));
        let r4 = row_text(buf, a, 4);
        assert!(r4.contains("NIGHT SHIFT BBS (Wren)"), "{r4:?}");
        assert!(r4.contains("555-0187") && r4.contains("28800") && r4.contains("12/10/99"));
        assert!(row_text(buf, a, 6).contains("Port Hecate Public Library"));
        assert!(row_text(buf, a, 6).contains("555-0100"));
        let all: String = (0..25).map(|y| row_text(buf, a, y)).collect();
        assert!(!all.contains("ABRAXAS"), "entry 2 must stay hidden");
    }
}
