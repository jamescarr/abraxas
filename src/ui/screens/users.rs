//! The user list: who has been on this board, and when.
//!
//! Rows, top to bottom: the SysOp (the cast member `philemon`), the player, WREN (cast `wren`;
//! last on `12/10/99`, `01/01/00` from depth 7), the Shadow (the cast member with the shadow
//! archetype, under the run's computed shadow handle, `TOMORROW`), the level's NPC sysop when it
//! is not a cast member, and the dead from `callers.json` as `(LOST Ldd)`.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl};
use crate::game::save::CallerStatus;
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};

/// Depth from which Wren's last call is dated after the Rollover.
pub const WREN_LATE_DEPTH: u32 = 7;

const TOP_ROW: u16 = 4;
const VISIBLE_ROWS: usize = 18;
const HANDLE_COL: u16 = 2;
const LAST_ON_COL: u16 = 22;
const NOTES_COL: u16 = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
struct UserRow {
    handle: String,
    last_on: String,
    notes: String,
    /// The row for the player.
    me: bool,
}

impl UserRow {
    fn new(handle: &str, last_on: &str, notes: &str) -> Self {
        Self {
            handle: handle.to_string(),
            last_on: last_on.to_string(),
            notes: notes.to_string(),
            me: false,
        }
    }
}

/// The user list for the current run.
fn build_rows(ctx: &Ctx) -> Vec<UserRow> {
    let Some(run) = ctx.run_ref() else {
        return Vec::new();
    };
    let depth = run.depth;
    let mut rows = Vec::new();
    let present = |id: &str| ctx.pack.character(id).filter(|c| c.from_depth <= depth);
    if let Some(sysop) = present("philemon") {
        rows.push(UserRow::new(&sysop.handle, "ALWAYS", "SysOp"));
    }
    let mut me = UserRow::new(&run.handle, "NOW", &format!("L{depth:02}"));
    me.me = true;
    rows.push(me);
    if let Some(wren) = present("wren") {
        let when = if depth >= WREN_LATE_DEPTH {
            "01/01/00"
        } else {
            "12/10/99"
        };
        rows.push(UserRow::new(&wren.handle, when, ""));
    }
    if ctx
        .pack
        .shadow_character()
        .is_some_and(|c| c.from_depth <= depth)
    {
        rows.push(UserRow::new(&run.shadow(), "TOMORROW", ""));
    }
    if let Some(place) = ctx.place()
        && place.sysop.cast_id.is_none()
    {
        rows.push(UserRow::new(&place.sysop.handle, "NOW", "SysOp"));
    }
    for caller in ctx.store.callers() {
        if caller.status == CallerStatus::Lost && !caller.handle.eq_ignore_ascii_case(&run.handle) {
            rows.push(UserRow::new(
                &caller.handle,
                &format!("(LOST L{:02})", caller.depth),
                "",
            ));
        }
    }
    rows
}

/// The user list screen.
pub struct UsersScreen {
    rows: Vec<UserRow>,
    top: usize,
}

impl UsersScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        Self {
            rows: build_rows(ctx),
            top: 0,
        }
    }

    fn max_top(&self) -> usize {
        self.rows.len().saturating_sub(VISIBLE_ROWS)
    }
}

