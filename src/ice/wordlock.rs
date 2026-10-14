//! WORDLOCK: the Fallout-style terminal that guards a host's PORTAL.

use std::time::Duration;

use rand::RngExt;
use rand::SeedableRng;
use rand::seq::{IndexedRandom, SliceRandom};

use super::{IceGame, IceInput, IceOutcome, Loadout};
use crate::game::danger::DangerParams;
use crate::rng::Rng;

/// Rows per column.
pub const ROWS: usize = 16;
/// Characters per row per column.
pub const WIDTH: usize = 12;
/// Number of 12-character lines (2 columns x 16 rows); line = `col * 16 + row`.
pub const LINES: usize = 2 * ROWS;
/// Attempts per layout.
pub const ATTEMPTS: u8 = 4;
/// Trace added by a lockout.
pub const TRACE_ON_LOCKOUT: f32 = 15.0;
/// Trace added by Esc.
pub const TRACE_ON_ABORT: f32 = 5.0;
/// Seconds before a locked terminal regenerates.
pub const LOCKOUT_SECS: u64 = 2;
/// First hex address; each row adds 12.
pub const BASE_ADDRESS: u32 = 0xF4A0;

const JUNK: &str = "!@#$%^&*-_=+/\\;:'\",.?";
const OPENERS: [char; 4] = ['(', '[', '{', '<'];
const CLOSERS: [char; 4] = [')', ']', '}', '>'];

/// Number of matching letters in the same positions.
pub fn likeness(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).filter(|(x, y)| x == y).count()
}

/// A word placed on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSlot {
    /// The word.
    pub text: String,
    /// Line index (`col * 16 + row`).
    pub line: usize,
    /// First character within the line.
    pub start: usize,
    /// True for the level password.
    pub is_password: bool,
    /// True once removed as a dud (its cells are dots).
    pub removed: bool,
    /// True once guessed.
    pub tried: bool,
}

/// Cursor position over the 24-character logical row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Column 0 or 1.
    pub col: usize,
    /// Row 0..16.
    pub row: usize,
    /// Character 0..12 within the column.
    pub ch: usize,
}

impl Cursor {
    /// Line index of the cursor.
    pub fn line(&self) -> usize {
        self.col * ROWS + self.row
    }
}

/// What the cursor currently points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    /// Line index.
    pub line: usize,
    /// First highlighted character.
    pub start: usize,
    /// Number of highlighted characters.
    pub len: usize,
    /// True for a word, false for a bracket pair.
    pub is_word: bool,
}

/// The WORDLOCK game state.
#[derive(Debug, Clone)]
pub struct Wordlock {
    /// The 32 lines of 12 characters.
    pub lines: Vec<[char; WIDTH]>,
    /// Placed words.
    pub words: Vec<WordSlot>,
    /// Cursor.
    pub cursor: Cursor,
    /// Attempts left.
    pub attempts: u8,
    /// Terminal log, oldest first.
    pub log: Vec<String>,
    /// Regeneration counter (starts at the constructor argument).
    pub attempt: u32,
    lockout: Option<Duration>,
    pool: Vec<String>,
    password: String,
    word_count: usize,
    rng: Rng,
    loadout: Loadout,
    outcome: Option<IceOutcome>,
    pending_trace: f32,
}

type Layout = (Vec<[char; WIDTH]>, Vec<WordSlot>);

fn free(occ: &[[bool; WIDTH]], line: usize, start: usize, len: usize, margin: bool) -> bool {
    if start + len > WIDTH {
        return false;
    }
    let (lo, hi) = if margin {
        (start.saturating_sub(1), (start + len + 1).min(WIDTH))
    } else {
        (start, start + len)
    };
    occ[line][lo..hi].iter().all(|c| !c)
}

