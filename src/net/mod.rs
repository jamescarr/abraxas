//! The telnet board: the same game as `ui::run`, played over a socket.
//!
//! [`telnet`] frames the byte stream, [`keys`] turns it into key events, [`TelnetBackend`] draws
//! ratatui frames as ANSI over the socket, [`session`] runs one caller and [`server`] is the
//! listener with its single line.

pub mod keys;
pub mod server;
pub mod session;
pub mod telnet;

#[cfg(test)]
mod testkit;

use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, PoisonError};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

/// What a cell looks like to the client: colours, bold and reverse video. The game paints only
/// colours; the rest of the modifiers are not sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Look {
    fg: Color,
    bg: Color,
    bold: bool,
    reversed: bool,
}

impl Look {
    fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            bold: cell.modifier.contains(Modifier::BOLD),
            reversed: cell.modifier.contains(Modifier::REVERSED),
        }
    }

    /// Switch the client to this look: reset everything, then set what differs from the default.
    fn write(self, buf: &mut Vec<u8>) -> io::Result<()> {
        buf.extend_from_slice(b"\x1b[0m");
        if self.bold {
            buf.extend_from_slice(b"\x1b[1m");
        }
        if self.reversed {
            buf.extend_from_slice(b"\x1b[7m");
        }
        if self.fg != Color::Reset {
            write_color(buf, self.fg, false)?;
        }
        if self.bg != Color::Reset {
            write_color(buf, self.bg, true)?;
        }
        Ok(())
    }
}

/// `ESC[..m` for one colour: the 16 named colours as their classic codes, `Indexed` and `Rgb`
/// through the 256-colour and truecolor forms.
fn write_color(buf: &mut Vec<u8>, color: Color, background: bool) -> io::Result<()> {
    let base: u8 = if background { 40 } else { 30 };
    match color {
        Color::Reset => write!(buf, "\x1b[{}m", base + 9),
        Color::Black => write!(buf, "\x1b[{base}m"),
        Color::Red => write!(buf, "\x1b[{}m", base + 1),
        Color::Green => write!(buf, "\x1b[{}m", base + 2),
        Color::Yellow => write!(buf, "\x1b[{}m", base + 3),
        Color::Blue => write!(buf, "\x1b[{}m", base + 4),
        Color::Magenta => write!(buf, "\x1b[{}m", base + 5),
        Color::Cyan => write!(buf, "\x1b[{}m", base + 6),
        Color::Gray => write!(buf, "\x1b[{}m", base + 7),
        Color::DarkGray => write!(buf, "\x1b[{}m", base + 60),
        Color::LightRed => write!(buf, "\x1b[{}m", base + 61),
        Color::LightGreen => write!(buf, "\x1b[{}m", base + 62),
        Color::LightYellow => write!(buf, "\x1b[{}m", base + 63),
        Color::LightBlue => write!(buf, "\x1b[{}m", base + 64),
        Color::LightMagenta => write!(buf, "\x1b[{}m", base + 65),
        Color::LightCyan => write!(buf, "\x1b[{}m", base + 66),
        Color::White => write!(buf, "\x1b[{}m", base + 67),
        Color::Rgb(r, g, b) => write!(buf, "\x1b[{};2;{r};{g};{b}m", base + 8),
        Color::Indexed(n) => write!(buf, "\x1b[{};5;{n}m", base + 8),
    }
}

/// `ESC[row;colH` (both 1-based).
fn move_to(buf: &mut Vec<u8>, x: u16, y: u16) -> io::Result<()> {
    write!(buf, "\x1b[{};{}H", u32::from(y) + 1, u32::from(x) + 1)
}

/// A ratatui backend that draws over a telnet socket: ANSI output, a size that comes from NAWS
/// (80x25 until the client says otherwise), no local terminal involved.
#[derive(Debug)]
pub struct TelnetBackend {
    /// A clone of the connection: the write half.
    out: TcpStream,
    /// The client's window size, updated by the session as NAWS reports arrive.
    size: Arc<Mutex<Size>>,
    /// Output of the current frame, sent by `flush`.
    buf: Vec<u8>,
    cursor: Position,
    cursor_shown: bool,
}

impl TelnetBackend {
    pub fn new(out: TcpStream, size: Arc<Mutex<Size>>) -> Self {
        Self {
            out,
            size,
            buf: Vec::with_capacity(8 * 1024),
            cursor: Position::ORIGIN,
            // A client starts with a visible cursor.
            cursor_shown: true,
        }
    }

    /// Switch the client to the alternate screen and hide the cursor (`ESC[?1049h ESC[?25l`); the
    /// first frame follows.
    pub fn enter(&mut self) -> io::Result<()> {
        self.buf.extend_from_slice(b"\x1b[?1049h\x1b[?25l");
        self.cursor_shown = false;
        self.flush()
    }

