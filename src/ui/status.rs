//! The status bar (canvas row 24) and the modem lights it shows.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::director::DirectorEvent;
use crate::palette::Dos;
use crate::text::truncate;
use crate::ui::app::Ctx;
use crate::ui::canvas::{WIDTH, put_str};

/// How long RD stays lit after a received chunk.
pub const RD_HOLD: Duration = Duration::from_millis(120);

/// State of the CD (carrier detect) light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    /// No call has been made (offline mode, or nothing yet): dark gray.
    Offline,
    /// The last call succeeded: green.
    Ok,
    /// The last call failed: red.
    Failed,
}

/// The SD / RD / CD modem lights.
///
/// SD is lit while any request is outstanding: `Request` raises the count and `CallOk` /
/// `CallFailed` / `CallCancelled` lower it (the director ends every request with one of them).
/// RD is lit for [`RD_HOLD`] after each `Chunk`. CD keeps the latest result: a call's, or red
/// when a job falls back to its offline result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModemLights {
    outstanding: u32,
    rd_left: Duration,
    carrier: Carrier,
}

impl Default for ModemLights {
    fn default() -> Self {
        Self::new()
    }
}

impl ModemLights {
    pub fn new() -> Self {
        Self {
            outstanding: 0,
            rd_left: Duration::ZERO,
            carrier: Carrier::Offline,
        }
    }

    /// Feed a director event; only `Request`, `Chunk`, the call results and `Fallback` matter.
    pub fn on_event(&mut self, ev: &DirectorEvent) {
        match ev {
            DirectorEvent::Request { .. } => self.outstanding += 1,
            DirectorEvent::Chunk => self.rd_left = RD_HOLD,
            DirectorEvent::CallOk => {
                self.outstanding = self.outstanding.saturating_sub(1);
                self.carrier = Carrier::Ok;
            }
            DirectorEvent::CallFailed => {
                self.outstanding = self.outstanding.saturating_sub(1);
                self.carrier = Carrier::Failed;
            }
            DirectorEvent::CallCancelled => {
                self.outstanding = self.outstanding.saturating_sub(1);
            }
            DirectorEvent::Fallback => self.carrier = Carrier::Failed,
            _ => {}
        }
    }

    /// Let time pass (RD fades after [`RD_HOLD`]).
    pub fn tick(&mut self, dt: Duration) {
        self.rd_left = self.rd_left.saturating_sub(dt);
    }

    pub fn sd_on(&self) -> bool {
        self.outstanding > 0
    }

    pub fn rd_on(&self) -> bool {
        !self.rd_left.is_zero()
    }

    pub fn carrier(&self) -> Carrier {
        self.carrier
    }
}

/// Width of the trailing ` SD RD CD ` block.
const LIGHTS_WIDTH: usize = 10;

/// The full status line text, padded to 80 columns: ` {BBS<=12} │ L{dd} {title} │ EGO {nnn} │
/// {kkkk}K │ {hh:mm:ss} {mm/dd/yy} │ SD RD CD `. The title is truncated so the line fits 80
/// columns. Before a place exists the BBS shows `ABRAXAS` and the title `...`. Returns an empty
/// string when there is no run.
pub fn status_text(ctx: &Ctx) -> String {
    let Some(run) = ctx.run_ref() else {
        return String::new();
    };
    let bbs = ctx.place().map_or_else(
        || "ABRAXAS".to_string(),
        |p| p.bbs_name.chars().take(12).collect::<String>(),
    );
    let title = ctx
        .place()
        .map_or_else(|| "...".to_string(), |p| p.title.clone());
    let clock = ctx.clock().map_or_else(
        || "00:00:00 12/31/99".to_string(),
        |c| format!("{} {}", c.hms(), c.date),
    );
    let head = format!(" {bbs} │ L{:02} ", run.depth);
    let tail = format!(
        " │ EGO {:03} │ {:04}K │ {clock} │",
        run.stats.ego, run.stats.ratio_kb
    );
    let fixed = head.chars().count() + tail.chars().count() + LIGHTS_WIDTH;
    let budget = usize::from(WIDTH).saturating_sub(fixed);
    let mut line = format!("{head}{}{tail} SD RD CD ", truncate(&title, budget));
    let len = line.chars().count();
    if len < usize::from(WIDTH) {
        // Keep the lights flush right: pad before them.
        let pad = usize::from(WIDTH) - len;
        let at = line
            .char_indices()
            .rev()
            .nth(LIGHTS_WIDTH - 1)
            .map_or(0, |(i, _)| i);
        line.insert_str(at, &" ".repeat(pad));
    }
    line
}