fn place(rng: &mut Rng, occ: &mut [[bool; WIDTH]], len: usize) -> Option<(usize, usize)> {
    let found = (0..300)
        .map(|_| (rng.random_range(0..LINES), rng.random_range(0..WIDTH)))
        .find(|&(l, s)| free(occ, l, s, len, true))
        .or_else(|| {
            let mut all: Vec<(usize, usize)> = (0..LINES)
                .flat_map(|l| (0..WIDTH).map(move |s| (l, s)))
                .collect();
            all.shuffle(rng);
            all.iter()
                .copied()
                .find(|&(l, s)| free(occ, l, s, len, true))
                .or_else(|| {
                    all.iter()
                        .copied()
                        .find(|&(l, s)| free(occ, l, s, len, false))
                })
        });
    let (l, s) = found?;
    occ[l][s..s + len].iter_mut().for_each(|c| *c = true);
    Some((l, s))
}

fn layout(rng: &mut Rng, pool: &[String], password: &str, word_count: usize) -> Layout {
    // Candidates: top 2(N-1) by likeness to the password, N-1 chosen at random.
    let mut others: Vec<&String> = Vec::new();
    for w in pool {
        if w != password && !others.contains(&w) && w.chars().count() <= WIDTH {
            others.push(w);
        }
    }
    others.sort_by_key(|w| std::cmp::Reverse(likeness(w, password)));
    let want = word_count.saturating_sub(1);
    others.truncate(2 * want);
    let mut picked: Vec<&String> = others.sample(rng, want).copied().collect();
    let mut texts: Vec<(String, bool)> = vec![(password.to_string(), true)];
    texts.extend(picked.drain(..).map(|w| (w.clone(), false)));
    texts.shuffle(rng);

    let mut lines: Vec<[char; WIDTH]> = (0..LINES)
        .map(|_| {
            let mut row = ['.'; WIDTH];
            for c in &mut row {
                *c = random_junk(rng);
            }
            row
        })
        .collect();
    let mut occ = vec![[false; WIDTH]; LINES];
    let mut words = Vec::with_capacity(texts.len());
    for (text, is_password) in texts {
        let len = text.chars().count();
        if let Some((line, start)) = place(rng, &mut occ, len) {
            for (i, ch) in text.chars().enumerate() {
                lines[line][start + i] = ch;
            }
            words.push(WordSlot {
                text,
                line,
                start,
                is_password,
                removed: false,
                tried: false,
            });
        }
    }
    let mut kinds = [0usize, 1, 2, 3];
    kinds.shuffle(rng);
    for &k in &kinds[..3] {
        let inside = rng.random_range(2..=6usize);
        if let Some((line, start)) = place(rng, &mut occ, inside + 2) {
            lines[line][start] = OPENERS[k];
            for i in 0..inside {
                lines[line][start + 1 + i] = random_junk(rng);
            }
            lines[line][start + 1 + inside] = CLOSERS[k];
        }
    }
    for _ in 0..6 {
        if let Some((line, start)) = place(rng, &mut occ, 1) {
            lines[line][start] = CLOSERS[rng.random_range(0..4usize)];
        }
    }
    (lines, words)
}

fn random_junk(rng: &mut Rng) -> char {
    let n = JUNK.chars().count();
    JUNK.chars().nth(rng.random_range(0..n)).unwrap_or('.')
}

impl Wordlock {
    /// Build a board. `words` and `password` come from the level's `Place`; `attempt` is the
    /// regeneration counter.
    pub fn generate(
        rng: &mut Rng,
        params: &DangerParams,
        words: &[String],
        password: &str,
        attempt: u32,
        loadout: Loadout,
    ) -> Self {
        let mut own = Rng::seed_from_u64(rng.random());
        let (lines, slots) = layout(&mut own, words, password, params.wordlock_words);
        Self {
            lines,
            words: slots,
            cursor: Cursor {
                col: 0,
                row: 0,
                ch: 0,
            },
            attempts: ATTEMPTS,
            log: Vec::new(),
            attempt,
            lockout: None,
            pool: words.to_vec(),
            password: password.to_string(),
            word_count: params.wordlock_words,
            rng: own,
            loadout,
            outcome: None,
            pending_trace: 0.0,
        }
    }

