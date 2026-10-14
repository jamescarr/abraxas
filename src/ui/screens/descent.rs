//! Descent: the fall between levels. The old screen dissolves into a vision, three captions
//! fade in, the next level's interlude is transmitted, and once everything the next level needs
//! has arrived its hexagram is cast. Then `CONNECT 28800/...` and the level card.
//!
//! The screen works from the run alone (`Phase::Descending`), so a quit mid-descent resumes here.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{DialerScreen, LevelCardScreen, Nav, Screen, ScreenImpl};
use crate::director::offline;
use crate::director::validate::GenContext;
use crate::game::save::CallerStatus;
use crate::game::seed::ensure_level;
use crate::game::state::GenSource;
use crate::pack::model::VisionKind;
use crate::palette::Dos;
use crate::ui::app::{Ctx, FxCommand};
use crate::ui::canvas::{HEIGHT, STATUS_ROW, WIDTH, fill, put_centered, put_str};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::transmission::Transmission;
use crate::visions::{HexagramCast, Vision};

const CONNECT: &str = "CONNECT 28800/ARQ/V34/LAPM/V42BIS";
/// Seconds the descent lasts at least before the hexagram casts (calm mode: 2 s).
const MIN_WAIT: f32 = 6.0;
const MIN_WAIT_CALM: f32 = 2.0;
/// The first caption appears at this time and each next one `CAPTION_STEP` later.
const CAPTION_START: f32 = 0.8;
const CAPTION_STEP: f32 = 1.2;
const CAPTION_START_CALM: f32 = 0.2;
const CAPTION_STEP_CALM: f32 = 0.3;
const CAPTION_FADE_MS: u64 = 700;
const DISSOLVE_MS: u64 = 600;
const COALESCE_MS: u64 = 400;
/// How long `CONNECT` stays up before the level card (shorter after a skip).
const HOLD: f32 = 0.7;
const HOLD_QUICK: f32 = 0.25;
const CAPTION_ROW: u16 = 2;
const HEXAGRAM_ROW: u16 = 6;
const BOX: Rect = Rect::new(4, 15, 72, 7);
const RX_ROW: u16 = 22;
const CONNECT_ROW: u16 = 23;

/// The Descent screen.
pub struct DescentScreen {
    /// The depth being left; the screen arrives at `from + 1`.
    from: u32,
    t: f32,
    vision: Vision,
    captions: Vec<String>,
    captions_shown: usize,
    interlude: Transmission,
    /// Bytes of `ctx.streams` already pushed into `interlude`.
    pushed: usize,
    interlude_final: bool,
    min_wait: f32,
    timeout: f32,
    fell_back: bool,
    cast: Option<HexagramCast>,
    connect: Option<Transmission>,
    hold: f32,
    quick: bool,
}