    /// Leave the alternate screen, show the cursor, reset colours (`ESC[0m ESC[?25h ESC[?1049l`).
    pub fn leave(&mut self) -> io::Result<()> {
        self.buf.extend_from_slice(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        self.cursor_shown = true;
        self.flush()
    }
}

impl Backend for TelnetBackend {
    type Error = io::Error;

    /// Every changed cell, left to right: a cursor move when the cell does not follow the
    /// previous one, a colour change when its look differs, then its symbol. The client is back
    /// at the default look when the call ends, so later clears do not paint with a stale
    /// background.
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut look = Look::default();
        let mut next = None;
        for (x, y, cell) in content {
            if next != Some((x, y)) {
                move_to(&mut self.buf, x, y)?;
            }
            let wanted = Look::of(cell);
            if wanted != look {
                wanted.write(&mut self.buf)?;
                look = wanted;
            }
            self.buf.extend_from_slice(cell.symbol().as_bytes());
            next = Some((x.saturating_add(1), y));
        }
        if look != Look::default() {
            self.buf.extend_from_slice(b"\x1b[0m");
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            self.buf.extend_from_slice(b"\r\n");
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        // ratatui hides the cursor after every frame; say so only once.
        if self.cursor_shown {
            self.buf.extend_from_slice(b"\x1b[?25l");
            self.cursor_shown = false;
        }
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        if !self.cursor_shown {
            self.buf.extend_from_slice(b"\x1b[?25h");
            self.cursor_shown = true;
        }
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        move_to(&mut self.buf, self.cursor.x, self.cursor.y)
    }

