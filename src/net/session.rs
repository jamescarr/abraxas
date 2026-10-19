//! One telnet caller: negotiation, the app loop, the hang-up.
//!
//! The loop is `ui::app`'s `event_loop` with the socket as the event source: draw a frame, collect
//! what the caller typed until the frame is due, step the app. Everything runs on the session
//! thread: the app holds `Rc`s and never leaves it.

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::crossterm::event::{Event, KeyEvent};
use ratatui::layout::Size;

use super::TelnetBackend;
use super::keys::KeyDecoder;
use super::telnet::{self, TelnetItem, TelnetParser};
use crate::palette::Palette;
use crate::ui::app::{App, Ctx};
use crate::ui::screens::{LoginScreen, Screen};

/// Milliseconds between frames (the pace of the local loop); calm mode slows it to 100.
pub const FRAME_MS: u64 = 33;
const CALM_FRAME_MS: u64 = 100;
/// Seconds without a byte from the client before the line is dropped.
pub const IDLE_SECS: u64 = 15 * 60;
/// Seconds a write may block before the line is dropped: a client that stops reading (a frozen
/// terminal, a suspended `telnet`) fills the socket buffers, and the idle limit cannot fire
/// while the session is stuck writing.
const WRITE_SECS: u64 = 30;
/// How long the farewell (leaving the alternate screen, NO CARRIER) may block on a caller who is
/// no longer reading.
const FAREWELL_TIMEOUT: Duration = Duration::from_millis(500);

/// What the caller reads as the line drops.
const NO_CARRIER: &[u8] = b"\r\nNO CARRIER\r\n";

/// The window used until the client reports one.
const DEFAULT_SIZE: Size = Size::new(80, 25);
/// The largest window taken from a client: the report is the client's word, and the screen
/// buffers grow with it.
const MAX_SIZE: Size = Size::new(512, 256);

/// How the loop ended.
enum Exit {
    /// The client closed the connection.
    Closed,
    /// We hang up: the game ended, the caller left the dialer's door or the line went idle.
    NoCarrier,
}

/// How long a caller may hold the line without playing.
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// No byte from the client for this long: NO CARRIER.
    idle: Duration,
    /// A write blocked for this long: the client has stopped reading.
    write: Duration,
}

impl Limits {
    const LINE: Limits = Limits {
        idle: Duration::from_secs(IDLE_SECS),
        write: Duration::from_secs(WRITE_SECS),
    };
}

/// Run the whole game for one connection; returns when the caller hangs up, the game quits, or
/// the socket fails. `make_ctx` builds the `Ctx` (own director channel, own rng) on this thread.
///
/// The connection stays the caller's: it is closed when they drop it, so the server can free the
/// line first and a caller who redials the moment they see the hang-up is not told it is busy.
pub fn serve_one(stream: &TcpStream, make_ctx: &dyn Fn() -> Ctx) -> io::Result<()> {
    serve(stream, make_ctx, Limits::LINE)
}

fn serve(stream: &TcpStream, make_ctx: &dyn Fn() -> Ctx, limits: Limits) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(limits.write))?;
    say(stream, &telnet::HELLO)?;
    let size = Arc::new(Mutex::new(DEFAULT_SIZE));

    let mut ctx = make_ctx();
    // The socket has no COLORTERM: callers get the 16 colours every telnet client renders. That
    // also keeps the CRT pass (truecolor only) off; it would redraw the whole screen every frame.
    ctx.palette = Palette::new(false);
    // The board opens at the login: no boot screen, and the dialer is telnet itself.
    let login = Screen::Login(LoginScreen::new(&mut ctx));
    let mut app = App::with_screen(ctx, login);

    let mut backend = TelnetBackend::new(stream.try_clone()?, size.clone());
    backend.enter()?;
    let mut terminal = Terminal::new(backend)?;

    let exit = event_loop(stream, &mut terminal, &mut app, &size, limits.idle).map_err(stalled);

    // Every path gives the client its screen back; the socket may be dead or full, so the
    // farewell gets a moment and its errors are moot.
    let _ = stream.set_write_timeout(Some(FAREWELL_TIMEOUT));
    let _ = terminal.backend_mut().leave();
    app.ctx.director.cancel_all();
    if let Exit::NoCarrier = exit? {
        let _ = say(stream, NO_CARRIER);
    }
    Ok(())
}