impl ScreenImpl for UsersScreen {
    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Nav {
        match classify(&key) {
            Key::Esc => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'q') => return Nav::Back,
            Key::Up => self.top = self.top.saturating_sub(1),
            Key::Down => self.top = (self.top + 1).min(self.max_top()),
            Key::PgUp => self.top = self.top.saturating_sub(VISIBLE_ROWS),
            Key::PgDn => self.top = (self.top + VISIBLE_ROWS).min(self.max_top()),
            _ => {}
        }
        Nav::Stay
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let w = area.width;
        let bbs = ctx.place().map_or("", |pl| pl.bbs_name.as_str());
        let title = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), title);
        put_str(
            buf,
            area.x,
            area.y,
            &format!(" {bbs} ─ User List"),
            w,
            title,
        );
        put_str(
            buf,
            area.x,
            area.y + 2,
            &format!(
                "{:width$}HANDLE{:pad1$}LAST ON{:pad2$}NOTES",
                "",
                "",
                "",
                width = usize::from(HANDLE_COL),
                pad1 = usize::from(LAST_ON_COL - HANDLE_COL) - 6,
                pad2 = usize::from(NOTES_COL - LAST_ON_COL) - 7,
            ),
            w,
            p.style(Dos::LightCyan, Dos::Black),
        );
        put_str(
            buf,
            area.x,
            area.y + 3,
            &"─".repeat(usize::from(w)),
            w,
            ctx.fg(Dos::DarkGray),
        );
        for (i, row) in self
            .rows
            .iter()
            .skip(self.top)
            .take(VISIBLE_ROWS)
            .enumerate()
        {
            let y = area.y + TOP_ROW + u16::try_from(i).unwrap_or(0);
            let lost = row.last_on.starts_with('(');
            let style = if row.me {
                ctx.fg(Dos::LightGreen)
            } else if lost {
                ctx.fg(Dos::DarkGray)
            } else {
                ctx.fg(Dos::White)
            };
            put_str(
                buf,
                area.x + HANDLE_COL,
                y,
                &row.handle,
                LAST_ON_COL - HANDLE_COL - 1,
                style,
            );
            put_str(
                buf,
                area.x + LAST_ON_COL,
                y,
                &row.last_on,
                NOTES_COL - LAST_ON_COL - 1,
                style,
            );
            put_str(buf, area.x + NOTES_COL, y, &row.notes, w - NOTES_COL, style);
        }
        put_str(
            buf,
            area.x,
            area.y + 23,
            " ↑↓ Scroll   Esc Back",
            w,
            ctx.fg(Dos::LightGray),
        );
    }

    fn wants_status(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::save::CallerRecord;
    use crate::game::state::Sysop;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::press;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn ctx_at(dir: &std::path::Path, depth: u32) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, depth);
        ctx
    }

    fn row(handle: &str, last_on: &str, notes: &str) -> (String, String, String) {
        (handle.into(), last_on.into(), notes.into())
    }

    fn rows_of(ctx: &Ctx) -> Vec<(String, String, String)> {
        build_rows(ctx)
            .into_iter()
            .map(|r| (r.handle, r.last_on, r.notes))
            .collect()
    }

    #[test]
    fn depth_zero_lists_sysop_player_and_wren_but_no_shadow() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path(), 0);
        assert_eq!(
            rows_of(&ctx),
            vec![
                row("PHILEMON", "ALWAYS", "SysOp"),
                row("Tester", "NOW", "L00"),
                row("WREN", "12/10/99", ""),
            ]
        );
    }

    #[test]
    fn shadow_row_appears_from_depth_one_under_the_computed_handle() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        // Make the level's sysop a cast member so only the fixed rows remain.
        ctx.run_mut()
            .unwrap()
            .levels
            .get_mut(&1)
            .unwrap()
            .place
            .as_mut()
            .unwrap()
            .sysop = Sysop {
            handle: "PHILEMON".into(),
            cast_id: Some("philemon".into()),
        };
        let shadow = ctx.run_ref().unwrap().shadow();
        assert_eq!(shadow, "RETSEt");
        assert_eq!(
            rows_of(&ctx),
            vec![
                row("PHILEMON", "ALWAYS", "SysOp"),
                row("Tester", "NOW", "L01"),
                row("WREN", "12/10/99", ""),
                row(&shadow, "TOMORROW", ""),
            ]
        );
    }

    #[test]
    fn wren_is_dated_after_the_rollover_from_depth_seven() {
        let dir = tempfile::tempdir().unwrap();
        for (depth, date) in [(6, "12/10/99"), (7, "01/01/00"), (9, "01/01/00")] {
            let ctx = ctx_at(dir.path(), depth);
            let wren = rows_of(&ctx).into_iter().find(|r| r.0 == "WREN").unwrap();
            assert_eq!(wren.1, date, "depth {depth}");
        }
    }

    #[test]
    fn an_npc_sysop_is_listed_but_a_cast_sysop_is_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 2);
        let place = ctx
            .run_mut()
            .unwrap()
            .levels
            .get_mut(&2)
            .unwrap()
            .place
            .as_mut()
            .unwrap();
        place.sysop = Sysop {
            handle: "DRYWALL".into(),
            cast_id: None,
        };
        assert!(rows_of(&ctx).contains(&row("DRYWALL", "NOW", "SysOp")));
        let place = ctx
            .run_mut()
            .unwrap()
            .levels
            .get_mut(&2)
            .unwrap()
            .place
            .as_mut()
            .unwrap();
        place.sysop = Sysop {
            handle: "SYZYGY".into(),
            cast_id: Some("syzygy".into()),
        };
        assert!(rows_of(&ctx).iter().all(|r| r.0 != "SYZYGY"));
    }

    #[test]
    fn the_dead_come_from_the_callers_log_and_only_the_lost_ones() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 0);
        for (handle, depth, status) in [
            ("ghostwalker", 4, CallerStatus::Lost),
            ("alive_one", 2, CallerStatus::Online),
            ("ascendant", 15, CallerStatus::Ascended),
            ("moth", 11, CallerStatus::Lost),
        ] {
            ctx.store
                .record_caller(CallerRecord {
                    handle: handle.into(),
                    depth,
                    clock: "23:10:00".into(),
                    status,
                })
                .unwrap();
        }
        let rows = rows_of(&ctx);
        let dead: Vec<_> = rows.iter().filter(|r| r.1.starts_with('(')).collect();
        assert_eq!(
            dead,
            vec![
                &row("moth", "(LOST L11)", ""),
                &row("ghostwalker", "(LOST L04)", ""),
            ]
        );
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        let mut screen = UsersScreen::new(&mut ctx);
        term.draw(|f| screen.render(f, f.area(), &ctx)).unwrap();
        let buf = term.backend().buffer();
        let text: Vec<String> = (0..25).map(|y| row_text(buf, buf.area, y)).collect();
        assert!(
            text[2].contains("HANDLE") && text[2].contains("LAST ON") && text[2].contains("NOTES")
        );
        assert!(
            text[4].contains("PHILEMON") && text[4].contains("ALWAYS") && text[4].contains("SysOp")
        );
        assert!(
            text.iter()
                .any(|l| l.contains("ghostwalker") && l.contains("(LOST L04)"))
        );
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
    }
}
