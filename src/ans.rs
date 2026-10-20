//! ANSI/CP437 art: SAUCE metadata, decoding, directory scanning and an 80-column canvas.
//!
//! Artpacks are CP437 bytes with ANSI escape sequences and an optional SAUCE record at the end.
//! The canvas interprets the subset of ANSI that real art uses (SGR colours, cursor movement,
//! save/restore, erase) so both the full-screen viewer and the hidden line draw the art as its
//! author intended.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::palette::Dos;
use crate::text::{CP437_TABLE, parse_pipe};

/// Columns in an ANSI canvas; art wider than this wraps.
pub const COLS: usize = 80;
/// Rows a canvas can grow to; writes past this are dropped.
pub const MAX_ROWS: usize = 4000;
/// Most files `scan` returns.
pub const MAX_FILES: usize = 5000;
/// Files `scan` accepts, by lower-case extension.
pub const EXTENSIONS: [&str; 2] = ["ans", "ice"];

/// How many directory levels below the starting directory `scan` descends.
const MAX_DEPTH: usize = 4;
/// Longest parameter/intermediate run kept for one escape sequence; a longer one is abandoned.
const MAX_CSI: usize = 64;
/// The SAUCE header is the last 128 bytes of a file.
const SAUCE_LEN: usize = 128;

/// SGR colour (0-7, black first) to DOS colour index.
const SGR_TO_DOS: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

const BLANK_CELL: Cell = Cell {
    ch: ' ',
    fg: Dos::LightGray,
    bg: Dos::Black,
};
static BLANK_ROW: [Cell; COLS] = [BLANK_CELL; COLS];

/// The SAUCE record (128 bytes after the 0x1A EOF byte), when present and starting with
/// `SAUCE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sauce {
    pub title: String,
    pub author: String,
    pub group: String,
    pub ice_colors: bool,
}

/// Parse the SAUCE header at the end of `bytes`, if any.
pub fn sauce(bytes: &[u8]) -> Option<Sauce> {
    let start = bytes.len().checked_sub(SAUCE_LEN)?;
    let record = &bytes[start..];
    if !record.starts_with(b"SAUCE") {
        return None;
    }
    Some(Sauce {
        title: sauce_text(&record[7..42]),
        author: sauce_text(&record[42..62]),
        group: sauce_text(&record[62..82]),
        ice_colors: record[105] & 1 == 1,
    })
}

/// A CP437 field, trailing spaces and NULs trimmed.
fn sauce_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |i| i + 1);
    bytes[..end]
        .iter()
        .map(|&b| CP437_TABLE[usize::from(b)])
        .collect()
}

/// Bytes to the decoded text the game stores and streams: every byte up to the first 0x1A maps
/// through the CP437 table, except the controls that keep their meaning in a terminal.
pub fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0x1A)
        .map(|&b| match b {
            0x08 => '\u{8}',
            0x09 => '\t',
            0x0A => '\n',
            0x0D => '\r',
            0x1B => '\u{1b}',
            _ => CP437_TABLE[usize::from(b)],
        })
        .collect()
}

/// Pipe-coded text (`|00`-`|15` foreground, `|16`-`|23` background) as ANSI text that the canvas
/// and a terminal both draw: an absolute 16-colour SGR per span, CRLF line ends, a final reset.
pub fn from_pipe(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in parse_pipe(text).into_iter().enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        for span in line {
            let fg = span.fg.unwrap_or(7) % 16;
            let bg = span.bg.unwrap_or(0) % 8;
            out.push_str("\u{1b}[0");
            if fg >= 8 {
                out.push_str(";1");
            }
            // The DOS/SGR colour order swap is its own inverse.
            let fg_sgr = 30 + SGR_TO_DOS[usize::from(fg % 8)];
            let bg_sgr = 40 + SGR_TO_DOS[usize::from(bg)];
            out.push_str(&format!(";{fg_sgr};{bg_sgr}m"));
            out.push_str(&span.text);
        }
    }
    out.push_str("\u{1b}[0m");
    out
}