    /// Hex address shown at the start of a row.
    pub fn address(row: usize) -> u32 {
        BASE_ADDRESS + (WIDTH * row) as u32
    }

    /// Programs left after play (charges spent on keys 1 and 2 are gone).
    pub fn loadout(&self) -> Loadout {
        self.loadout
    }

    /// Drain trace penalties incurred since the last call.
    pub fn take_trace(&mut self) -> f32 {
        std::mem::take(&mut self.pending_trace)
    }

    /// True while the terminal is locked out awaiting regeneration.
    pub fn locked(&self) -> bool {
        self.lockout.is_some()
    }

    /// The level password.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Bracket pair spans `(open, close)` on a line, as they currently read.
    pub fn bracket_spans(&self, line: usize) -> Vec<(usize, usize)> {
        let row = &self.lines[line];
        let mut out = Vec::new();
        for (j, c) in row.iter().enumerate() {
            let Some(k) = OPENERS.iter().position(|o| o == c) else {
                continue;
            };
            for (m, &ch) in row.iter().enumerate().skip(j + 1) {
                if ch.is_alphabetic() {
                    break;
                }
                if ch == CLOSERS[k] {
                    out.push((j, m));
                    break;
                }
            }
        }
        out
    }

    fn word_at(&self, line: usize, ch: usize) -> Option<usize> {
        self.words.iter().position(|w| {
            !w.removed && w.line == line && ch >= w.start && ch < w.start + w.text.chars().count()
        })
    }

    fn bracket_at(&self, line: usize, ch: usize) -> Option<(usize, usize)> {
        self.bracket_spans(line)
            .into_iter()
            .find(|&(a, b)| ch >= a && ch <= b)
    }

    /// The word or bracket pair under the cursor.
    pub fn highlight(&self) -> Option<Highlight> {
        let line = self.cursor.line();
        if let Some(i) = self.word_at(line, self.cursor.ch) {
            let w = &self.words[i];
            return Some(Highlight {
                line,
                start: w.start,
                len: w.text.chars().count(),
                is_word: true,
            });
        }
        let (a, b) = self.bracket_at(line, self.cursor.ch)?;
        Some(Highlight {
            line,
            start: a,
            len: b - a + 1,
            is_word: false,
        })
    }

    fn push_log(&mut self, s: String) {
        self.log.push(s);
        if self.log.len() > 24 {
            self.log.remove(0);
        }
    }

    fn remove_dud(&mut self) -> bool {
        let duds: Vec<usize> = self
            .words
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.is_password && !w.removed && !w.tried)
            .map(|(i, _)| i)
            .collect();
        let Some(&i) = duds.choose(&mut self.rng) else {
            return false;
        };
        let (line, start, len) = {
            let w = &mut self.words[i];
            w.removed = true;
            (w.line, w.start, w.text.chars().count())
        };
        self.lines[line][start..start + len]
            .iter_mut()
            .for_each(|c| *c = '.');
        true
    }

    fn select_word(&mut self, i: usize) {
        if self.words[i].tried {
            return;
        }
        self.words[i].tried = true;
        let text = self.words[i].text.clone();
        self.push_log(format!(">{text}"));
        if self.words[i].is_password {
            self.push_log(">Access granted.".into());
            self.outcome = Some(IceOutcome::Cleared);
            return;
        }
        let k = likeness(&text, &self.password);
        self.push_log(">Entry denied.".into());
        self.push_log(format!(">Likeness={k}"));
        self.attempts = self.attempts.saturating_sub(1);
        if self.attempts == 0 {
            self.push_log(">TERMINAL LOCKED".into());
            self.pending_trace += TRACE_ON_LOCKOUT;
            self.lockout = Some(Duration::from_secs(LOCKOUT_SECS));
        }
    }

    fn select_bracket(&mut self, line: usize, a: usize, b: usize) {
        self.lines[line][a..=b].iter_mut().for_each(|c| *c = '.');
        if self.attempts < ATTEMPTS && self.rng.random_bool(0.25) {
            self.attempts = ATTEMPTS;
            self.push_log(">Allowance replenished.".into());
        } else if self.remove_dud() {
            self.push_log(">Dud removed.".into());
        }
    }

    fn sniff(&mut self) {
        if self.loadout.sniffer == 0 {
            return;
        }
        let Some(i) = self.word_at(self.cursor.line(), self.cursor.ch) else {
            return;
        };
        self.loadout.sniffer -= 1;
        let text = self.words[i].text.clone();
        let k = likeness(&text, &self.password);
        self.push_log(format!(">{text}"));
        self.push_log(format!(">Likeness={k}"));
    }

    fn decrypt(&mut self) {
        if self.loadout.decrypt == 0 {
            return;
        }
        if self.remove_dud() {
            self.loadout.decrypt -= 1;
            self.push_log(">Dud removed.".into());
        }
    }

    fn regenerate(&mut self) {
        self.attempt += 1;
        let (lines, words) = layout(&mut self.rng, &self.pool, &self.password, self.word_count);
        self.lines = lines;
        self.words = words;
        self.attempts = ATTEMPTS;
        self.lockout = None;
        self.log.clear();
    }
}