/// Reads never fail with a timeout (their timeout is the frame clock), so one that gets this far
/// is a write that gave up: the caller stopped reading.
fn stalled(e: io::Error) -> io::Error {
    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
        io::Error::new(ErrorKind::TimedOut, "the caller stopped reading")
    } else {
        e
    }
}

/// Write `bytes` to the caller.
fn say(mut stream: &TcpStream, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(bytes)
}

fn event_loop(
    stream: &TcpStream,
    terminal: &mut Terminal<TelnetBackend>,
    app: &mut App,
    size: &Mutex<Size>,
    idle: Duration,
) -> io::Result<Exit> {
    let frame_time = Duration::from_millis(if app.ctx.calm() {
        CALM_FRAME_MS
    } else {
        FRAME_MS
    });
    let mut line = Line {
        stream,
        size,
        parser: TelnetParser::default(),
        decoder: KeyDecoder::default(),
    };
    let mut last = Instant::now();
    let mut heard = last;
    loop {
        terminal.draw(|f| app.render(f))?;
        let Some(keys) = line.keys_until(last + frame_time, &mut heard)? else {
            return Ok(Exit::Closed);
        };
        let now = Instant::now();
        let dt = now - last;
        last = now;
        for key in keys {
            app.step(Some(Event::Key(key)), Duration::ZERO);
            if hung_up(app) {
                return Ok(Exit::NoCarrier);
            }
        }
        app.step(None, dt);
        if hung_up(app) || now - heard >= idle {
            return Ok(Exit::NoCarrier);
        }
    }
}

/// The game is over (the caller answered the `Hang up?` prompt) or the caller has been put at
/// the dialer. The dialer is how a local player picks a board; on the line it must never be
/// shown, so it ends the call instead. By then every screen that leads there has saved the run
/// and the caller record, as it does locally.
fn hung_up(app: &App) -> bool {
    app.quit || matches!(app.screen, Screen::Dialer(_))
}

/// The caller's end of the line: the socket, and what has been read from it and not yet turned
/// into keys.
struct Line<'a> {
    stream: &'a TcpStream,
    size: &'a Mutex<Size>,
    parser: TelnetParser,
    decoder: KeyDecoder,
}

