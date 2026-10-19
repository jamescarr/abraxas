//! Decodes the byte stream of a telnet client into the key events the app understands.
//!
//! crossterm's own escape-sequence parser is private, so this is the small subset a terminal
//! sends for the keys the game uses: arrows, Home/End, paging, Insert/Delete, F1-F12 (also with
//! Shift/Alt/Ctrl), Backspace, Tab, Enter, Ctrl+letter and Alt+key, plus UTF-8 text. Events have
//! the shape crossterm delivers locally, so `ui::keys::classify` treats both alike.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How long an unfinished escape sequence waits for its rest before it is given up: a lone `ESC`
/// then is the Esc key.
pub const ESC_WAIT: Duration = Duration::from_millis(50);

const ESC: u8 = 0x1b;
/// The longest CSI sequence accepted (`ESC [ 1 ; 5 A` is 6 bytes); a longer one is junk and is
/// dropped rather than buffered for ever.
const CSI_MAX: usize = 32;

/// Turns client data bytes into key presses. Sequences may arrive split across reads.
#[derive(Debug, Default)]
pub struct KeyDecoder {
    /// Bytes of an unfinished sequence.
    pending: Vec<u8>,
    /// When the unfinished sequence started.
    since: Option<Instant>,
}

impl KeyDecoder {
    /// Feed decoded data bytes; returns complete key events. An unfinished escape sequence is
    /// kept until the next call or [`flush`](Self::flush).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<KeyEvent> {
        self.feed_at(bytes, Instant::now())
    }

    /// Give up on an unfinished sequence that has waited longer than [`ESC_WAIT`]: its `ESC` is
    /// the Esc key and the bytes after it are typed text.
    pub fn flush(&mut self) -> Vec<KeyEvent> {
        self.flush_at(Instant::now())
    }

    fn feed_at(&mut self, bytes: &[u8], now: Instant) -> Vec<KeyEvent> {
        self.pending.extend_from_slice(bytes);
        self.drain(false, now)
    }

    fn flush_at(&mut self, now: Instant) -> Vec<KeyEvent> {
        match self.since {
            Some(since) if now.saturating_duration_since(since) >= ESC_WAIT => {
                self.drain(true, now)
            }
            _ => Vec::new(),
        }
    }

    /// Decode `pending`. With `last` nothing more is coming, so an unfinished sequence is
    /// resolved instead of waited for.
    fn drain(&mut self, last: bool, now: Instant) -> Vec<KeyEvent> {
        let mut keys = Vec::new();
        let mut used = 0;
        while used < self.pending.len() {
            match parse(&self.pending[used..], last) {
                Step::Key(len, key) => {
                    keys.push(key);
                    used += len;
                }
                Step::Skip(len) => used += len,
                Step::More => break,
            }
        }
        self.pending.drain(..used);
        if self.pending.is_empty() {
            self.since = None;
        } else if used > 0 || self.since.is_none() {
            // A new unfinished sequence starts here.
            self.since = Some(now);
        }
        keys
    }
}

/// The result of reading one key from the front of the buffer.
enum Step {
    /// `len` bytes made this key.
    Key(usize, KeyEvent),
    /// `len` bytes mean nothing (an unknown sequence, a bad byte).
    Skip(usize),
    /// The buffer ends inside a sequence.
    More,
}

fn key(len: usize, code: KeyCode, modifiers: KeyModifiers) -> Step {
    Step::Key(len, KeyEvent::new(code, modifiers))
}

fn parse(buf: &[u8], last: bool) -> Step {
    let Some(&b) = buf.first() else {
        return Step::More;
    };
    match b {
        ESC => parse_escape(buf, last),
        b'\r' | b'\n' => key(1, KeyCode::Enter, KeyModifiers::NONE),
        b'\t' => key(1, KeyCode::Tab, KeyModifiers::NONE),
        0x7f | 0x08 => key(1, KeyCode::Backspace, KeyModifiers::NONE),
        0x01..=0x1a => key(
            1,
            KeyCode::Char(char::from(b + 0x60)),
            KeyModifiers::CONTROL,
        ),
        0x20..=0x7e => key(1, KeyCode::Char(char::from(b)), KeyModifiers::NONE),
        0x80.. => parse_utf8(buf, last),
        // NUL (Ctrl+Space) and Ctrl+\ ] ^ _ are not game keys.
        _ => Step::Skip(1),
    }
}