/// The canvas as explicit terminal rows, for a client of any width: a 256-colour SGR per colour
/// run, a reset and CRLF at every row end, trailing blank cells dropped. Cursor moves, wraps at
/// column 80 and iCE colours are resolved here, so the client needs no ANSI.SYS behaviour.
pub fn to_terminal(text: &str, ice_colors: bool) -> String {
    let mut canvas = Canvas::new(ice_colors);
    canvas.feed_str(text);
    let mut out = String::new();
    for r in 0..canvas.rows() {
        let row = canvas.row(r);
        let end = row
            .iter()
            .rposition(|cell| *cell != Cell::default())
            .map_or(0, |i| i + 1);
        let mut style: Option<(Dos, Dos)> = None;
        for cell in &row[..end] {
            if style != Some((cell.fg, cell.bg)) {
                out.push_str(&format!(
                    "\u{1b}[38;5;{};48;5;{}m",
                    cell.fg.ansi_index(),
                    cell.bg.ansi_index()
                ));
                style = Some((cell.fg, cell.bg));
            }
            out.push(cell.ch);
        }
        out.push_str("\u{1b}[0m\r\n");
    }
    out
}

/// `.ans`/`.ice` by extension, case-insensitive.
pub fn is_ans_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| {
            EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// 8.3 upper-case inventory name: the stem truncated to 8 with anything outside `[A-Za-z0-9_-]`
/// turned into `_`, extension `ICE` when `ice` else `ANS`.
pub fn display_name(path: &Path, ice: bool) -> String {
    let stem = path.file_stem().and_then(OsStr::to_str).unwrap_or_default();
    let stem: String = stem
        .chars()
        .take(8)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let ext = if ice { "ICE" } else { "ANS" };
    format!("{stem}.{ext}")
}

/// Every `.ans`/`.ice` under `dir` (recursive, at most 4 levels, at most [`MAX_FILES`]),
/// sorted by path. Empty when `dir` is missing or unreadable (logged at debug).
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), 0_usize)];
    while let Some((current, depth)) = dirs.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::debug!("ans: cannot read {}: {e}", current.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_FILES {
                break;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && depth < MAX_DEPTH => {
                    dirs.push((path, depth + 1));
                }
                Ok(kind) if kind.is_file() && is_ans_path(&path) => found.push(path),
                _ => {}
            }
        }
    }
    found.sort();
    found.truncate(MAX_FILES);
    found
}

fn is_ans_path(path: &Path) -> bool {
    path.to_str().is_some_and(is_ans_name)
}

/// One character cell of a canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Dos,
    pub bg: Dos,
}