impl IceGame for Wordlock {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        if input == IceInput::Esc {
            self.outcome = Some(IceOutcome::Aborted {
                trace: TRACE_ON_ABORT,
            });
            return;
        }
        if self.lockout.is_some() {
            return;
        }
        let c = &mut self.cursor;
        match input {
            IceInput::Left => {
                if c.ch > 0 {
                    c.ch -= 1;
                } else if c.col == 1 {
                    c.col = 0;
                    c.ch = WIDTH - 1;
                }
            }
            IceInput::Right => {
                if c.ch + 1 < WIDTH {
                    c.ch += 1;
                } else if c.col == 0 {
                    c.col = 1;
                    c.ch = 0;
                }
            }
            IceInput::Up => c.row = c.row.saturating_sub(1),
            IceInput::Down => c.row = (c.row + 1).min(ROWS - 1),
            IceInput::Enter => {
                let line = c.line();
                let ch = c.ch;
                if let Some(i) = self.word_at(line, ch) {
                    self.select_word(i);
                } else if let Some((a, b)) = self.bracket_at(line, ch) {
                    self.select_bracket(line, a, b);
                }
            }
            IceInput::Char('1') => self.decrypt(),
            IceInput::Char('2') => self.sniff(),
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.outcome.is_some() {
            return;
        }
        if let Some(left) = self.lockout {
            let left = left.saturating_sub(dt);
            if left.is_zero() {
                self.regenerate();
            } else {
                self.lockout = Some(left);
            }
        }
    }

    fn outcome(&self) -> Option<IceOutcome> {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::params;
    use crate::rng::substream;

    fn make_words(len: usize, n: usize, rng: &mut Rng) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        while out.len() < n {
            let w: String = (0..len)
                .map(|_| char::from(b'A' + rng.random_range(0..6u8)))
                .collect();
            if !out.contains(&w) {
                out.push(w);
            }
        }
        out
    }

    fn game(seed: u64, d: u8, loadout: Loadout) -> Wordlock {
        let p = params(d);
        let mut rng = substream(seed, 0, "wl-test", 0);
        let words = make_words(p.wordlock_len, 2 * p.wordlock_words, &mut rng);
        let password = words[0].clone();
        Wordlock::generate(&mut rng, &p, &words, &password, 0, loadout)
    }

    fn goto(g: &mut Wordlock, line: usize, ch: usize) {
        g.cursor = Cursor {
            col: line / ROWS,
            row: line % ROWS,
            ch,
        };
    }

