//! Boot: the BIOS screen and `C:\> TERMINUS`, about three seconds, any key skips.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{DialerScreen, Nav, Screen, ScreenImpl};
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::put_str;

const BIOS: &str = include_str!("../../../assets/boot.txt");

/// Seconds between BIOS lines.
const LINE_STEP: f32 = 0.06;
/// Seconds the memory counter takes to reach 65536K.
const MEMORY_SECS: f32 = 0.9;
/// Memory size shown at the end of the count, in K.
const MEMORY_K: u32 = 65_536;
/// Pause between the last BIOS line and the prompt.
const PROMPT_DELAY: f32 = 0.2;
/// Seconds per typed character of the prompt.
const TYPE_STEP: f32 = 0.05;
/// Pause after the prompt is typed.
const HOLD: f32 = 0.25;
const PROMPT: &str = "C:\\> TERMINUS";

/// The Boot screen. Its output is a pure function of the elapsed time `t`.
pub struct BootScreen {
    t: f32,
    lines: Vec<&'static str>,
    memory_line: Option<usize>,
}

impl BootScreen {
    pub fn new(_ctx: &mut Ctx) -> Self {
        let mut lines: Vec<&'static str> = BIOS.lines().collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        let memory_line = lines.iter().position(|l| l.starts_with("Memory Test"));
        Self {
            t: 0.0,
            lines,
            memory_line,
        }
    }

    /// When BIOS line `i` starts appearing (the lines after the memory test wait for it).
    fn line_start(&self, i: usize) -> f32 {
        let delay = match self.memory_line {
            Some(m) if i > m => MEMORY_SECS,
            _ => 0.0,
        };
        i as f32 * LINE_STEP + delay
    }

    fn prompt_start(&self) -> f32 {
        self.line_start(self.lines.len()) + PROMPT_DELAY
    }

    /// Time at which Boot hands over to the Dialer.
    pub fn duration(&self) -> f32 {
        self.prompt_start() + PROMPT.len() as f32 * TYPE_STEP + HOLD
    }

    /// The screen's text rows at time `t`: BIOS lines revealed so far, the memory counter, then
    /// the typed prompt.
    pub fn rows_at(&self, t: f32) -> Vec<String> {
        let mut rows = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let start = self.line_start(i);
            if t < start {
                break;
            }
            if self.memory_line == Some(i) {
                let p = ((t - start) / MEMORY_SECS).clamp(0.0, 1.0);
                if p >= 1.0 {
                    rows.push(format!("{line}{MEMORY_K}K OK"));
                } else {
                    let k = ((MEMORY_K as f32 * p) as u32 / 64) * 64;
                    rows.push(format!("{line}{k}K"));
                }
            } else {
                rows.push((*line).to_string());
            }
        }
        let ps = self.prompt_start();
        if t >= ps {
            let typed = (((t - ps) / TYPE_STEP) as usize).min(PROMPT.len());
            rows.push(String::new());
            rows.push(PROMPT[..typed].to_string());
        }
        rows
    }

    fn to_dialer(ctx: &mut Ctx) -> Nav {
        Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))))
    }
}

impl ScreenImpl for BootScreen {
    fn handle_key(&mut self, _key: KeyEvent, ctx: &mut Ctx) -> Nav {
        Self::to_dialer(ctx)
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        self.t += dt.as_secs_f32();
        if self.t >= self.duration() {
            Self::to_dialer(ctx)
        } else {
            Nav::Stay
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let text = ctx.fg(Dos::LightGray);
        let bright = ctx.fg(Dos::White);
        let rows = self.rows_at(self.t);
        let last = rows.len().saturating_sub(1);
        for (i, row) in rows.iter().enumerate().take(usize::from(area.height)) {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            let style = if i == 0 || row.starts_with("C:") {
                bright
            } else {
                text
            };
            let n = put_str(buf, area.x, y, row, area.width, style);
            // A blinking block cursor after the prompt line.
            if i == last && row.starts_with("C:") && ((self.t * 2.0) as u32).is_multiple_of(2) {
                put_str(buf, area.x + n, y, "_", 1, bright);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_ctx;
    use crate::ui::keys::press_char;

    fn boot() -> (BootScreen, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        // The tempdir may vanish; Boot never touches the disk.
        let b = BootScreen::new(&mut ctx);
        (b, ctx)
    }

    #[test]
    fn the_memory_count_climbs_to_65536k_ok() {
        let (b, _ctx) = boot();
        let m = b.memory_line.unwrap();
        let start = b.line_start(m);
        let row_at = |t: f32| b.rows_at(t)[m].clone();
        assert_eq!(row_at(start), "Memory Test : 0K");
        let k_of = |row: String| -> i64 {
            row.trim_start_matches("Memory Test : ")
                .trim_end_matches('K')
                .parse()
                .unwrap()
        };
        // Float rounding may land one 64K step low.
        assert!((k_of(row_at(start + MEMORY_SECS / 2.0)) - 32_768).abs() <= 64);
        assert!((k_of(row_at(start + MEMORY_SECS * 0.25)) - 16_384).abs() <= 64);
        assert_eq!(
            row_at(start + MEMORY_SECS + 0.01),
            "Memory Test : 65536K OK"
        );
        // Lines after the memory test wait for it to finish.
        assert_eq!(b.rows_at(start + MEMORY_SECS / 2.0).len(), m + 1);
    }

    #[test]
    fn the_prompt_is_typed_last_and_the_whole_boot_takes_about_three_seconds() {
        let (b, _ctx) = boot();
        let total = b.duration();
        assert!((2.8..=3.8).contains(&total), "{total}");
        let rows = b.rows_at(total);
        assert_eq!(rows.last().unwrap(), "C:\\> TERMINUS");
        assert_eq!(rows[rows.len() - 2], "");
        assert!(rows[0].starts_with("HX-BIOS"));
        assert!(rows.len() <= 25, "fits the canvas: {}", rows.len());
        let ps = b.prompt_start();
        assert_eq!(b.rows_at(ps + 0.12).last().unwrap(), "C:");
        assert!(!b.rows_at(ps - 0.01).iter().any(|r| r.starts_with("C:")));
    }

    #[test]
    fn any_key_skips_to_the_dialer() {
        let (mut b, mut ctx) = boot();
        let nav = b.handle_key(press_char(' '), &mut ctx);
        assert!(matches!(nav, Nav::To(s) if s.name() == "Dialer"));
    }

    #[test]
    fn finishing_the_boot_goes_to_the_dialer() {
        let (mut b, mut ctx) = boot();
        let mut navs = 0;
        let mut t = 0.0;
        while t < 10.0 {
            match b.tick(Duration::from_millis(100), &mut ctx) {
                Nav::Stay => {}
                Nav::To(s) => {
                    assert_eq!(s.name(), "Dialer");
                    navs += 1;
                    break;
                }
                _ => panic!("unexpected nav"),
            }
            t += 0.1;
        }
        assert_eq!(navs, 1);
        assert!(t > 2.8 && t < 3.9, "{t}");
    }
}
