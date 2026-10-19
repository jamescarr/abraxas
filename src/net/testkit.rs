//! Test support: a tiny ANSI terminal that replays what the board sent, and a caller that holds
//! one at the end of a socket, so tests assert on what a caller would see instead of on escape
//! sequences.
//!
//! The terminal understands exactly what [`TelnetBackend`](super::TelnetBackend) and the session
//! emit: cursor moves, erase, colours (SGR), cursor visibility and the alternate screen, plus
//! plain text, CR and LF.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Size;
use ratatui::style::Color;

/// One character cell of the emulated screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Glyph {
    ch: char,
    fg: Color,
    bg: Color,
    bold: bool,
    reversed: bool,
}

impl Glyph {
    const BLANK: Glyph = Glyph {
        ch: ' ',
        fg: Color::Reset,
        bg: Color::Reset,
        bold: false,
        reversed: false,
    };
}

#[derive(Debug, Clone)]
struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<Glyph>,
}

impl Grid {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Glyph::BLANK; usize::from(cols) * usize::from(rows)],
        }
    }

    fn at(&mut self, x: u16, y: u16) -> Option<&mut Glyph> {
        (x < self.cols && y < self.rows)
            .then(|| &mut self.cells[usize::from(y) * usize::from(self.cols) + usize::from(x)])
    }

    fn rows(&self) -> Vec<String> {
        self.cells
            .chunks(usize::from(self.cols).max(1))
            .map(|row| row.iter().map(|g| g.ch).collect())
            .collect()
    }
}

/// Where a byte is inside a telnet command (see [`Term::feed_telnet`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Iac {
    Idle,
    Command,
    /// After `IAC WILL|WONT|DO|DONT`: the option byte comes next.
    Verb,
    /// Inside a subnegotiation.
    Sub,
}

/// An emulated terminal of `cols` x `rows` with a main and an alternate screen.
#[derive(Debug)]
pub(crate) struct Term {
    main: Grid,
    alt: Grid,
    pub alt_screen: bool,
    pub cursor_visible: bool,
    x: u16,
    y: u16,
    saved: (u16, u16),
    fg: Color,
    bg: Color,
    bold: bool,
    reversed: bool,
    /// Bytes of an escape sequence or character still incomplete.
    pending: Vec<u8>,
    iac: Iac,
}