    /// Clears the screen and homes the cursor.
    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        match clear_type {
            ClearType::All => {
                self.buf.extend_from_slice(b"\x1b[2J\x1b[H");
                self.cursor = Position::ORIGIN;
            }
            ClearType::AfterCursor => self.buf.extend_from_slice(b"\x1b[J"),
            ClearType::BeforeCursor => self.buf.extend_from_slice(b"\x1b[1J"),
            ClearType::CurrentLine => self.buf.extend_from_slice(b"\x1b[2K"),
            ClearType::UntilNewLine => self.buf.extend_from_slice(b"\x1b[K"),
        }
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        // `Size` is plain data: a panic elsewhere cannot have left it half written.
        Ok(*self.size.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size()?,
            pixels: Size::new(0, 0),
        })
    }

    /// Send what the frame produced; a failed write is the caller hanging up.
    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        // The text is UTF-8, which never holds 0xFF; escape only for safety.
        let sent = if self.buf.contains(&telnet::IAC) {
            self.out.write_all(&telnet::escape(&self.buf))
        } else {
            self.out.write_all(&self.buf)
        };
        self.buf.clear();
        sent?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::testkit::Term;
    use super::*;

    fn shared(width: u16, height: u16) -> Arc<Mutex<Size>> {
        Arc::new(Mutex::new(Size::new(width, height)))
    }

    /// Run `f` with a backend wired to a local socket; returns everything the client at the other
    /// end received once the backend is dropped.
    fn received_by_client(size: &Arc<Mutex<Size>>, f: impl FnOnce(TelnetBackend)) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        f(TelnetBackend::new(server, size.clone()));
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        bytes
    }

    /// A deterministic frame of `w`x`h` cells over a few colours, one that changes with `n`.
    fn pattern(w: u16, h: u16, n: u16) -> Buffer {
        let colours = [
            Color::Reset,
            Color::Indexed(11),
            Color::Indexed(4),
            Color::Rgb(10, 200, 30),
            Color::LightCyan,
            Color::DarkGray,
        ];
        let glyphs = [' ', 'a', '#', '─', 'Ω', '▓'];
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        for y in 0..h {
            for x in 0..w {
                let k = usize::from((x * 7 + y * 13 + n * (x % 3 + 1)) % 11);
                let cell = &mut buf[(x, y)];
                cell.set_symbol(&glyphs[k % glyphs.len()].to_string());
                cell.set_fg(colours[k % colours.len()]);
                cell.set_bg(colours[(k / 2) % colours.len()]);
                if k % 5 == 0 {
                    cell.modifier |= Modifier::BOLD;
                }
                if k % 7 == 0 {
                    cell.modifier |= Modifier::REVERSED;
                }
            }
        }
        buf
    }

    /// Copy `pattern` into the terminal's frame.
    fn paint(terminal: &mut Terminal<TelnetBackend>, pattern: &Buffer) {
        terminal
            .draw(|frame| {
                let area = frame.area();
                for y in 0..area.height {
                    for x in 0..area.width {
                        frame.buffer_mut()[(x, y)].clone_from(&pattern[(x, y)]);
                    }
                }
            })
            .unwrap();
    }

    #[test]
    fn what_the_client_replays_is_the_frame_that_was_drawn() {
        let (w, h) = (30, 8);
        let bytes = received_by_client(&shared(w, h), |backend| {
            let mut terminal = Terminal::new(backend).unwrap();
            // The second frame is a diff against the first; the third changes one cell only.
            paint(&mut terminal, &pattern(w, h, 1));
            paint(&mut terminal, &pattern(w, h, 2));
            let mut last = pattern(w, h, 2);
            last[(4, 3)].set_symbol("Z").set_fg(Color::Red);
            paint(&mut terminal, &last);
        });
        let mut screen = Term::new(w, h);
        screen.feed(&bytes);
        let mut last = pattern(w, h, 2);
        last[(4, 3)].set_symbol("Z").set_fg(Color::Red);
        let wrong = screen.mismatches(&last);
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn a_frame_starts_at_the_origin_and_moves_only_across_gaps() {
        let bytes = received_by_client(&shared(5, 2), |backend| {
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|f| {
                    let buf = f.buffer_mut();
                    buf.set_string(0, 0, "ab", Style::new().fg(Color::Rgb(1, 2, 3)));
                    buf.set_string(4, 1, "c", Style::new().fg(Color::Indexed(9)));
                })
                .unwrap();
        });
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("\x1b[1;1H"), "{text:?}");
        assert!(text.contains("\x1b[38;2;1;2;3m"), "truecolor: {text:?}");
        assert!(text.contains("\x1b[38;5;9m"), "indexed: {text:?}");
        assert!(
            text.contains("ab"),
            "adjacent cells share one move: {text:?}"
        );
        assert_eq!(text.matches('H').count(), 2, "two moves in all: {text:?}");
        assert!(!text.contains('\u{ff}'));
    }

    #[test]
    fn an_idle_frame_sends_nothing_and_the_cursor_is_hidden_only_once() {
        let bytes = received_by_client(&shared(4, 2), |backend| {
            let mut terminal = Terminal::new(backend).unwrap();
            for _ in 0..5 {
                terminal
                    .draw(|f| {
                        f.buffer_mut().set_string(0, 0, "hi", Style::new());
                    })
                    .unwrap();
            }
        });
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.matches("hi").count(), 1, "{text:?}");
        assert_eq!(text.matches("\x1b[?25l").count(), 1, "{text:?}");
    }

    #[test]
    fn enter_and_leave_bracket_the_alternate_screen() {
        let bytes = received_by_client(&shared(4, 2), |mut backend| {
            backend.enter().unwrap();
            backend.leave().unwrap();
        });
        assert_eq!(
            bytes,
            b"\x1b[?1049h\x1b[?25l\x1b[0m\x1b[?25h\x1b[?1049l".to_vec()
        );
        let mut screen = Term::new(4, 2);
        screen.feed(&bytes);
        assert!(!screen.alt_screen && screen.cursor_visible);
    }

    #[test]
    fn a_new_window_size_redraws_everything_at_the_new_size() {
        let size = shared(6, 2);
        let bytes = received_by_client(&size, |backend| {
            let mut terminal = Terminal::new(backend).unwrap();
            paint(&mut terminal, &pattern(6, 2, 1));
            *size.lock().unwrap() = Size::new(9, 4);
            paint(&mut terminal, &pattern(9, 4, 1));
        });
        let mut screen = Term::new(9, 4);
        screen.feed(&bytes);
        let wrong = screen.mismatches(&pattern(9, 4, 1));
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn the_size_is_what_the_client_reported() {
        let size = shared(80, 25);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let mut backend = TelnetBackend::new(server, size.clone());
        assert_eq!(backend.size().unwrap(), Size::new(80, 25));
        *size.lock().unwrap() = Size::new(132, 43);
        assert_eq!(backend.size().unwrap(), Size::new(132, 43));
        assert_eq!(
            backend.window_size().unwrap().columns_rows,
            Size::new(132, 43)
        );
        drop(client);
    }

    #[test]
    fn a_closed_client_makes_the_frame_fail() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        drop(client);
        let mut backend = TelnetBackend::new(server, shared(4, 2));
        // The first writes after the peer closed may still be accepted by the kernel; a few
        // frames in, the reset surfaces as an error.
        let failed = (0..200).any(|_| {
            backend.buf.extend_from_slice(&[b'x'; 4096]);
            backend.flush().is_err()
        });
        assert!(failed);
    }
}
