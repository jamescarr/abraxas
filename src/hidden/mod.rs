//! The hidden line: a Unix-socket server offering the current board's zine directory and the
//! PHREAK relay's TAP minigame to a second terminal.
//!
//! The app owns a [`HiddenLine`], publishes a [`BoardSnapshot`] whenever the board changes, and
//! drains the [`HiddenEvent`]s the listener sends back (backdoors, downloads and learned
//! attacks). Connections are served one at a time on a named thread; the protocol itself lives
//! in [`session`] and never touches a socket.

pub mod board;
pub mod session;

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::game::programs::Program;
use crate::game::state::Backdoor;
use crate::pack::model::FileDef;

use board::BoardSnapshot;
use session::Session;

/// Lines longer than this are discarded until the newline.
const LINE_CAP: usize = 512;
/// How long a connection read waits before the loop rechecks the generation and shutdown.
const POLL: Duration = Duration::from_millis(250);
/// How long the accept loop sleeps between polls.
const ACCEPT_POLL: Duration = Duration::from_millis(100);
/// Sent when the socket is up but no board is published.
const NO_CARRIER: &[u8] = b"NO CARRIER\n";

/// Something a hidden-line caller did that the run must record.
#[derive(Debug, Clone, PartialEq)]
pub enum HiddenEvent {
    /// A TAP attempt changed the backdoor of `depth`.
    Backdoor { depth: u32, state: Backdoor },
    /// A file to add to the run's IRC files, from `depth`'s hidden line.
    Download { depth: u32, file: FileDef },
    /// An article taught `program` (a zine attack).
    Learn(Program),
}

/// The listener plus the state a connection reads.
pub struct HiddenLine {
    shared: Arc<Shared>,
    path: Option<PathBuf>,
}

/// What every connection shares with the app thread.
struct Shared {
    /// The current board and its generation, bumped whenever the board itself changes.
    board: Mutex<(u64, Option<Arc<BoardSnapshot>>)>,
    baud: u32,
    shutdown: AtomicBool,
    tx: Sender<HiddenEvent>,
}

impl HiddenLine {
    /// A hidden line with no socket: tests and a failed bind. `publish`/`refresh` do nothing.
    pub fn idle(tx: Sender<HiddenEvent>, baud: u32) -> Self {
        Self {
            shared: Arc::new(Shared {
                board: Mutex::new((0, None)),
                baud,
                shutdown: AtomicBool::new(false),
                tx,
            }),
            path: None,
        }
    }

    /// Listen on `path`, replacing a stale socket file. The accept loop runs on its own thread.
    pub fn listen(&mut self, path: PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new()
            .name("hidden-line".to_string())
            .spawn(move || serve(listener, shared))?;
        self.path = Some(path);
        Ok(())
    }

    /// The socket path, once listening.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// A different board: replace the snapshot and bump the generation, so a connected session
    /// reloads (and prints `CARRIER CHANGED`).
    pub fn publish(&self, board: Option<BoardSnapshot>) {
        if self.path.is_none() {
            return;
        }
        let mut guard = self.lock();
        let generation = guard.0.wrapping_add(1);
        *guard = (generation, board.map(Arc::new));
    }

    /// The same board with updated owned/learned/backdoor state: replace the snapshot without
    /// bumping the generation, so a connected session keeps its mode.
    pub fn refresh(&self, board: Option<BoardSnapshot>) {
        if self.path.is_none() {
            return;
        }
        let mut guard = self.lock();
        let generation = guard.0;
        *guard = (generation, board.map(Arc::new));
    }

    fn lock(&self) -> MutexGuard<'_, (u64, Option<Arc<BoardSnapshot>>)> {
        self.shared
            .board
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The current board generation (tests only).
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.lock().0
    }

    /// The published snapshot (tests only).
    #[cfg(test)]
    pub(crate) fn board(&self) -> Option<Arc<BoardSnapshot>> {
        self.lock().1.clone()
    }
}

impl Drop for HiddenLine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The accept loop: one client at a time, polled so shutdown is seen within [`ACCEPT_POLL`].
fn serve(listener: UnixListener, shared: Arc<Shared>) {
    while !shared.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _addr)) => handle_client(stream, &shared),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// The `(generation, board)` a new or polling connection sees.
fn current_board(shared: &Shared) -> (u64, Option<Arc<BoardSnapshot>>) {
    let guard = shared.board.lock().unwrap_or_else(PoisonError::into_inner);
    (guard.0, guard.1.clone())
}