    fn check_layout(g: &Wordlock, d: u8) {
        let p = params(d);
        assert_eq!(g.words.iter().filter(|w| w.is_password).count(), 1);
        assert_eq!(g.words.len(), p.wordlock_words);
        let mut covered = vec![[false; WIDTH]; LINES];
        for w in &g.words {
            assert_eq!(w.text.chars().count(), p.wordlock_len);
            for i in 0..p.wordlock_len {
                assert!(!covered[w.line][w.start + i], "overlap");
                covered[w.line][w.start + i] = true;
                assert_eq!(g.lines[w.line][w.start + i], w.text.chars().nth(i).unwrap());
            }
        }
        // Every letter on the board belongs to a word.
        for (l, row) in g.lines.iter().enumerate() {
            for (i, c) in row.iter().enumerate() {
                assert_eq!(c.is_alphabetic(), covered[l][i]);
            }
        }
        let pairs: usize = (0..LINES).map(|l| g.bracket_spans(l).len()).sum();
        assert_eq!(pairs, 3);
        for l in 0..LINES {
            for (a, b) in g.bracket_spans(l) {
                assert!((2..=6).contains(&(b - a - 1)));
                assert!(g.lines[l][a..=b].iter().all(|c| !c.is_alphabetic()));
            }
        }
    }

    #[test]
    fn layouts_are_valid_for_all_danger_classes() {
        for d in 1..=5u8 {
            for seed in 0..500 {
                check_layout(&game(seed, d, Loadout::default()), d);
            }
        }
    }

    #[test]
    fn candidates_come_from_the_most_alike_half() {
        for seed in 0..100 {
            let g = game(seed, 3, Loadout::default());
            let pool = &g.pool;
            let mut others: Vec<&String> = pool.iter().filter(|w| **w != g.password).collect();
            others.sort_by_key(|w| std::cmp::Reverse(likeness(w, &g.password)));
            let top = &others[..2 * (g.word_count - 1)];
            for w in g.words.iter().filter(|w| !w.is_password) {
                assert!(top.contains(&&w.text));
            }
        }
    }

    #[test]
    fn dud_removal_never_removes_password_and_likeness_is_brute_force() {
        for d in 1..=5u8 {
            for seed in 0..500 {
                let mut g = game(
                    seed,
                    d,
                    Loadout {
                        decrypt: 255,
                        ..Loadout::default()
                    },
                );
                for _ in 0..20 {
                    g.handle(IceInput::Char('1'));
                }
                let pw = g.words.iter().find(|w| w.is_password).unwrap();
                assert!(!pw.removed);
                assert_eq!(g.words.iter().filter(|w| !w.removed).count(), 1);
                let g2 = game(seed, d, Loadout::default());
                for w in &g2.words {
                    let brute = (0..w.text.len())
                        .filter(|&i| w.text.as_bytes()[i] == g2.password.as_bytes()[i])
                        .count();
                    assert_eq!(likeness(&w.text, &g2.password), brute);
                }
            }
        }
    }

    #[test]
    fn selecting_password_clears() {
        let mut g = game(1, 2, Loadout::default());
        let pw = g.words.iter().find(|w| w.is_password).unwrap().clone();
        goto(&mut g, pw.line, pw.start + 1);
        g.handle(IceInput::Enter);
        assert_eq!(g.outcome(), Some(IceOutcome::Cleared));
    }

    #[test]
    fn wrong_pick_logs_three_lines_and_costs_an_attempt() {
        let mut g = game(2, 2, Loadout::default());
        let w = g.words.iter().find(|w| !w.is_password).unwrap().clone();
        goto(&mut g, w.line, w.start);
        g.handle(IceInput::Enter);
        let k = likeness(&w.text, g.password());
        assert_eq!(
            g.log,
            vec![
                format!(">{}", w.text),
                ">Entry denied.".to_string(),
                format!(">Likeness={k}")
            ]
        );
        assert_eq!(g.attempts, 3);
        // Re-selecting a tried word is free.
        g.handle(IceInput::Enter);
        assert_eq!(g.attempts, 3);
    }