impl Default for Cell {
    fn default() -> Self {
        BLANK_CELL
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum EscState {
    #[default]
    None,
    Esc,
    Csi {
        params: String,
    },
}

/// An 80-column ANSI screen: feed decoded text, read the cells back.
#[derive(Debug, Clone)]
pub struct Canvas {
    cells: Vec<Cell>,
    rows_used: usize,
    row: usize,
    col: usize,
    saved: Option<(usize, usize)>,
    fg: u8,
    bg: u8,
    bright: bool,
    blink: bool,
    ice: bool,
    esc: EscState,
}

impl Canvas {
    /// A blank canvas. `ice_colors` makes blink mean a bright background.
    pub fn new(ice_colors: bool) -> Self {
        Self {
            cells: Vec::new(),
            rows_used: 1,
            row: 0,
            col: 0,
            saved: None,
            fg: 7,
            bg: 0,
            bright: false,
            blink: false,
            ice: ice_colors,
            esc: EscState::None,
        }
    }

    /// Feed one decoded character.
    pub fn feed(&mut self, c: char) {
        match std::mem::replace(&mut self.esc, EscState::None) {
            EscState::None => self.plain(c),
            EscState::Esc => match c {
                '[' => {
                    self.esc = EscState::Csi {
                        params: String::new(),
                    };
                }
                // A second ESC restarts the sequence; anything else drops both characters.
                '\u{1b}' => self.esc = EscState::Esc,
                _ => {}
            },
            EscState::Csi { mut params } => match c {
                // Parameter bytes (digits, `;`, `:` and the private markers `< = > ?`) and
                // intermediate bytes (space to `/`) keep the sequence open; a final byte
                // (`@` to `~`) ends it.
                '0'..='?' | ' '..='/' if params.len() < MAX_CSI => {
                    params.push(c);
                    self.esc = EscState::Csi { params };
                }
                '@'..='~' => self.csi(&params, c),
                // A control character or a runaway sequence is abandoned.
                _ => {}
            },
        }
    }

    /// Feed decoded text.
    pub fn feed_str(&mut self, s: &str) {
        for c in s.chars() {
            self.feed(c);
        }
    }

    /// Rows with something written, or an explicit cursor move to them, at least 1. A trailing
    /// newline does not add a row.
    pub fn rows(&self) -> usize {
        self.rows_used
    }

    /// The cursor's row.
    pub fn cursor_row(&self) -> usize {
        self.row
    }

    /// One row of [`COLS`] cells; blank beyond [`rows`](Self::rows).
    pub fn row(&self, r: usize) -> &[Cell] {
        let slice = r
            .checked_mul(COLS)
            .and_then(|start| start.checked_add(COLS).map(|end| start..end))
            .and_then(|range| self.cells.get(range));
        slice.unwrap_or(BLANK_ROW.as_slice())
    }

    fn plain(&mut self, c: char) {
        match c {
            '\u{1b}' => self.esc = EscState::Esc,
            '\n' => self.newline(),
            '\r' => self.col = 0,
            '\t' => self.tab(),
            '\u{8}' => self.col = self.col.saturating_sub(1),
            _ => self.put(c),
        }
    }

    fn put(&mut self, ch: char) {
        if self.row >= MAX_ROWS {
            return;
        }
        let fg = self.fg + 8 * u8::from(self.bright);
        let bg = self.bg + 8 * u8::from(self.blink && self.ice);
        let cell = Cell {
            ch,
            fg: Dos::from_index(fg).unwrap_or(Dos::LightGray),
            bg: Dos::from_index(bg).unwrap_or(Dos::Black),
        };
        let end = (self.row + 1) * COLS;
        if self.cells.len() < end {
            self.cells.resize(end, Cell::default());
        }
        self.cells[self.row * COLS + self.col] = cell;
        self.touch(self.row);
        self.col += 1;
        if self.col >= COLS {
            self.newline();
        }
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row = self.row.saturating_add(1).min(MAX_ROWS);
    }

    fn tab(&mut self) {
        let mut next = (self.col / 8 + 1) * 8;
        while next >= COLS {
            next -= COLS;
            self.newline();
        }
        self.col = next;
    }

    fn touch(&mut self, row: usize) {
        if row < MAX_ROWS {
            self.rows_used = self.rows_used.max(row + 1);
        }
    }

    fn csi(&mut self, params: &str, final_char: char) {
        // Private (`?7h`), intermediate-byte and sub-parameter (`38:5:n`) forms are not the
        // sequences art draws with: they are consumed and ignored, whatever their final byte.
        if !params.chars().all(|c| c.is_ascii_digit() || c == ';') {
            return;
        }
        let values: Vec<u32> = if params.is_empty() {
            Vec::new()
        } else {
            params
                .split(';')
                .map(|p| p.parse::<u32>().unwrap_or(0))
                .collect()
        };
        match final_char {
            'm' => self.sgr_params(&values),
            'A' => {
                self.row = self.row.saturating_sub(count(&values, 0)).min(MAX_ROWS - 1);
                self.touch(self.row);
            }
            'B' => {
                self.row = self.row.saturating_add(count(&values, 0)).min(MAX_ROWS - 1);
                self.touch(self.row);
            }
            'C' => {
                self.col = self.col.saturating_add(count(&values, 0)).min(COLS - 1);
            }
            'D' => {
                self.col = self.col.saturating_sub(count(&values, 0));
            }
            'H' | 'f' => {
                self.row = count(&values, 0).saturating_sub(1).min(MAX_ROWS - 1);
                self.col = count(&values, 1).saturating_sub(1).min(COLS - 1);
                self.touch(self.row);
            }
            's' => self.saved = Some((self.row, self.col)),
            'u' => {
                if let Some((row, col)) = self.saved {
                    self.row = row.min(MAX_ROWS - 1);
                    self.col = col.min(COLS - 1);
                    self.touch(self.row);
                }
            }
            'J' => self.erase_display(values.first().copied().unwrap_or(2)),
            'K' => self.erase_line(values.first().copied().unwrap_or(0)),
            _ => {}
        }
    }

    fn sgr(&mut self, value: u32) {
        match value {
            0 => {
                self.fg = 7;
                self.bg = 0;
                self.bright = false;
                self.blink = false;
            }
            1 => self.bright = true,
            2 | 22 => self.bright = false,
            5 => self.blink = true,
            25 => self.blink = false,
            7 => std::mem::swap(&mut self.fg, &mut self.bg),
            30..=37 => self.fg = sgr_dos(value - 30),
            39 => self.fg = 7,
            40..=47 => self.bg = sgr_dos(value - 40),
            49 => self.bg = 0,
            90..=97 => {
                self.fg = sgr_dos(value - 90);
                self.bright = true;
            }
            100..=107 => {
                self.bg = sgr_dos(value - 100);
                self.blink = true;
            }
            _ => {}
        }
    }

    /// Apply an SGR parameter list. The extended-colour forms (`38;5;n`, `38;2;r;g;b` and their
    /// `48` twins) are consumed as a unit so their numbers are never read as attributes.
    fn sgr_params(&mut self, values: &[u32]) {
        if values.is_empty() {
            self.sgr(0);
            return;
        }
        let mut i = 0;
        while i < values.len() {
            let value = values[i];
            i += 1;
            if value != 38 && value != 48 {
                self.sgr(value);
                continue;
            }
            match values.get(i) {
                Some(5) => {
                    if let Some(&n) = values.get(i + 1) {
                        self.indexed(value == 38, n);
                    }
                    i += 2;
                }
                Some(2) => i += 4,
                _ => {}
            }
        }
    }

    /// One of the first 16 colours of the 256-colour palette; the rest have no 16-colour
    /// equivalent and are left as they were.
    fn indexed(&mut self, foreground: bool, n: u32) {
        if n >= 16 {
            return;
        }
        let colour = sgr_dos(n % 8);
        let bright = n >= 8;
        if foreground {
            self.fg = colour;
            self.bright = bright;
        } else {
            self.bg = colour;
            self.blink = bright;
        }
    }

    fn erase_display(&mut self, mode: u32) {
        match mode {
            0 => {
                let start = (self.row * COLS + self.col).min(self.cells.len());
                self.cells[start..].fill(Cell::default());
            }
            1 => {
                let end = (self.row * COLS + self.col + 1).min(self.cells.len());
                self.cells[..end].fill(Cell::default());
            }
            _ => {
                self.cells.fill(Cell::default());
                self.row = 0;
                self.col = 0;
                self.rows_used = 1;
            }
        }
    }

    fn erase_line(&mut self, mode: u32) {
        if self.row >= MAX_ROWS {
            return;
        }
        let start = self.row * COLS;
        let end = start + COLS;
        if self.cells.len() < end {
            self.cells.resize(end, Cell::default());
        }
        let (from, to) = match mode {
            1 => (start, start + self.col + 1),
            2 => (start, end),
            _ => (start + self.col, end),
        };
        self.cells[from..to].fill(Cell::default());
    }
}

/// An SGR move parameter: absent or zero means one.
fn count(values: &[u32], index: usize) -> usize {
    match values.get(index) {
        Some(0) | None => 1,
        Some(&n) => usize::try_from(n).unwrap_or(usize::MAX),
    }
}

fn sgr_dos(n: u32) -> u8 {
    SGR_TO_DOS[usize::try_from(n).unwrap_or(0).min(SGR_TO_DOS.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_stops_at_eof_and_keeps_controls() {
        assert_eq!(
            decode(b"A\x1b[31mB\xdb\x1aSAUCE00..."),
            "A\u{1b}[31mB\u{2588}"
        );
        assert_eq!(decode(b"a\tb\r\nc"), "a\tb\r\nc");
    }

    #[test]
    fn sauce_reads_the_last_record() {
        let mut file = b"\x1a".to_vec();
        let mut record = [0_u8; 128];
        record[..5].copy_from_slice(b"SAUCE");
        record[5..7].copy_from_slice(b"00");
        record[7..12].copy_from_slice(b"TITLE");
        record[42..48].copy_from_slice(b"AUTHOR");
        record[62..66].copy_from_slice(b"ACiD");
        record[105] = 1;
        file.extend_from_slice(&record);
        let parsed = sauce(&file).unwrap();
        assert_eq!(parsed.title, "TITLE");
        assert_eq!(parsed.author, "AUTHOR");
        assert_eq!(parsed.group, "ACiD");
        assert!(parsed.ice_colors);
        assert!(sauce(b"nope").is_none());
        assert!(sauce(&record).is_some());
        assert!(sauce(&record[..127]).is_none());
    }

    #[test]
    fn feed_lines_and_wraps() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("ab\r\ncd");
        assert_eq!(canvas.rows(), 2);
        assert_eq!(canvas.row(0)[0].ch, 'a');
        assert_eq!(canvas.row(0)[1].ch, 'b');
        assert_eq!(canvas.row(1)[0].ch, 'c');
        assert_eq!(canvas.row(1)[1].ch, 'd');

        let mut wide = Canvas::new(false);
        wide.feed_str(&"x".repeat(81));
        assert_eq!(wide.rows(), 2);
        assert_eq!(wide.row(1)[0].ch, 'x');
        assert_eq!(wide.cursor_row(), 1);
    }

    #[test]
    fn sgr_colours_use_dos_order() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("\u{1b}[1;31mX");
        assert_eq!(canvas.row(0)[0].fg, Dos::LightRed);

        let mut ice = Canvas::new(true);
        ice.feed_str("\u{1b}[5;44mX");
        assert_eq!(ice.row(0)[0].bg, Dos::LightBlue);
        let mut plain = Canvas::new(false);
        plain.feed_str("\u{1b}[5;44mX");
        assert_eq!(plain.row(0)[0].bg, Dos::Blue);
    }

    #[test]
    fn cursor_position_and_restore() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("\u{1b}[3;5HX");
        assert_eq!(canvas.rows(), 3);
        assert_eq!(canvas.row(2)[4].ch, 'X');

        let mut saved = Canvas::new(false);
        saved.feed_str("abc\u{1b}[s\r\n\u{1b}[uZ");
        assert_eq!(saved.row(0)[3].ch, 'Z');
    }

    #[test]
    fn erase_resets_the_display() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("hello\nworld");
        canvas.feed_str("\u{1b}[2J");
        assert_eq!(canvas.rows(), 1);
        assert_eq!(canvas.row(0)[0].ch, ' ');
        assert_eq!(canvas.cursor_row(), 0);
    }