/// Draw the status bar into `area` (one row, normally canvas row 24): black on cyan, with the
/// modem lights as LEDs on black.
pub fn render_status(ctx: &Ctx, buf: &mut Buffer, area: Rect) {
    let text = status_text(ctx);
    if text.is_empty() || area.height == 0 {
        return;
    }
    let bar = ctx.palette.style(Dos::Black, Dos::Cyan);
    let body = {
        let split = text
            .char_indices()
            .rev()
            .nth(LIGHTS_WIDTH - 1)
            .map_or(0, |(i, _)| i);
        &text[..split]
    };
    let body_len = u16::try_from(body.chars().count()).unwrap_or(WIDTH);
    for x in area.left()..area.right() {
        if let Some(c) = buf.cell_mut((x, area.y)) {
            c.set_symbol(" ");
            c.set_style(bar);
        }
    }
    put_str(buf, area.x, area.y, body, area.width, bar);
    // lights = " SD RD CD " (10 cells): label cells at offsets 1-2, 4-5, 7-8.
    let l = &ctx.lights;
    let led = |on: bool, on_colour: Dos| {
        ctx.palette
            .style(if on { on_colour } else { Dos::DarkGray }, Dos::Black)
    };
    let cd = match l.carrier() {
        Carrier::Offline => Dos::DarkGray,
        Carrier::Ok => Dos::LightGreen,
        Carrier::Failed => Dos::LightRed,
    };
    let styles: [Style; 3] = [
        led(l.sd_on(), Dos::Yellow),
        led(l.rd_on(), Dos::LightGreen),
        ctx.palette.style(cd, Dos::Black),
    ];
    for (i, name) in ["SD", "RD", "CD"].iter().enumerate() {
        let off = u16::try_from(1 + i * 3).unwrap_or(1);
        put_str(
            buf,
            area.x + body_len + off,
            area.y,
            name,
            2,
            styles.get(i).copied().unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::JobKind;
    use crate::game::state::RunState;
    use crate::ui::app::{sample_place, test_ctx};

    #[test]
    fn rd_lights_for_120ms_after_a_chunk() {
        let mut l = ModemLights::new();
        assert!(!l.rd_on());
        l.on_event(&DirectorEvent::Chunk);
        assert!(l.rd_on());
        l.tick(Duration::from_millis(119));
        assert!(l.rd_on());
        l.tick(Duration::from_millis(1));
        assert!(!l.rd_on());
        // A new chunk re-arms the full hold.
        l.on_event(&DirectorEvent::Chunk);
        l.tick(Duration::from_millis(100));
        l.on_event(&DirectorEvent::Chunk);
        l.tick(Duration::from_millis(100));
        assert!(l.rd_on());
    }

    #[test]
    fn cancelled_calls_end_their_request_and_fallback_only_turns_cd_red() {
        let mut l = ModemLights::new();
        for kind in [JobKind::Place, JobKind::Passage] {
            l.on_event(&DirectorEvent::Request { kind });
        }
        l.on_event(&DirectorEvent::CallOk);
        // A job falling back ends no request: the passage call is still out.
        l.on_event(&DirectorEvent::Fallback);
        assert!(l.sd_on());
        assert_eq!(l.carrier(), Carrier::Failed);
        l.on_event(&DirectorEvent::CallCancelled);
        assert!(!l.sd_on());
        assert_eq!(l.carrier(), Carrier::Failed);
    }

    #[test]
    fn sd_is_lit_while_requests_are_outstanding_and_cd_keeps_the_last_result() {
        let mut l = ModemLights::new();
        assert!(!l.sd_on());
        assert_eq!(l.carrier(), Carrier::Offline);
        l.on_event(&DirectorEvent::Request {
            kind: JobKind::Place,
        });
        l.on_event(&DirectorEvent::Request {
            kind: JobKind::Passage,
        });
        assert!(l.sd_on());
        l.on_event(&DirectorEvent::CallOk);
        assert!(l.sd_on());
        assert_eq!(l.carrier(), Carrier::Ok);
        l.on_event(&DirectorEvent::CallFailed);
        assert!(!l.sd_on());
        assert_eq!(l.carrier(), Carrier::Failed);
        // A stray result never underflows the count.
        l.on_event(&DirectorEvent::CallOk);
        assert!(!l.sd_on());
        l.on_event(&DirectorEvent::Request {
            kind: JobKind::Chat,
        });
        assert!(l.sd_on());
    }

    fn ctx_with_run(dir: &std::path::Path) -> Ctx {
        let mut ctx = test_ctx(dir);
        ctx.run = Some(RunState::new(
            "Tester", "T", "a dream", 42, "rollover", None,
        ));
        ctx
    }

    #[test]
    fn status_line_before_a_place_uses_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_run(dir.path());
        let text = status_text(&ctx);
        assert_eq!(text.chars().count(), 80);
        assert!(text.starts_with(" ABRAXAS │ L00 ... "), "{text:?}");
        assert!(text.contains("│ EGO 100 │ 0096K │ 23:00:00 12/31/99 │"));
        assert!(text.ends_with(" SD RD CD "));
    }

    #[test]
    fn status_line_truncates_the_title_to_fit_80_columns() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_with_run(dir.path());
        let mut place = sample_place("A".repeat(60));
        place.bbs_name = "SOL NIGER LONG NAME".into();
        let pack = ctx.pack.clone();
        let run = ctx.run.as_mut().unwrap();
        run.depth = 7;
        crate::game::seed::ensure_level(run, &pack, 7).place = Some(place);
        let text = status_text(&ctx);
        assert_eq!(text.chars().count(), 80);
        assert!(text.starts_with(" SOL NIGER LO │ L07 AAAA"), "{text:?}");
        assert!(text.contains("A... │ EGO 100 │ 0096K │"), "{text:?}");
        assert!(text.ends_with(" │ SD RD CD "), "{text:?}");
    }

    #[test]
    fn no_run_means_no_status_line() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        assert_eq!(status_text(&ctx), "");
    }

    #[test]
    fn lights_are_drawn_over_the_label_cells() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_with_run(dir.path());
        ctx.lights.on_event(&DirectorEvent::Request {
            kind: JobKind::Place,
        });
        ctx.lights.on_event(&DirectorEvent::CallFailed);
        ctx.lights.on_event(&DirectorEvent::Request {
            kind: JobKind::Place,
        });
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_status(&ctx, &mut buf, area);
        // SD lit (yellow), RD dark, CD red.
        let p = ctx.palette;
        assert_eq!(buf[(71, 0)].symbol(), "S");
        assert_eq!(buf[(71, 0)].fg, p.color(Dos::Yellow));
        assert_eq!(buf[(74, 0)].fg, p.color(Dos::DarkGray));
        assert_eq!(buf[(77, 0)].symbol(), "C");
        assert_eq!(buf[(77, 0)].fg, p.color(Dos::LightRed));
        // The body is black on cyan.
        assert_eq!(buf[(2, 0)].bg, p.color(Dos::Cyan));
        assert_eq!(buf[(2, 0)].fg, p.color(Dos::Black));
    }
}