impl DescentScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let calm = ctx.calm();
        let (from, seed) = ctx.run_ref().map_or((0, 0), |r| (r.depth, r.seed));
        let kind = ctx
            .passage()
            .map(|p| p.vision)
            .or_else(|| ctx.act().map(|a| a.default_vision))
            .unwrap_or(VisionKind::Tunnel);
        let captions = ctx
            .passage()
            .map(|p| p.captions.clone())
            .unwrap_or_default();
        let mut vision = Vision::new(kind, seed, from, ctx.gradient(), calm);
        if kind == VisionKind::Mirror {
            vision.set_snapshot(ctx.last_frame.clone());
        }
        if !calm {
            ctx.fx.push(FxCommand::Dissolve { ms: DISSOLVE_MS });
        }
        Self {
            from,
            t: 0.0,
            vision,
            captions,
            captions_shown: 0,
            interlude: Transmission::streaming(ctx.baud()),
            pushed: 0,
            interlude_final: false,
            min_wait: if calm { MIN_WAIT_CALM } else { MIN_WAIT },
            timeout: ctx.config.ai.timeout_secs.max(1) as f32,
            fell_back: false,
            cast: None,
            connect: None,
            hold: 0.0,
            quick: false,
        }
    }

    /// The vision playing behind the descent.
    pub fn vision_kind(&self) -> VisionKind {
        self.vision.kind()
    }

    /// Whether the hexagram is being cast (or already cast).
    pub fn is_casting(&self) -> bool {
        self.cast.is_some()
    }

    /// Whether `CONNECT ...` has started.
    pub fn is_connecting(&self) -> bool {
        self.connect.is_some()
    }

    /// Seconds since the descent began.
    pub fn elapsed(&self) -> f32 {
        self.t
    }

    /// Everything level `from + 1` needs has arrived: place, passage and interlude.
    fn content_ready(&self, ctx: &Ctx) -> bool {
        ctx.run_ref()
            .and_then(|r| r.levels.get(&(self.from + 1)))
            .is_some_and(|l| l.place.is_some() && l.passage.is_some() && l.interlude.is_some())
    }

    /// Feed the interlude transmission from the streamed text, then the final text.
    fn sync_interlude(&mut self, ctx: &Ctx) {
        if self.interlude_final {
            return;
        }
        let next = self.from + 1;
        let done = ctx
            .run_ref()
            .and_then(|r| r.levels.get(&next))
            .and_then(|l| l.interlude.as_deref());
        if let Some(text) = done {
            self.interlude.set_final(text);
            self.interlude_final = true;
        } else if let Some(stream) = ctx.streams.get(&next)
            && let Some(fresh) = stream.get(self.pushed..)
            && !fresh.is_empty()
        {
            self.interlude.push_str(fresh);
            self.pushed = stream.len();
        }
    }

    /// Fade in each caption in rows 2-4 at its time.
    fn reveal_captions(&mut self, ctx: &mut Ctx) {
        let (start, step) = if ctx.calm() {
            (CAPTION_START_CALM, CAPTION_STEP_CALM)
        } else {
            (CAPTION_START, CAPTION_STEP)
        };
        while self.captions_shown < self.captions.len().min(3) {
            let due = start + step * self.captions_shown as f32;
            if self.t < due {
                break;
            }
            let row = CAPTION_ROW + self.captions_shown as u16;
            let len = self.captions[self.captions_shown].chars().count() as u16 + 2;
            let x = WIDTH.saturating_sub(len) / 2;
            if !ctx.calm() {
                ctx.fx.push(FxCommand::FadeFromFg {
                    ms: CAPTION_FADE_MS,
                    area: Rect::new(x, row, len, 1),
                });
            }
            self.captions_shown += 1;
        }
    }

    /// The director never delivered: build the missing parts of the next level offline so the
    /// descent cannot hang.
    fn fall_back(&mut self, ctx: &mut Ctx) {
        self.fell_back = true;
        let next = self.from + 1;
        let Ctx { run, pack, .. } = ctx;
        let Some(run) = run.as_mut() else {
            return;
        };
        let seed = ensure_level(run, pack, next).seed.clone();
        let (place, passage, interlude) = {
            let level = run.levels.get(&next);
            let gen_ctx = GenContext {
                run,
                pack,
                seed: &seed,
            };
            let missing = |f: fn(&crate::game::state::Level) -> bool| level.is_some_and(f);
            let place_new = missing(|l| l.place.is_none()).then(|| offline::place(&gen_ctx));
            let place_ref = place_new
                .clone()
                .or_else(|| level.and_then(|l| l.place.clone()));
            let passage_new = place_ref
                .as_ref()
                .filter(|_| missing(|l| l.passage.is_none()))
                .map(|p| offline::passage(&gen_ctx, p));
            let interlude_new = place_ref
                .as_ref()
                .filter(|_| missing(|l| l.interlude.is_none()))
                .map(|p| offline::interlude(&gen_ctx, p, run.exit_taken.as_ref()));
            (place_new, passage_new, interlude_new)
        };
        let source = GenSource::Offline {
            reason: "timeout".into(),
        };
        if let Some(place) = &place {
            run.used_pool.extend(place.pool_keys());
        }
        if let Some(level) = run.levels.get_mut(&next) {
            if let Some(place) = place {
                level.place = Some(place);
                level.place_source = Some(source.clone());
            }
            if let Some(passage) = passage {
                level.passage = Some(passage);
                level.passage_source = Some(source.clone());
            }
            if let Some(interlude) = interlude {
                level.interlude = Some(interlude);
                level.interlude_source = Some(source);
            }
        }
        ctx.autosave();
    }

    fn start_cast(&mut self, ctx: &Ctx) {
        let omen = ctx
            .run_ref()
            .and_then(|r| r.levels.get(&(self.from + 1)))
            .map_or(1, |l| l.seed.omen);
        self.cast = Some(HexagramCast::new(omen, ctx.calm()));
    }

    fn start_connect(&mut self, ctx: &Ctx) {
        self.connect = Some(Transmission::new(CONNECT, ctx.baud()));
        self.hold = 0.0;
    }

    /// Arrive at the next level.
    fn finish(&self, ctx: &mut Ctx) -> Nav {
        let next = self.from + 1;
        let Some(run) = ctx.run_mut() else {
            return Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))));
        };
        run.arrive(next);
        ctx.streams.retain(|d, _| *d >= next);
        ctx.autosave();
        ctx.record_caller(CallerStatus::Online);
        if !ctx.calm() {
            ctx.fx.push(FxCommand::Coalesce { ms: COALESCE_MS });
        }
        Nav::To(Box::new(Screen::LevelCard(LevelCardScreen::new(ctx))))
    }
}