    #[test]
    fn names_and_scanning() {
        assert!(is_ans_name("a.ANS"));
        assert!(is_ans_name("a.ice"));
        assert!(!is_ans_name("a.txt"));
        assert_eq!(
            display_name(Path::new("p/LongFileName.ans"), true),
            "LONGFILE.ICE"
        );
        assert_eq!(display_name(Path::new("a/b.ans"), false), "B.ANS");

        let dir = tempfile::tempdir().unwrap();
        for (path, body) in [
            ("a/x.ANS", &b"x"[..]),
            ("b/y.ice", &b"y"[..]),
            ("c/z.txt", &b"z"[..]),
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        let found = scan(dir.path());
        assert_eq!(
            found,
            vec![dir.path().join("a/x.ANS"), dir.path().join("b/y.ice")]
        );
        assert!(scan(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn display_names_are_8_3_safe() {
        assert_eq!(
            display_name(Path::new("p/my art 1.ans"), false),
            "MY_ART_1.ANS"
        );
        assert_eq!(
            display_name(Path::new("p/ACID-054.ANS"), false),
            "ACID-054.ANS"
        );
    }

    #[test]
    fn pipe_coded_art_becomes_sgr_text_the_canvas_draws() {
        let ansi = from_pipe("|12red|07 plain\n|09x");
        assert_eq!(
            ansi,
            "\u{1b}[0;1;31;40mred\u{1b}[0;37;40m plain\r\n\u{1b}[0;1;34;40mx\u{1b}[0m"
        );
        assert_eq!(from_pipe("|20|15hi"), "\u{1b}[0;1;37;41mhi\u{1b}[0m");
        let mut canvas = Canvas::new(false);
        canvas.feed_str(&ansi);
        assert_eq!(canvas.rows(), 2);
        assert_eq!(
            (canvas.row(0)[0].ch, canvas.row(0)[0].fg),
            ('r', Dos::LightRed)
        );
        assert_eq!(canvas.row(0)[3].fg, Dos::LightGray);
        assert_eq!(canvas.row(1)[0].fg, Dos::LightBlue);
    }

    #[test]
    fn private_and_intermediate_sequences_are_consumed_whole() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("\u{1b}[?7h\u{1b}[=7h\u{1b}[?25l\u{1b}[0 qX");
        assert_eq!(canvas.row(0)[0].ch, 'X');
        assert_eq!(canvas.rows(), 1);
        // A private-mode sequence ending in a final byte we do implement is not that command.
        let mut private = Canvas::new(false);
        private.feed_str("ab\u{1b}[?1HX");
        assert_eq!(private.row(0)[2].ch, 'X');
    }

    #[test]
    fn a_sequence_split_across_feeds_still_parses() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("\u{1b}[1;3");
        canvas.feed_str("1mX");
        assert_eq!(canvas.row(0)[0].fg, Dos::LightRed);
    }

    #[test]
    fn extended_colours_are_mapped_or_skipped_never_read_as_attributes() {
        let mut canvas = Canvas::new(false);
        canvas.feed_str("\u{1b}[38;5;9mA\u{1b}[38;2;1;2;3mB\u{1b}[38:5:196mC");
        // Index 9 is bright red; the truecolour and colon forms leave it alone.
        for x in 0..3 {
            assert_eq!(canvas.row(0)[x].fg, Dos::LightRed, "column {x}");
        }
        // None of the numbers was taken for SGR 5 (blink), which iCE would show as a bright
        // background.
        let mut ice = Canvas::new(true);
        ice.feed_str("\u{1b}[38;5;196mX\u{1b}[48;5;4mY");
        assert_eq!(ice.row(0)[0].bg, Dos::Black);
        assert_eq!(ice.row(0)[1].bg, Dos::Blue);
    }

    #[test]
    fn terminal_rows_do_not_depend_on_the_client_width() {
        assert_eq!(
            to_terminal("ab   \r\ncd", false),
            "\u{1b}[38;5;7;48;5;0mab\u{1b}[0m\r\n\u{1b}[38;5;7;48;5;0mcd\u{1b}[0m\r\n"
        );
        // A full row wraps in the canvas where ANSI.SYS wrapped it, so what follows lands on the
        // next row whatever the client's width.
        let rows = to_terminal(&format!("{}\u{1b}[2Cy", "x".repeat(80)), false);
        let lines: Vec<&str> = rows.split("\r\n").collect();
        assert_eq!(lines.len(), 3, "{rows:?}");
        assert_eq!(
            lines[0],
            format!("\u{1b}[38;5;7;48;5;0m{}\u{1b}[0m", "x".repeat(80))
        );
        assert_eq!(lines[1], "\u{1b}[38;5;7;48;5;0m  y\u{1b}[0m");
        // iCE: SGR 5 is a bright background, so it is sent as one.
        assert_eq!(
            to_terminal("\u{1b}[5;44m \u{1b}[0mx", true),
            "\u{1b}[38;5;7;48;5;12m \u{1b}[38;5;7;48;5;0mx\u{1b}[0m\r\n"
        );
    }
}