fn parse_escape(buf: &[u8], last: bool) -> Step {
    let Some(&second) = buf.get(1) else {
        // A lone ESC is the Esc key once nothing more can follow it.
        return if last {
            key(1, KeyCode::Esc, KeyModifiers::NONE)
        } else {
            Step::More
        };
    };
    match second {
        b'[' => parse_csi(buf, last),
        b'O' => parse_ss3(buf, last),
        // Alt+key arrives as ESC and the key.
        0x20..=0x7e => key(2, KeyCode::Char(char::from(second)), KeyModifiers::ALT),
        // ESC and then something that starts no sequence: the Esc key, then that byte alone.
        _ => key(1, KeyCode::Esc, KeyModifiers::NONE),
    }
}

/// `ESC O x`: application-mode arrows, Home/End and F1-F4.
fn parse_ss3(buf: &[u8], last: bool) -> Step {
    let Some(&fin) = buf.get(2) else {
        return unfinished(last);
    };
    match nav_or_function(fin) {
        Some(code) => key(3, code, KeyModifiers::NONE),
        None => Step::Skip(3),
    }
}

/// `ESC [ params final`.
fn parse_csi(buf: &[u8], last: bool) -> Step {
    let mut end = 2;
    loop {
        match buf.get(end) {
            // Parameter and intermediate bytes.
            Some(0x20..=0x3f) if end < CSI_MAX => end += 1,
            Some(&fin @ 0x40..=0x7e) => return csi_key(&buf[2..end], fin, end + 1),
            // A byte that cannot be part of a sequence (or one that is too long): drop what came
            // before it and read that byte as typed.
            Some(_) => return Step::Skip(end),
            None => return unfinished(last),
        }
    }
}

/// The key of a finished CSI sequence, or a skip of its `len` bytes when it is not one we know.
fn csi_key(params: &[u8], fin: u8, len: usize) -> Step {
    let Some([first, second]) = numbers(params) else {
        return Step::Skip(len);
    };
    let mut modifiers = modifier_param(second);
    let code = match fin {
        b'Z' => {
            modifiers |= KeyModifiers::SHIFT;
            Some(KeyCode::BackTab)
        }
        b'~' => tilde_key(first),
        _ => nav_or_function(fin),
    };
    match code {
        Some(code) => key(len, code, modifiers),
        None => Step::Skip(len),
    }
}

/// What a sequence that ends the buffer means: wait, or (with nothing more coming) the Esc key
/// followed by the bytes typed after it.
fn unfinished(last: bool) -> Step {
    if last {
        key(1, KeyCode::Esc, KeyModifiers::NONE)
    } else {
        Step::More
    }
}

/// The first two `;`-separated numbers of CSI parameter bytes (0 when absent), or `None` when
/// the parameters hold anything but digits and `;` (private sequences such as `ESC [ ? 1 ; 2 c`).
fn numbers(params: &[u8]) -> Option<[u16; 2]> {
    let mut out = [0u16; 2];
    let mut at = 0;
    for &b in params {
        match b {
            b'0'..=b'9' => {
                if let Some(n) = out.get_mut(at) {
                    *n = n.saturating_mul(10).saturating_add(u16::from(b - b'0'));
                }
            }
            b';' => at += 1,
            _ => return None,
        }
    }
    Some(out)
}

/// xterm's modifier parameter: 1 + (Shift 1, Alt 2, Ctrl 4).
fn modifier_param(param: u16) -> KeyModifiers {
    let bits = param.saturating_sub(1);
    let mut mods = KeyModifiers::NONE;
    if bits & 1 != 0 {
        mods |= KeyModifiers::SHIFT;
    }
    if bits & 2 != 0 {
        mods |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        mods |= KeyModifiers::CONTROL;
    }
    mods
}

/// The keys that end an SS3 or CSI sequence with a letter.
fn nav_or_function(fin: u8) -> Option<KeyCode> {
    Some(match fin {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        b'P'..=b'S' => KeyCode::F(1 + fin - b'P'),
        _ => return None,
    })
}