impl Line<'_> {
    /// Read until `deadline` and return the keys the caller pressed meanwhile, or `None` when
    /// the client closed the connection. `heard` is set to the time of the last byte received.
    fn keys_until(
        &mut self,
        deadline: Instant,
        heard: &mut Instant,
    ) -> io::Result<Option<Vec<KeyEvent>>> {
        let mut keys = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            self.stream.set_read_timeout(Some(deadline - now))?;
            match self.stream.read(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(n) => {
                    *heard = Instant::now();
                    keys.extend(self.hear(&buf[..n])?);
                }
                // Nothing arrived before the timeout.
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        keys.extend(self.decoder.flush());
        Ok(Some(keys))
    }

    /// Take one read's bytes through the telnet layer: answers go back to the client, window
    /// sizes to the backend, the rest are keys.
    fn hear(&mut self, bytes: &[u8]) -> io::Result<Vec<KeyEvent>> {
        let mut reply = Vec::new();
        let mut typed = Vec::new();
        for item in self.parser.feed(bytes, &mut reply) {
            match item {
                TelnetItem::Data(b) => typed.push(b),
                TelnetItem::Naws { cols, rows } => {
                    *self.size.lock().unwrap_or_else(PoisonError::into_inner) =
                        Size::new(cols.min(MAX_SIZE.width), rows.min(MAX_SIZE.height));
                }
            }
        }
        if !reply.is_empty() {
            self.stream.write_all(&reply)?;
        }
        Ok(self.decoder.feed(&typed))
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread::JoinHandle;

    use super::*;
    use crate::config::{Paths, TruecolorMode};
    use crate::game::save::SaveStore;
    use crate::net::telnet::{DO, IAC, OPT_NAWS, SB, SE, WONT};
    use crate::net::testkit::{Caller, PATIENCE};
    use crate::ui::app::test_ctx;

    /// The board's side of the call: `serve` running on its own thread.
    struct Board(JoinHandle<io::Result<()>>);

    impl Board {
        fn is_finished(&self) -> bool {
            self.0.is_finished()
        }

        fn finish(self) -> io::Result<()> {
            self.0.join().expect("the session thread panicked")
        }
    }

    /// Connect a caller whose terminal is `window` big; `make_ctx` builds the game's context.
    fn dial_with(
        make_ctx: impl Fn() -> Ctx + Send + 'static,
        limits: Limits,
        window: Size,
    ) -> (Caller, Board) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (line, _) = listener.accept().unwrap();
        let board = Board(std::thread::spawn(move || serve(&line, &make_ctx, limits)));
        (Caller::new(stream, window), board)
    }

    fn dial(dir: &Path) -> (Caller, Board) {
        let dir = dir.to_path_buf();
        dial_with(move || test_ctx(&dir), Limits::LINE, DEFAULT_SIZE)
    }

    /// Register `Tester` and wait for the level card.
    fn register(caller: &mut Caller) {
        caller.wait_for("Handle:");
        caller.send(b"Tester\r");
        caller.wait_for("Register as Tester? [Y/n] _");
        caller.send(b"y");
        caller.wait_for("Real name (it will be used against you):");
        caller.send(b"Ada\r");
        caller.wait_for("Where did you hear about ABRAXAS?");
        caller.send(b"\r");
        caller.wait_for("press any key");
    }

    /// A NAWS report: `IAC SB NAWS cols rows IAC SE`, with 0xFF bytes doubled.
    fn naws(cols: u16, rows: u16) -> Vec<u8> {
        let mut bytes = vec![IAC, SB, OPT_NAWS];
        for byte in cols.to_be_bytes().into_iter().chain(rows.to_be_bytes()) {
            bytes.push(byte);
            if byte == IAC {
                bytes.push(IAC);
            }
        }
        bytes.extend([IAC, SE]);
        bytes
    }

    fn store(dir: &Path) -> SaveStore {
        SaveStore::new(&Paths::in_dir(dir.to_path_buf()))
    }

    #[test]
    fn a_caller_is_negotiated_with_and_lands_on_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        caller.wait_for("Handle:");
        assert!(caller.raw.starts_with(&telnet::HELLO), "{:?}", caller.raw);
        assert!(
            caller.raw[telnet::HELLO.len()..].starts_with(b"\x1b[?1049h"),
            "the alternate screen follows the negotiation"
        );
        let screen = caller.screen.text();
        assert!(screen.contains("LAST CALLERS"), "{screen}");
        assert!(!screen.contains("TERMINUS"), "no boot screen:\n{screen}");
        drop(caller);
        assert!(board.finish().is_ok());
    }

    #[test]
    fn options_the_board_did_not_ask_for_are_declined_on_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        caller.wait_for("Handle:");
        // The client offers TERMINAL-TYPE (24); the board is not interested.
        caller.send(&[IAC, DO, 24]);
        let declined = [IAC, WONT, 24];
        let deadline = Instant::now() + PATIENCE;
        while !caller.raw.windows(3).any(|w| w == declined) && Instant::now() < deadline {
            caller.pump(Duration::from_millis(20));
        }
        assert!(caller.raw.windows(3).any(|w| w == declined));
        drop(caller);
        assert!(board.finish().is_ok());
    }

    #[test]
    fn registering_over_the_socket_reaches_the_level_card() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        caller.wait_for("Handle:");
        // A real client's Enter is CR LF or CR NUL, and it is one key, not two: a second Enter
        // would answer the confirmation before the caller saw it.
        caller.send(b"Tester\r\n");
        caller.wait_for("Register as Tester? [Y/n] _");
        caller.pump(Duration::from_millis(200));
        let screen = caller.screen.text();
        assert!(screen.contains("Register as Tester? [Y/n] _"), "{screen}");
        caller.send(b"y");
        caller.wait_for("Real name (it will be used against you):");
        caller.send(b"Ada\r\0");
        caller.wait_for("Where did you hear about ABRAXAS?");
        caller.send(b"\r");
        caller.wait_for("press any key");

        let saved = store(dir.path()).load("Tester").unwrap().expect("saved");
        let title = saved.levels[&0]
            .place
            .as_ref()
            .expect("a place")
            .title
            .clone();
        let screen = caller.screen.text();
        assert!(screen.contains("LEVEL 00"), "{screen}");
        assert!(screen.contains(&title), "{title:?} missing:\n{screen}");
        drop(caller);
        assert!(board.finish().is_ok());
    }

    #[test]
    fn esc_at_the_handle_prompt_hangs_up_and_the_dialer_is_never_shown() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        caller.wait_for("Handle:");
        caller.send(b"\x1b");
        assert!(caller.hung_up(), "{}", caller.screen.text());
        let main = caller.screen.main_text();
        assert!(main.contains("NO CARRIER"), "{main}");
        assert!(!caller.screen.alt_screen && caller.screen.cursor_visible);
        let last_frame = caller.screen.alt_text();
        assert!(last_frame.contains("Handle:"), "{last_frame}");
        assert!(!last_frame.contains("NIGHT SHIFT"), "{last_frame}");
        assert!(board.finish().is_ok());
    }

    #[test]
    fn every_quit_chord_asks_to_hang_up_and_n_keeps_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        caller.wait_for("Handle:");
        for chord in [&b"\x03"[..], b"\x11", b"\x1bx"] {
            caller.send(chord);
            caller.wait_for("Hang up? [Y/n]");
            caller.send(b"n");
            caller.wait_until("the prompt to close", |t| !t.text().contains("Hang up?"));
        }
        drop(caller);
        assert!(board.finish().is_ok());
    }

    #[test]
    fn ctrl_c_then_y_saves_the_run_and_hangs_up() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        register(&mut caller);
        // Only the hang-up can bring the save back.
        let save = dir.path().join("saves").join("tester.json");
        std::fs::remove_file(&save).unwrap();
        caller.send(b"\x03");
        caller.wait_for("Hang up? [Y/n]");
        caller.send(b"y");
        assert!(caller.hung_up(), "{}", caller.screen.text());
        let main = caller.screen.main_text();
        assert!(main.contains("NO CARRIER"), "{main}");
        assert!(store(dir.path()).exists("Tester"));
        assert!(board.finish().is_ok());
    }

    #[test]
    fn a_client_that_closes_mid_game_ends_the_call_and_keeps_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial(dir.path());
        register(&mut caller);
        drop(caller);
        assert!(board.finish().is_ok());
        let saved = store(dir.path()).load("Tester").unwrap().expect("saved");
        assert_eq!(saved.handle, "Tester");
    }

    #[test]
    fn a_caller_who_goes_silent_is_dropped_and_one_who_types_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let idle = Duration::from_millis(800);
        let limits = Limits {
            idle,
            ..Limits::LINE
        };
        let (mut caller, board) = dial_with(move || test_ctx(&path), limits, DEFAULT_SIZE);
        caller.wait_for("Handle:");
        // Typing for longer than the idle limit keeps the call: each byte restarts the clock.
        // Backspace on an empty prompt changes nothing, but it is a byte from the caller.
        let started = Instant::now();
        while started.elapsed() < idle + idle / 2 {
            caller.send(b"\x7f");
            assert!(
                caller.pump(Duration::from_millis(150)),
                "dropped while typing"
            );
        }
        assert!(caller.hung_up());
        let main = caller.screen.main_text();
        assert!(main.contains("NO CARRIER"), "{main}");
        assert!(board.finish().is_ok());
    }

    #[test]
    fn a_caller_who_stops_reading_is_dropped_once_the_socket_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let limits = Limits {
            write: Duration::from_millis(300),
            ..Limits::LINE
        };
        let (mut caller, board) = dial_with(move || test_ctx(&path), limits, DEFAULT_SIZE);
        caller.wait_for("Handle:");

        // From here the caller reads nothing. Every resize makes the board redraw the whole
        // window, far more than the socket buffers hold; a write that cannot finish must end the
        // call, since the idle limit cannot fire while the session is stuck writing.
        let started = Instant::now();
        let mut width = MAX_SIZE.width;
        while !board.is_finished() && started.elapsed() < Duration::from_secs(20) {
            caller.send(&naws(width, MAX_SIZE.height));
            width = if width == MAX_SIZE.width {
                width - 1
            } else {
                MAX_SIZE.width
            };
            std::thread::sleep(Duration::from_millis(40));
        }
        // Joining a session stuck in a write would hang the test, so look first.
        assert!(
            board.is_finished(),
            "the board is stuck writing to a caller who never reads"
        );
        let err = board.finish().expect_err("the call ends with an error");
        assert_eq!(err.kind(), ErrorKind::TimedOut, "{err}");
        assert_eq!(err.to_string(), "the caller stopped reading");
    }

    #[test]
    fn the_board_follows_the_window_the_client_reports() {
        let dir = tempfile::tempdir().unwrap();
        let (mut caller, board) = dial_with(
            {
                let dir = dir.path().to_path_buf();
                move || test_ctx(&dir)
            },
            Limits::LINE,
            MAX_SIZE,
        );
        caller.wait_for("Handle:");
        let home = caller.screen.find("Handle:").unwrap();

        // A bigger window: the 80x25 canvas moves to its middle.
        caller.send(&naws(100, 40));
        caller.wait_until("the board centred in 100x40", |t| {
            t.find("Handle:") == Some((home.0 + 10, home.1 + 7))
        });

        // A smaller one gets the too-small message, and the board comes back with a big enough one.
        caller.send(&naws(60, 20));
        caller.wait_for("This one is 60x20.");
        caller.send(&naws(80, 25));
        caller.wait_until("the board at 80x25 again", |t| {
            t.find("Handle:") == Some(home)
        });

        // A window beyond what is allowed is cut to the limit.
        caller.send(&naws(u16::MAX, u16::MAX));
        let (dx, dy) = (
            usize::from((MAX_SIZE.width - 80) / 2),
            usize::from((MAX_SIZE.height - 25) / 2),
        );
        caller.wait_until("the board centred in the largest window", |t| {
            t.find("Handle:") == Some((home.0 + dx, home.1 + dy))
        });
        // A window of 0x0 (a client that does not know its size yet) changes nothing.
        caller.send(&naws(0, 0));
        caller.pump(Duration::from_millis(200));
        assert_eq!(
            caller.screen.find("Handle:"),
            Some((home.0 + dx, home.1 + dy))
        );
        drop(caller);
        assert!(board.finish().is_ok());
    }

    #[test]
    fn callers_get_sixteen_colours_whatever_the_server_would_use_locally() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (mut caller, board) = dial_with(
            move || {
                let mut ctx = test_ctx(&path);
                ctx.config.display.truecolor = TruecolorMode::Always;
                ctx.config.display.crt = true;
                ctx.palette = Palette::new(true);
                ctx
            },
            Limits::LINE,
            DEFAULT_SIZE,
        );
        caller.wait_for("Handle:");
        let sent = String::from_utf8_lossy(&caller.raw).into_owned();
        assert!(sent.contains("\x1b[38;5;"), "indexed colours are used");
        assert!(
            !sent.contains("\x1b[38;2;") && !sent.contains("\x1b[48;2;"),
            "no truecolor"
        );
        drop(caller);
        assert!(board.finish().is_ok());
    }
}