impl ScreenImpl for DescentScreen {
    fn handle_key(&mut self, _key: KeyEvent, ctx: &mut Ctx) -> Nav {
        self.interlude.skip();
        if let Some(connect) = &mut self.connect {
            connect.skip();
            self.quick = true;
            return Nav::Stay;
        }
        // Never skip past missing generation.
        self.sync_interlude(ctx);
        if self.content_ready(ctx) && self.interlude_final {
            self.start_cast(ctx);
            if let Some(cast) = &mut self.cast {
                cast.tick(Duration::from_secs(10));
            }
            self.start_connect(ctx);
            self.quick = true;
        }
        Nav::Stay
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if ctx.run.is_none() {
            return Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))));
        }
        self.t += dt.as_secs_f32();
        self.vision.tick(dt);
        self.reveal_captions(ctx);
        if !self.fell_back && !self.content_ready(ctx) && self.t >= self.timeout {
            self.fall_back(ctx);
        }
        self.sync_interlude(ctx);
        self.interlude.tick(dt);

        if self.cast.is_none()
            && self.connect.is_none()
            && self.content_ready(ctx)
            && self.interlude.is_done()
            && self.t >= self.min_wait
        {
            self.start_cast(ctx);
        }
        if let Some(cast) = &mut self.cast {
            cast.tick(dt);
            if cast.is_done() && self.connect.is_none() {
                self.start_connect(ctx);
            }
        }
        if let Some(connect) = &mut self.connect {
            connect.tick(dt);
            if connect.is_done() {
                self.hold += dt.as_secs_f32();
                let hold = if self.quick { HOLD_QUICK } else { HOLD };
                if self.hold >= hold {
                    return self.finish(ctx);
                }
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let black = p.color(Dos::Black);
        fill(buf, area, ctx.text_style());
        let vision_area = Rect::new(area.x, area.y, WIDTH, STATUS_ROW.min(HEIGHT));
        self.vision.render(vision_area, buf);

        let caption_style = p.style(Dos::White, Dos::Black);
        for (i, caption) in self.captions.iter().take(self.captions_shown).enumerate() {
            let text = format!(" {caption} ");
            put_centered(
                buf,
                area,
                area.y + CAPTION_ROW + i as u16,
                &text,
                caption_style,
            );
        }

        if let Some(cast) = &self.cast {
            self.render_cast(buf, area, cast, ctx);
        }

        let chrome = Style::new().fg(ctx.chrome()).bg(black);
        let outer = Rect::new(area.x + BOX.x, area.y + BOX.y, BOX.width, BOX.height);
        let inner = boxed(
            buf,
            outer,
            Some("INCOMING TRANSMISSION"),
            BoxKind::Double,
            chrome,
        );
        let text_area = Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            inner.height,
        );
        self.interlude.render(buf, text_area, ctx.text_style());

        let rx = self
            .interlude
            .full_text()
            .len()
            .min(self.interlude.shown_chars());
        let rx_line = format!(" RX {rx} bytes │ CARRIER 28800 ");
        let rx_style = p.style(Dos::LightCyan, Dos::Black);
        put_str(
            buf,
            area.x + BOX.x,
            area.y + RX_ROW,
            &rx_line,
            BOX.width,
            rx_style,
        );

        if let Some(connect) = &self.connect {
            let shown = connect.visible_lines(WIDTH).concat();
            put_centered(
                buf,
                area,
                area.y + CONNECT_ROW,
                &shown,
                p.style(Dos::White, Dos::Black),
            );
        }
    }

    fn wants_status(&self) -> bool {
        true
    }
}