/// The keys of `ESC [ n ~`.
fn tilde_key(n: u16) -> Option<KeyCode> {
    let function = |base: u16| u8::try_from(n - base).ok().map(KeyCode::F);
    match n {
        1 | 7 => Some(KeyCode::Home),
        2 => Some(KeyCode::Insert),
        3 => Some(KeyCode::Delete),
        4 | 8 => Some(KeyCode::End),
        5 => Some(KeyCode::PageUp),
        6 => Some(KeyCode::PageDown),
        11..=15 => function(10),
        17..=21 => function(11),
        23 | 24 => function(12),
        _ => None,
    }
}

/// One UTF-8 encoded character. Bytes that cannot start or continue one are dropped.
fn parse_utf8(buf: &[u8], last: bool) -> Step {
    let need = match buf[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Step::Skip(1),
    };
    if buf.len() < need {
        let continues = buf[1..].iter().all(|b| b & 0xc0 == 0x80);
        return if continues && !last {
            Step::More
        } else {
            Step::Skip(1)
        };
    }
    match std::str::from_utf8(&buf[..need])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(c) => key(need, KeyCode::Char(c), KeyModifiers::NONE),
        None => Step::Skip(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    fn ev(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn decode(bytes: &[u8]) -> Vec<KeyEvent> {
        KeyDecoder::default().feed(bytes)
    }

    #[test]
    fn plain_keys() {
        let rows: &[(&[u8], KeyCode, KeyModifiers)] = &[
            (b"a", KeyCode::Char('a'), NONE),
            (b"Z", KeyCode::Char('Z'), NONE),
            (b" ", KeyCode::Char(' '), NONE),
            (b"\r", KeyCode::Enter, NONE),
            (b"\n", KeyCode::Enter, NONE),
            (b"\t", KeyCode::Tab, NONE),
            (b"\x7f", KeyCode::Backspace, NONE),
            (b"\x08", KeyCode::Backspace, NONE),
            (b"\x1b[Z", KeyCode::BackTab, SHIFT),
        ];
        for &(bytes, code, mods) in rows {
            assert_eq!(decode(bytes), vec![ev(code, mods)], "{bytes:?}");
        }
    }

    #[test]
    fn arrows_home_end_and_paging_in_both_cursor_modes() {
        let rows: &[(&[u8], KeyCode)] = &[
            (b"\x1b[A", KeyCode::Up),
            (b"\x1b[B", KeyCode::Down),
            (b"\x1b[C", KeyCode::Right),
            (b"\x1b[D", KeyCode::Left),
            (b"\x1bOA", KeyCode::Up),
            (b"\x1bOB", KeyCode::Down),
            (b"\x1bOC", KeyCode::Right),
            (b"\x1bOD", KeyCode::Left),
            (b"\x1b[H", KeyCode::Home),
            (b"\x1b[1~", KeyCode::Home),
            (b"\x1b[7~", KeyCode::Home),
            (b"\x1bOH", KeyCode::Home),
            (b"\x1b[F", KeyCode::End),
            (b"\x1b[4~", KeyCode::End),
            (b"\x1b[8~", KeyCode::End),
            (b"\x1bOF", KeyCode::End),
            (b"\x1b[2~", KeyCode::Insert),
            (b"\x1b[3~", KeyCode::Delete),
            (b"\x1b[5~", KeyCode::PageUp),
            (b"\x1b[6~", KeyCode::PageDown),
        ];
        for &(bytes, code) in rows {
            assert_eq!(decode(bytes), vec![ev(code, NONE)], "{bytes:?}");
        }
        // Several keys in one read come out in order.
        assert_eq!(
            decode(b"\x1b[5~\x1b[6~\x1bOA"),
            vec![
                ev(KeyCode::PageUp, NONE),
                ev(KeyCode::PageDown, NONE),
                ev(KeyCode::Up, NONE)
            ]
        );
    }

    #[test]
    fn function_keys() {
        let ss3: &[(&[u8], u8)] = &[
            (b"\x1bOP", 1),
            (b"\x1bOQ", 2),
            (b"\x1bOR", 3),
            (b"\x1bOS", 4),
        ];
        for &(bytes, n) in ss3 {
            assert_eq!(decode(bytes), vec![ev(KeyCode::F(n), NONE)], "{bytes:?}");
        }
        // The tilde codes skip 16 and 22.
        let tilde = [
            (11, 1),
            (12, 2),
            (13, 3),
            (14, 4),
            (15, 5),
            (17, 6),
            (18, 7),
            (19, 8),
            (20, 9),
            (21, 10),
            (23, 11),
            (24, 12),
        ];
        for (code, n) in tilde {
            let bytes = format!("\x1b[{code}~");
            assert_eq!(
                decode(bytes.as_bytes()),
                vec![ev(KeyCode::F(n), NONE)],
                "{bytes:?}"
            );
        }
        assert!(decode(b"\x1b[16~").is_empty(), "16 is no key");
        assert!(decode(b"\x1b[22~").is_empty(), "22 is no key");
    }

    #[test]
    fn modifier_parameters() {
        let rows: &[(&[u8], KeyCode, KeyModifiers)] = &[
            (b"\x1b[1;2A", KeyCode::Up, SHIFT),
            (b"\x1b[1;3B", KeyCode::Down, ALT),
            (b"\x1b[1;5C", KeyCode::Right, CTRL),
            (b"\x1b[1;6D", KeyCode::Left, CTRL | SHIFT),
            (b"\x1b[1;5H", KeyCode::Home, CTRL),
            (b"\x1b[1;2F", KeyCode::End, SHIFT),
            (b"\x1b[1;5P", KeyCode::F(1), CTRL),
            (b"\x1b[15;2~", KeyCode::F(5), SHIFT),
            (b"\x1b[3;5~", KeyCode::Delete, CTRL),
        ];
        for &(bytes, code, mods) in rows {
            assert_eq!(decode(bytes), vec![ev(code, mods)], "{bytes:?}");
        }
    }

    #[test]
    fn alt_and_control_chords() {
        // Alt+x is how the quit chord arrives; Ctrl+C and Ctrl+Q are the other two.
        assert_eq!(decode(b"\x1bx"), vec![ev(KeyCode::Char('x'), ALT)]);
        assert_eq!(decode(b"\x1bX"), vec![ev(KeyCode::Char('X'), ALT)]);
        assert_eq!(decode(b"\x1b1"), vec![ev(KeyCode::Char('1'), ALT)]);
        assert_eq!(decode(b"\x03"), vec![ev(KeyCode::Char('c'), CTRL)]);
        assert_eq!(decode(b"\x11"), vec![ev(KeyCode::Char('q'), CTRL)]);
        assert_eq!(decode(b"\x0c"), vec![ev(KeyCode::Char('l'), CTRL)]);
        assert_eq!(decode(b"\x1a"), vec![ev(KeyCode::Char('z'), CTRL)]);
        for chord in [&b"\x03"[..], b"\x11", b"\x1bx"] {
            assert!(
                crate::ui::keys::is_quit_chord(&decode(chord)[0]),
                "{chord:?}"
            );
        }
        assert!(!crate::ui::keys::is_quit_chord(&decode(b"x")[0]));
    }

    #[test]
    fn text_is_decoded_as_utf8_and_junk_bytes_are_dropped() {
        let typed = decode("aé─€😀z".as_bytes());
        let chars: Vec<KeyCode> = typed.iter().map(|k| k.code).collect();
        assert_eq!(
            chars,
            ['a', 'é', '─', '€', '😀', 'z'].map(KeyCode::Char).to_vec()
        );
        // A Latin-1 `é` (a lone 0xE9) swallows nothing after it; stray continuation bytes and
        // NUL are dropped.
        assert_eq!(
            decode(b"a\xe9b\x80\x00c"),
            ['a', 'b', 'c'].map(|c| ev(KeyCode::Char(c), NONE)).to_vec()
        );
    }

    #[test]
    fn sequences_may_arrive_in_pieces() {
        let mut d = KeyDecoder::default();
        assert!(d.feed(b"\x1b").is_empty());
        assert!(d.feed(b"[").is_empty());
        assert!(d.feed(b"1;5").is_empty());
        assert_eq!(
            d.feed(b"Ab"),
            vec![ev(KeyCode::Up, CTRL), ev(KeyCode::Char('b'), NONE)]
        );

        // A multi-byte character split across reads.
        let bytes = "é".as_bytes();
        assert!(d.feed(&bytes[..1]).is_empty());
        assert_eq!(d.feed(&bytes[1..]), vec![ev(KeyCode::Char('é'), NONE)]);
    }

    #[test]
    fn a_lone_esc_is_the_esc_key_only_after_the_wait() {
        let t0 = Instant::now();
        let mut d = KeyDecoder::default();
        assert!(d.feed_at(b"\x1b", t0).is_empty());
        assert!(d.flush_at(t0 + ESC_WAIT / 2).is_empty(), "still waiting");
        assert_eq!(d.flush_at(t0 + ESC_WAIT), vec![ev(KeyCode::Esc, NONE)]);
        assert!(d.flush_at(t0 + ESC_WAIT * 2).is_empty(), "nothing left");
        assert_eq!(
            d.feed_at(b"a", t0 + ESC_WAIT * 3),
            vec![ev(KeyCode::Char('a'), NONE)]
        );
    }

    #[test]
    fn the_rest_of_the_sequence_arriving_in_time_beats_the_wait() {
        let t0 = Instant::now();
        let mut d = KeyDecoder::default();
        assert!(d.feed_at(b"\x1b", t0).is_empty());
        assert_eq!(
            d.feed_at(b"[A", t0 + ESC_WAIT / 2),
            vec![ev(KeyCode::Up, NONE)]
        );
        assert!(d.flush_at(t0 + ESC_WAIT * 2).is_empty());
    }

    #[test]
    fn an_abandoned_sequence_becomes_esc_and_typed_text() {
        let t0 = Instant::now();
        let mut d = KeyDecoder::default();
        assert!(d.feed_at(b"\x1b[", t0).is_empty());
        assert_eq!(
            d.flush_at(t0 + ESC_WAIT),
            vec![ev(KeyCode::Esc, NONE), ev(KeyCode::Char('['), NONE)]
        );
        // The wait counts from where the unfinished sequence started, not from older input.
        assert_eq!(
            d.feed_at(b"a\x1b", t0 + ESC_WAIT * 4),
            vec![ev(KeyCode::Char('a'), NONE)]
        );
        assert!(d.flush_at(t0 + ESC_WAIT * 4 + ESC_WAIT / 2).is_empty());
        assert_eq!(d.flush_at(t0 + ESC_WAIT * 5), vec![ev(KeyCode::Esc, NONE)]);
    }

    #[test]
    fn esc_before_something_that_starts_no_sequence_is_esc_then_that_key() {
        assert_eq!(
            decode(b"\x1b\r"),
            vec![ev(KeyCode::Esc, NONE), ev(KeyCode::Enter, NONE)]
        );
        // Two Escs: the first is Esc at once, the second waits like any lone Esc.
        let t0 = Instant::now();
        let mut d = KeyDecoder::default();
        assert_eq!(d.feed_at(b"\x1b\x1b", t0), vec![ev(KeyCode::Esc, NONE)]);
        assert_eq!(d.flush_at(t0 + ESC_WAIT), vec![ev(KeyCode::Esc, NONE)]);
    }

    #[test]
    fn unknown_and_oversized_sequences_vanish_without_eating_what_follows() {
        // An unknown CSI (mouse, cursor report, F-key we do not map) and a private one.
        assert_eq!(decode(b"\x1b[9~a"), vec![ev(KeyCode::Char('a'), NONE)]);
        assert_eq!(decode(b"\x1b[?1;2cb"), vec![ev(KeyCode::Char('b'), NONE)]);
        assert_eq!(
            decode(b"\x1b[<0;10;5Mc"),
            vec![ev(KeyCode::Char('c'), NONE)]
        );
        // An unknown SS3 key.
        assert_eq!(decode(b"\x1bOZd"), vec![ev(KeyCode::Char('d'), NONE)]);
        // A control byte inside a sequence ends it; the byte is read as typed.
        assert_eq!(decode(b"\x1b[1\r"), vec![ev(KeyCode::Enter, NONE)]);
        // Endless parameters never accumulate.
        let mut d = KeyDecoder::default();
        let mut junk = b"\x1b[".to_vec();
        junk.extend_from_slice(&[b'1'; 200]);
        junk.push(b'x');
        let keys = d.feed(&junk);
        assert!(d.pending.is_empty(), "{:?}", d.pending);
        assert_eq!(keys.last(), Some(&ev(KeyCode::Char('x'), NONE)));
    }
}