    #[test]
    fn four_wrong_picks_lock_out_then_regenerate_with_same_password() {
        for seed in 0..50 {
            let mut g = game(seed, 3, Loadout::default());
            let before = g.lines.clone();
            let duds: Vec<WordSlot> = g.words.iter().filter(|w| !w.is_password).cloned().collect();
            for w in duds.iter().take(4) {
                goto(&mut g, w.line, w.start);
                g.handle(IceInput::Enter);
            }
            assert!(g.locked());
            assert_eq!(g.take_trace(), 15.0);
            assert_eq!(g.take_trace(), 0.0);
            assert!(g.log.iter().any(|l| l == ">TERMINAL LOCKED"));
            g.tick(Duration::from_millis(1900));
            assert!(g.locked());
            g.tick(Duration::from_millis(200));
            assert!(!g.locked());
            assert_eq!(g.attempts, ATTEMPTS);
            assert_eq!(g.attempt, 1);
            assert_eq!(g.password(), game(seed, 3, Loadout::default()).password());
            assert_ne!(g.lines, before);
            check_layout(&g, 3);
            assert!(g.outcome().is_none());
        }
    }

    #[test]
    fn esc_aborts_with_trace_five() {
        let mut g = game(3, 1, Loadout::default());
        g.handle(IceInput::Esc);
        assert_eq!(g.outcome(), Some(IceOutcome::Aborted { trace: 5.0 }));
    }

    #[test]
    fn bracket_removes_dud_at_full_attempts_and_is_single_use() {
        let mut g = game(4, 3, Loadout::default());
        let (line, (a, b)) = (0..LINES)
            .find_map(|l| g.bracket_spans(l).first().map(|s| (l, *s)))
            .unwrap();
        let before = g.words.iter().filter(|w| !w.removed).count();
        goto(&mut g, line, a);
        g.handle(IceInput::Enter);
        assert_eq!(g.words.iter().filter(|w| !w.removed).count(), before - 1);
        assert!(g.words.iter().any(|w| w.is_password && !w.removed));
        assert!(g.lines[line][a..=b].iter().all(|c| *c == '.'));
        assert!(g.bracket_spans(line).iter().all(|s| *s != (a, b)));
    }

    #[test]
    fn decrypt_and_sniffer_spend_charges() {
        let mut g = game(
            5,
            2,
            Loadout {
                decrypt: 1,
                sniffer: 1,
                ..Loadout::default()
            },
        );
        g.handle(IceInput::Char('1'));
        g.handle(IceInput::Char('1'));
        assert_eq!(g.loadout().decrypt, 0);
        assert_eq!(g.words.iter().filter(|w| w.removed).count(), 1);
        let w = g
            .words
            .iter()
            .find(|w| !w.removed && !w.is_password)
            .unwrap()
            .clone();
        goto(&mut g, w.line, w.start);
        g.handle(IceInput::Char('2'));
        assert_eq!(g.loadout().sniffer, 0);
        assert_eq!(g.attempts, ATTEMPTS);
        assert_eq!(
            g.log.last().unwrap(),
            &format!(">Likeness={}", likeness(&w.text, g.password()))
        );
    }

    #[test]
    fn cursor_crosses_columns_and_highlights_words() {
        let mut g = game(6, 1, Loadout::default());
        goto(&mut g, 3, WIDTH - 1);
        g.handle(IceInput::Right);
        assert_eq!(
            g.cursor,
            Cursor {
                col: 1,
                row: 3,
                ch: 0
            }
        );
        g.handle(IceInput::Left);
        assert_eq!(
            g.cursor,
            Cursor {
                col: 0,
                row: 3,
                ch: WIDTH - 1
            }
        );
        let w = g.words[0].clone();
        goto(&mut g, w.line, w.start + 2);
        let h = g.highlight().unwrap();
        assert_eq!(
            (h.line, h.start, h.len, h.is_word),
            (w.line, w.start, w.text.len(), true)
        );
        assert_eq!(Wordlock::address(2), 0xF4A0 + 24);
    }
}