impl DescentScreen {
    /// The hexagram in a cleared patch of the vision, between the captions and the box.
    fn render_cast(&self, buf: &mut Buffer, area: Rect, cast: &HexagramCast, ctx: &Ctx) {
        let omen = ctx
            .run_ref()
            .and_then(|r| r.levels.get(&(self.from + 1)))
            .map_or(1, |l| l.seed.omen);
        let title_len = crate::oracle::title(omen).chars().count() as u16;
        let block_w = title_len.max(14);
        let x = area.x + WIDTH.saturating_sub(block_w) / 2;
        let y = area.y + HEXAGRAM_ROW;
        let pad = Rect::new(
            x.saturating_sub(2).max(area.x),
            y.saturating_sub(1),
            (block_w + 4).min(WIDTH),
            crate::visions::hexagram::HEIGHT + 2,
        );
        fill(buf, pad, ctx.text_style());
        cast.render(
            Rect::new(x, y, block_w, crate::visions::hexagram::HEIGHT),
            buf,
            ctx.chrome(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::save::CallerStatus;
    use crate::game::state::Phase;
    use crate::ui::app::{App, install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::press_char;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const DT: Duration = Duration::from_millis(100);

    /// A run at `depth` that made its choice and took exit 1: the state Exits leaves behind.
    fn descending(dir: &std::path::Path, depth: u32) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, depth);
        let pack = ctx.pack.clone();
        let run = ctx.run.as_mut().unwrap();
        assert!(run.apply_choice(&pack, 0));
        assert!(run.apply_exit(1));
        ctx
    }

    fn next_level_mut(ctx: &mut Ctx) -> &mut crate::game::state::Level {
        let run = ctx.run.as_mut().unwrap();
        let next = run.depth + 1;
        run.levels.get_mut(&next).unwrap()
    }

    /// Tick for `secs`; the name of the screen navigated to, if any.
    fn run_for(s: &mut DescentScreen, ctx: &mut Ctx, secs: f32) -> Option<&'static str> {
        let steps = (secs / DT.as_secs_f32()).round() as u32;
        for _ in 0..steps {
            if let Nav::To(next) = s.tick(DT, ctx) {
                return Some(next.name());
            }
        }
        None
    }

    fn rows(s: &DescentScreen, ctx: &Ctx) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 25);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| s.render(f, area, ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, area, y)).collect()
    }

    #[test]
    fn the_hexagram_waits_for_the_six_second_minimum() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        let mut s = DescentScreen::new(&mut ctx);
        assert_eq!(run_for(&mut s, &mut ctx, 5.8), None);
        assert!(s.interlude.is_done(), "the interlude finished long ago");
        assert!(!s.is_casting());
        run_for(&mut s, &mut ctx, 0.4);
        assert!(s.is_casting());
        assert!(!s.is_connecting());
    }

    #[test]
    fn the_hexagram_waits_for_the_interlude_and_both_generated_parts() {
        for missing in ["interlude", "place", "passage"] {
            let dir = tempfile::tempdir().unwrap();
            let mut ctx = descending(dir.path(), 3);
            let kept = next_level_mut(&mut ctx).clone();
            {
                let l = next_level_mut(&mut ctx);
                match missing {
                    "interlude" => l.interlude = None,
                    "place" => l.place = None,
                    _ => l.passage = None,
                }
            }
            let mut s = DescentScreen::new(&mut ctx);
            run_for(&mut s, &mut ctx, 9.0);
            assert!(!s.is_casting(), "no cast without the {missing}");
            *next_level_mut(&mut ctx) = kept;
            run_for(&mut s, &mut ctx, 0.2);
            assert!(s.is_casting(), "the {missing} arrived, the cast starts");
        }
    }

    #[test]
    fn calm_mode_needs_only_two_seconds_and_issues_no_effects() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        ctx.config.display.calm = true;
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 1.8);
        assert!(!s.is_casting());
        let nav = run_for(&mut s, &mut ctx, 0.3);
        assert!(s.is_casting() || nav.is_some());
        run_for(&mut s, &mut ctx, 5.0);
        assert!(ctx.fx.is_empty(), "calm: {:?}", ctx.fx);
    }

    #[test]
    fn calm_freezes_the_vision() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        ctx.config.display.calm = true;
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 1.0);
        let a = rows(&s, &ctx);
        run_for(&mut s, &mut ctx, 0.5);
        assert_eq!(a, rows(&s, &ctx));
    }

    #[test]
    fn captions_fade_in_one_at_a_time_in_rows_two_to_four() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        let captions = ctx.passage().unwrap().captions.clone();
        let mut s = DescentScreen::new(&mut ctx);
        assert!(ctx.fx.contains(&FxCommand::Dissolve { ms: 600 }));
        ctx.fx.clear();
        run_for(&mut s, &mut ctx, 0.5);
        assert!(ctx.fx.is_empty(), "nothing before 0.8 s");
        run_for(&mut s, &mut ctx, 0.5);
        assert_eq!(ctx.fx.len(), 1);
        assert!(matches!(ctx.fx[0], FxCommand::FadeFromFg { ms: 700, area } if area.y == 2));
        let r = rows(&s, &ctx);
        assert!(r[2].contains(&captions[0]));
        assert!(!r[3].contains(&captions[1]));
        run_for(&mut s, &mut ctx, 2.4);
        let r = rows(&s, &ctx);
        assert!(r[3].contains(&captions[1]) && r[4].contains(&captions[2]));
        assert_eq!(ctx.fx.len(), 3);
    }

    #[test]
    fn the_vision_is_the_passages_when_ready_else_the_acts_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .passage
            .as_mut()
            .unwrap()
            .vision = VisionKind::Plasma;
        assert_eq!(
            DescentScreen::new(&mut ctx).vision_kind(),
            VisionKind::Plasma
        );
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .passage = None;
        // Depth 3 is in NIGREDO, whose default vision is fire.
        assert_eq!(DescentScreen::new(&mut ctx).vision_kind(), VisionKind::Fire);
    }

    #[test]
    fn a_key_skips_only_once_the_content_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        let kept = next_level_mut(&mut ctx).clone();
        next_level_mut(&mut ctx).interlude = None;
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 1.0);
        s.handle_key(press_char(' '), &mut ctx);
        assert!(
            !s.is_casting() && !s.is_connecting(),
            "never skips past missing text"
        );
        *next_level_mut(&mut ctx) = kept;
        s.handle_key(press_char(' '), &mut ctx);
        assert!(s.is_connecting(), "ready: skip jumps to CONNECT");
        assert_eq!(run_for(&mut s, &mut ctx, 1.0), Some("LevelCard"));
        assert!(s.elapsed() < 3.0, "skipped well before the 6 s minimum");
    }

    #[test]
    fn the_full_timeline_casts_connects_and_arrives_at_the_next_level() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        ctx.run.as_mut().unwrap().stats.ego = 50;
        let omen = next_level_mut(&mut ctx).seed.omen;
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 7.0);
        let casting = rows(&s, &ctx).join("\n");
        assert!(casting.contains('█'), "the hexagram is drawing:\n{casting}");
        for _ in 0..60 {
            assert!(matches!(s.tick(DT, &mut ctx), Nav::Stay));
            if s.is_connecting() {
                break;
            }
        }
        assert!(s.is_connecting());
        assert!(
            s.elapsed() < 11.0,
            "the cast takes about 3 s: {}",
            s.elapsed()
        );
        let title = crate::oracle::title(omen);
        let r = rows(&s, &ctx);
        assert!(r.join("\n").contains(&title), "{title}");
        assert!(r[23].contains("CONNECT 28800/ARQ/V34/LAPM/V42BIS"));
        ctx.fx.clear();
        assert_eq!(run_for(&mut s, &mut ctx, 1.5), Some("LevelCard"));
        let run = ctx.run.as_ref().unwrap();
        assert_eq!((run.depth, run.stats.ego), (4, 60));
        assert_eq!(run.phase, Phase::Arrival);
        assert!(ctx.fx.contains(&FxCommand::Coalesce { ms: 400 }));
        assert_eq!(ctx.store.load("Tester").unwrap().unwrap().depth, 4);
        let caller = &ctx.store.callers()[0];
        assert_eq!((caller.depth, caller.status), (4, CallerStatus::Online));
    }

    #[test]
    fn the_interlude_streams_into_the_box_with_an_rx_counter() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        next_level_mut(&mut ctx).interlude = None;
        ctx.streams.insert(4, "You fall".into());
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 0.3);
        let r = rows(&s, &ctx);
        assert!(r[15].contains("INCOMING TRANSMISSION"));
        assert!(r[16].contains("You fall"));
        assert!(r[22].contains("RX 8 bytes │ CARRIER 28800"), "{:?}", r[22]);
        ctx.streams.get_mut(&4).unwrap().push_str(" through.");
        run_for(&mut s, &mut ctx, 0.2);
        assert!(rows(&s, &ctx)[16].contains("You fall through."));
        assert!(!s.interlude.is_done(), "still open until the final text");
    }

    #[test]
    fn a_stuck_director_falls_back_to_offline_content_after_the_ai_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        ctx.config.ai.timeout_secs = 4;
        ctx.run.as_mut().unwrap().levels.remove(&4);
        let mut s = DescentScreen::new(&mut ctx);
        run_for(&mut s, &mut ctx, 3.5);
        assert!(!s.content_ready(&ctx), "not before the timeout");
        run_for(&mut s, &mut ctx, 1.0);
        let l = next_level_mut(&mut ctx).clone();
        assert!(l.place.is_some() && l.passage.is_some() && l.interlude.is_some());
        let off = GenSource::Offline {
            reason: "timeout".into(),
        };
        assert_eq!(l.place_source, Some(off.clone()));
        assert_eq!(l.passage_source, Some(off.clone()));
        assert_eq!(l.interlude_source, Some(off));
        assert_eq!(run_for(&mut s, &mut ctx, 12.0), Some("LevelCard"));
        assert_eq!(ctx.run.as_ref().unwrap().depth, 4);
    }

    #[test]
    fn a_resumed_descent_runs_from_the_saved_run_alone() {
        let dir = tempfile::tempdir().unwrap();
        descending(dir.path(), 7).autosave();
        let mut fresh = test_ctx(dir.path());
        fresh.run = fresh.store.load("Tester").unwrap();
        assert!(matches!(
            fresh.run.as_ref().unwrap().phase,
            Phase::Descending { exit: 1 }
        ));
        let mut s = DescentScreen::new(&mut fresh);
        assert_eq!(run_for(&mut s, &mut fresh, 12.0), Some("LevelCard"));
        assert_eq!(fresh.run.as_ref().unwrap().depth, 8);
    }

    #[test]
    fn through_the_app_the_descent_ends_on_the_next_level_card() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = descending(dir.path(), 3);
        let screen = Screen::Descent(DescentScreen::new(&mut ctx));
        let mut app = App::with_screen(ctx, screen);
        let mut names = Vec::new();
        for _ in 0..200 {
            app.step(None, DT);
            if app.screen.name() != "Descent" {
                names.push(app.screen.name());
                break;
            }
        }
        assert_eq!(names, vec!["LevelCard"]);
        assert_eq!(app.ctx.run.as_ref().unwrap().depth, 4);
    }
}