/// Serve one caller until it hangs up, the socket fails, or the hidden line shuts down.
fn handle_client(stream: UnixStream, shared: &Arc<Shared>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(POLL));
    let mut reader = stream;
    let mut writer = match reader.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };

    let (generation, board) = current_board(shared);
    let Some(board) = board else {
        let _ = writer.write_all(NO_CARRIER);
        return;
    };
    let (mut session, motd) = Session::open(board, generation);
    if write_and_prompt(&mut writer, &motd, &session).is_err() {
        return;
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut discarding = false;
    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let mut buf = [0u8; 256];
        match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' {
                        if discarding {
                            discarding = false;
                            pending.clear();
                            if !answer(&mut writer, &session, "? line too long.\n") {
                                return;
                            }
                        } else {
                            let line = String::from_utf8_lossy(&pending).into_owned();
                            pending.clear();
                            let line = line.trim_end_matches('\r');
                            if !run_line(&mut writer, &mut session, shared, line) {
                                return;
                            }
                        }
                    } else if discarding {
                        continue;
                    } else if pending.len() >= LINE_CAP {
                        pending.clear();
                        discarding = true;
                    } else {
                        pending.push(byte);
                    }
                }
            }
            Err(e) if is_timeout(&e) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }

        let (generation, board) = current_board(shared);
        if generation != session.generation() {
            pending.clear();
            discarding = false;
            let Some(board) = board else {
                let _ = writer.write_all(NO_CARRIER);
                return;
            };
            let message = session.reload(board, generation);
            if write_and_prompt(&mut writer, &message, &session).is_err() {
                return;
            }
        }
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn write_and_prompt(writer: &mut UnixStream, text: &str, session: &Session) -> std::io::Result<()> {
    writer.write_all(text.as_bytes())?;
    writer.write_all(session.prompt().as_bytes())?;
    writer.flush()
}

/// Write a fixed answer and the prompt.
fn answer(writer: &mut UnixStream, session: &Session, text: &str) -> bool {
    writer.write_all(text.as_bytes()).is_ok()
        && writer.write_all(session.prompt().as_bytes()).is_ok()
        && writer.flush().is_ok()
}

/// Handle one complete line: send its events, write the reply, stream any ANSI. `false` ends the
/// connection.
fn run_line(writer: &mut UnixStream, session: &mut Session, shared: &Shared, line: &str) -> bool {
    let reply = session.line(line);
    for event in reply.events {
        let _ = shared.tx.send(event);
    }
    if writer.write_all(reply.text.as_bytes()).is_err() {
        return false;
    }
    if let Some(text) = reply.stream
        && stream_at_baud(writer, &text, shared.baud, &shared.shutdown).is_err()
    {
        return false;
    }
    if reply.close {
        return false;
    }
    writer.write_all(session.prompt().as_bytes()).is_ok() && writer.flush().is_ok()
}

/// What a non-blocking read on the client saw while art was streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// The caller closed its sending side, as a script does with `printf 'TYPE X\n' | nc -U`.
    /// That is no input and no hang-up (a vanished caller fails the next write).
    Gone,
    /// The caller pressed a key (any line skips).
    Input,
    /// Nothing yet.
    Idle,
}

/// Stream terminal text at `baud` B (0 = instant). Any client input skips to the end; a caller
/// that closed its sending side still gets the whole piece, and one that vanished ends it with
/// a failed write. Always closes with a reset.
fn stream_at_baud(
    stream: &mut UnixStream,
    text: &str,
    baud: u32,
    shutdown: &AtomicBool,
) -> std::io::Result<()> {
    if baud == 0 {
        stream.write_all(text.as_bytes())?;
        stream.write_all(b"\x1b[0m\n")?;
        stream.flush()?;
        return Ok(());
    }
    let chars: Vec<char> = text.chars().collect();
    let cps = u64::from((baud / 10).max(1));
    let chunk = usize::try_from((cps / 20).max(1)).unwrap_or(1);
    let mut i = 0;
    let mut listening = true;
    while i < chars.len() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let end = (i + chunk).min(chars.len());
        let piece: String = chars[i..end].iter().copied().collect();
        stream.write_all(piece.as_bytes())?;
        stream.flush()?;
        i = end;
        if i == chars.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        if listening {
            match probe(stream)? {
                Probe::Gone => listening = false,
                Probe::Input => {
                    stream.write_all(b"\x1b[0m\n*** skipped.\n")?;
                    stream.flush()?;
                    return Ok(());
                }
                Probe::Idle => {}
            }
        }
    }
    stream.write_all(b"\x1b[0m\n")?;
    stream.flush()?;
    Ok(())
}

/// One non-blocking read on the client, restoring blocking mode afterwards.
fn probe(stream: &mut UnixStream) -> std::io::Result<Probe> {
    stream.set_nonblocking(true)?;
    let mut buf = [0u8; 64];
    let result = stream.read(&mut buf);
    stream.set_nonblocking(false)?;
    match result {
        Ok(0) => Ok(Probe::Gone),
        Ok(_) => Ok(Probe::Input),
        Err(e) if is_timeout(&e) => Ok(Probe::Idle),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(Probe::Idle),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn publish_bumps_the_generation_and_refresh_does_not() {
        let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let (tx, _rx) = channel();
        let mut line = HiddenLine::idle(tx, 0);

        // Idle: no socket, so publishing is a no-op.
        line.publish(None);
        assert_eq!(line.generation(), 0);

        line.listen(path.clone()).unwrap();
        assert_eq!(line.path(), Some(path.as_path()));
        line.publish(None);
        assert_eq!(line.generation(), 1);
        line.refresh(None);
        assert_eq!(line.generation(), 1);

        drop(line);
        assert!(!path.exists());
    }
}