impl Term {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            main: Grid::new(cols, rows),
            alt: Grid::new(cols, rows),
            alt_screen: false,
            cursor_visible: true,
            x: 0,
            y: 0,
            saved: (0, 0),
            fg: Color::Reset,
            bg: Color::Reset,
            bold: false,
            reversed: false,
            pending: Vec::new(),
            iac: Iac::Idle,
        }
    }

    /// Replay raw output (no telnet commands in it).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let pending = std::mem::take(&mut self.pending);
        let mut i = 0;
        while i < pending.len() {
            let rest = &pending[i..];
            match rest[0] {
                0x1b => {
                    if rest.len() < 2 {
                        break;
                    }
                    if rest[1] != b'[' {
                        i += 2;
                        continue;
                    }
                    let Some(end) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                        break;
                    };
                    self.csi(&rest[2..2 + end], rest[2 + end]);
                    i += 3 + end;
                }
                b'\r' => {
                    self.x = 0;
                    i += 1;
                }
                b'\n' => {
                    self.y = (self.y + 1).min(self.grid().rows.saturating_sub(1));
                    i += 1;
                }
                b if b < 0x20 => i += 1,
                lead => {
                    let need = match lead {
                        0x00..=0x7f => 1,
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    if rest.len() < need {
                        break;
                    }
                    let text = String::from_utf8_lossy(&rest[..need]);
                    self.put(text.chars().next().unwrap_or('?'));
                    i += need;
                }
            }
        }
        self.pending = pending[i..].to_vec();
    }

    /// Replay what a client receives: the same, after dropping telnet commands.
    pub fn feed_telnet(&mut self, bytes: &[u8]) {
        let mut data = Vec::with_capacity(bytes.len());
        for &b in bytes {
            self.iac = match (self.iac, b) {
                (Iac::Idle, 255) => Iac::Command,
                (Iac::Idle, _) => {
                    data.push(b);
                    Iac::Idle
                }
                (Iac::Command, 255) => {
                    data.push(255);
                    Iac::Idle
                }
                (Iac::Command, 251..=254) => Iac::Verb,
                (Iac::Command, 250) => Iac::Sub,
                (Iac::Command | Iac::Verb, _) => Iac::Idle,
                (Iac::Sub, 240) => Iac::Idle,
                (Iac::Sub, _) => Iac::Sub,
            };
        }
        self.feed(&data);
    }

    fn grid(&self) -> &Grid {
        if self.alt_screen {
            &self.alt
        } else {
            &self.main
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_screen {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    fn put(&mut self, ch: char) {
        let glyph = Glyph {
            ch,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            reversed: self.reversed,
        };
        let (x, y) = (self.x, self.y);
        if let Some(cell) = self.grid_mut().at(x, y) {
            *cell = glyph;
        }
        self.x = self.x.saturating_add(1);
    }

    fn csi(&mut self, params: &[u8], fin: u8) {
        let private = params.first() == Some(&b'?');
        let digits = if private { &params[1..] } else { params };
        let nums: Vec<u16> = digits
            .split(|&b| b == b';')
            .map(|p| {
                p.iter().fold(0u16, |n, &b| {
                    n.saturating_mul(10).saturating_add(u16::from(b - b'0'))
                })
            })
            .collect();
        match (private, fin) {
            (false, b'H') => {
                self.y = nums[0].max(1) - 1;
                self.x = nums.get(1).copied().unwrap_or(1).max(1) - 1;
            }
            (false, b'J') => self.erase_display(nums[0]),
            (false, b'K') => self.erase_line(nums[0]),
            (false, b'm') => self.sgr(&nums),
            (true, b'h' | b'l') => {
                let on = fin == b'h';
                for n in nums {
                    match n {
                        25 => self.cursor_visible = on,
                        1049 if on && !self.alt_screen => {
                            self.saved = (self.x, self.y);
                            self.alt = Grid::new(self.alt.cols, self.alt.rows);
                            self.alt_screen = true;
                        }
                        1049 if !on && self.alt_screen => {
                            self.alt_screen = false;
                            (self.x, self.y) = self.saved;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let (x, y) = (self.x, self.y);
        let grid = self.grid_mut();
        let cols = usize::from(grid.cols);
        let at = usize::from(y) * cols + usize::from(x);
        let range = match mode {
            0 => at..grid.cells.len(),
            1 => 0..(at + 1).min(grid.cells.len()),
            _ => 0..grid.cells.len(),
        };
        grid.cells[range].fill(Glyph::BLANK);
    }

    fn erase_line(&mut self, mode: u16) {
        let (x, y) = (self.x, self.y);
        let grid = self.grid_mut();
        let cols = usize::from(grid.cols);
        let start = usize::from(y) * cols;
        let at = start + usize::from(x);
        let range = match mode {
            0 => at..start + cols,
            1 => start..(at + 1).min(start + cols),
            _ => start..start + cols,
        };
        if range.end <= grid.cells.len() {
            grid.cells[range].fill(Glyph::BLANK);
        }
    }

    fn sgr(&mut self, nums: &[u16]) {
        let named = |base: u16, n: u16| match n - base {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            _ => Color::Gray,
        };
        let bright = |base: u16, n: u16| match n - base {
            0 => Color::DarkGray,
            1 => Color::LightRed,
            2 => Color::LightGreen,
            3 => Color::LightYellow,
            4 => Color::LightBlue,
            5 => Color::LightMagenta,
            6 => Color::LightCyan,
            _ => Color::White,
        };
        let mut it = nums.iter().copied();
        while let Some(n) = it.next() {
            match n {
                0 => {
                    (self.fg, self.bg) = (Color::Reset, Color::Reset);
                    (self.bold, self.reversed) = (false, false);
                }
                1 => self.bold = true,
                7 => self.reversed = true,
                30..=37 => self.fg = named(30, n),
                90..=97 => self.fg = bright(90, n),
                39 => self.fg = Color::Reset,
                40..=47 => self.bg = named(40, n),
                100..=107 => self.bg = bright(100, n),
                49 => self.bg = Color::Reset,
                38 | 48 => {
                    let colour = match it.next() {
                        Some(5) => it
                            .next()
                            .map(|i| Color::Indexed(u8::try_from(i).unwrap_or(0))),
                        Some(2) => {
                            let mut c = || u8::try_from(it.next().unwrap_or(0)).unwrap_or(0);
                            let (r, g, b) = (c(), c(), c());
                            Some(Color::Rgb(r, g, b))
                        }
                        _ => None,
                    };
                    if let Some(colour) = colour {
                        if n == 38 {
                            self.fg = colour;
                        } else {
                            self.bg = colour;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The text of the screen the caller currently sees, one line per row.
    pub fn text(&self) -> String {
        self.grid().rows().join("\n")
    }

    /// The text of the main screen (what is left once the board leaves the alternate screen).
    pub fn main_text(&self) -> String {
        self.main.rows().join("\n")
    }

    /// The text of the alternate screen, showing or not: the last thing the board drew there.
    pub fn alt_text(&self) -> String {
        self.alt.rows().join("\n")
    }

    /// Where `needle` first appears on the screen the caller sees, as (column, row).
    pub fn find(&self, needle: &str) -> Option<(usize, usize)> {
        self.grid()
            .rows()
            .iter()
            .enumerate()
            .find_map(|(y, row)| row.find(needle).map(|at| (row[..at].chars().count(), y)))
    }

    /// Every cell of the current screen that differs from `expected` (symbol, colours, bold and
    /// reverse video), described for a failure message.
    pub fn mismatches(&self, expected: &Buffer) -> Vec<String> {
        use ratatui::style::Modifier;
        let grid = self.grid();
        let mut wrong = Vec::new();
        for y in 0..grid.rows {
            for x in 0..grid.cols {
                let want = &expected[(x, y)];
                let got = grid.cells[usize::from(y) * usize::from(grid.cols) + usize::from(x)];
                let wanted = Glyph {
                    ch: want.symbol().chars().next().unwrap_or(' '),
                    fg: want.fg,
                    bg: want.bg,
                    bold: want.modifier.contains(Modifier::BOLD),
                    reversed: want.modifier.contains(Modifier::REVERSED),
                };
                if got != wanted {
                    wrong.push(format!("({x},{y}): got {got:?}, wanted {wanted:?}"));
                }
            }
        }
        wrong
    }
}

/// How long a test waits for anything the board should do.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

/// A caller at the far end of a socket, with a screen to look at.
pub(crate) struct Caller {
    stream: TcpStream,
    pub screen: Term,
    /// Every byte received, telnet commands included.
    pub raw: Vec<u8>,
}

impl Caller {
    /// A caller whose terminal is `window` big.
    pub fn new(stream: TcpStream, window: Size) -> Self {
        Self {
            stream,
            screen: Term::new(window.width, window.height),
            raw: Vec::new(),
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).unwrap();
    }

    /// Take in what the board sends for up to `wait`; false once it has closed the line.
    pub fn pump(&mut self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        let mut buf = [0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            self.stream.set_read_timeout(Some(left)).unwrap();
            match self.stream.read(&mut buf) {
                Ok(0) => return false,
                Ok(n) => {
                    self.raw.extend_from_slice(&buf[..n]);
                    self.screen.feed_telnet(&buf[..n]);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return true;
                }
                // A reset is the board closing with our last keys unread.
                Err(_) => return false,
            }
        }
    }

    /// Wait until `ok` holds of the screen; `what` names it in the failure.
    pub fn wait_until(&mut self, what: &str, ok: impl Fn(&Term) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if ok(&self.screen) {
                return;
            }
            if !self.pump(Duration::from_millis(20)) {
                break;
            }
        }
        panic!(
            "never saw {what}; the screen shows:\n{}",
            self.screen.text()
        );
    }

    pub fn wait_for(&mut self, needle: &str) {
        self.wait_until(&format!("{needle:?}"), |t| t.text().contains(needle));
    }

    /// Wait for the board to close the line; false if it did not within the patience.
    pub fn hung_up(&mut self) -> bool {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if !self.pump(Duration::from_millis(20)) {
                return true;
            }
        }
        false
    }
}
