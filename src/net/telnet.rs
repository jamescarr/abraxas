//! The telnet layer: option negotiation, IAC escaping, NAWS window size.
//!
//! Just enough of RFC 854 for a character-mode game: [`HELLO`] puts the client in character mode
//! and asks for its window size, [`TelnetParser`] pulls data bytes and window sizes out of what
//! the client sends and answers every other negotiation with a refusal, [`escape`] makes outgoing
//! bytes safe to send.

/// "Interpret as command": introduces every telnet command and doubles as a data byte when sent
/// twice.
pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
/// Start of a subnegotiation (`IAC SB option ... IAC SE`).
pub const SB: u8 = 250;
/// End of a subnegotiation.
pub const SE: u8 = 240;

pub const OPT_ECHO: u8 = 1;
/// Suppress go-ahead: no half-duplex turn-taking.
pub const OPT_SGA: u8 = 3;
/// Negotiate about window size (RFC 1073).
pub const OPT_NAWS: u8 = 31;

/// Sent once after accept: `IAC WILL ECHO`, `IAC WILL SGA`, `IAC DO SGA`, `IAC DO NAWS`
/// (character-at-a-time, no local echo, report the window size).
pub const HELLO: [u8; 12] = [
    IAC, WILL, OPT_ECHO, IAC, WILL, OPT_SGA, IAC, DO, OPT_SGA, IAC, DO, OPT_NAWS,
];

/// The longest subnegotiation payload kept (a NAWS report is 5 bytes, 9 with every byte escaped);
/// longer ones are skipped without growing.
const SB_MAX: usize = 32;

/// What the parser found in a chunk of client bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelnetItem {
    /// A byte of user input.
    Data(u8),
    /// The client's window size in columns and rows (never 0 in either).
    Naws { cols: u16, rows: u16 },
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Data,
    /// A `\r` was just delivered: a `\n` or NUL right after it is the rest of the Enter key.
    AfterCr,
    /// After `IAC`.
    Command,
    /// After `IAC WILL|WONT|DO|DONT`: the option byte comes next.
    Option(u8),
    /// Inside `IAC SB`: the payload so far (option byte first).
    Sb(Vec<u8>),
    /// Inside a subnegotiation, after an `IAC`.
    SbIac(Vec<u8>),
}

/// Incremental parser; feed every byte read from the socket.
#[derive(Debug, Default)]
pub struct TelnetParser {
    state: State,
}

impl TelnetParser {
    /// Decode `bytes`; `reply` collects negotiation answers to write back to the client.
    ///
    /// A client's Enter arrives as `\r\n` or `\r\0` (or a bare `\r`): all three give one
    /// `Data(b'\r')`. Commands other than negotiation and window-size reports are ignored.
    pub fn feed(&mut self, bytes: &[u8], reply: &mut Vec<u8>) -> Vec<TelnetItem> {
        let mut items = Vec::new();
        for &b in bytes {
            match std::mem::take(&mut self.state) {
                State::Data => self.data(b, &mut items),
                State::AfterCr if b == b'\n' || b == 0 => {}
                State::AfterCr => self.data(b, &mut items),
                State::Command => match b {
                    IAC => items.push(TelnetItem::Data(IAC)),
                    WILL | WONT | DO | DONT => self.state = State::Option(b),
                    SB => self.state = State::Sb(Vec::new()),
                    // NOP, AYT, IP and the rest: nothing to do.
                    _ => {}
                },
                State::Option(verb) => negotiate(verb, b, reply),
                State::Sb(mut payload) => {
                    if b == IAC {
                        self.state = State::SbIac(payload);
                    } else {
                        keep(&mut payload, b);
                        self.state = State::Sb(payload);
                    }
                }
                State::SbIac(mut payload) => match b {
                    SE => items.extend(naws(&payload)),
                    IAC => {
                        keep(&mut payload, IAC);
                        self.state = State::Sb(payload);
                    }
                    // A command inside a subnegotiation is malformed: abandon it.
                    _ => {}
                },
            }
        }
        items
    }

    fn data(&mut self, b: u8, items: &mut Vec<TelnetItem>) {
        match b {
            IAC => self.state = State::Command,
            b'\r' => {
                items.push(TelnetItem::Data(b'\r'));
                self.state = State::AfterCr;
            }
            _ => items.push(TelnetItem::Data(b)),
        }
    }
}

fn keep(payload: &mut Vec<u8>, b: u8) {
    if payload.len() < SB_MAX {
        payload.push(b);
    }
}

/// Answer one `IAC verb option`. What [`HELLO`] asked for needs no answer, and refusals
/// (`WONT`, `DONT`) are not answered either (answering would start a negotiation loop); every
/// other option is declined.
fn negotiate(verb: u8, option: u8, reply: &mut Vec<u8>) {
    match (verb, option) {
        (DO, OPT_ECHO | OPT_SGA) | (WILL, OPT_SGA | OPT_NAWS) => {}
        (DO, _) => reply.extend_from_slice(&[IAC, WONT, option]),
        (WILL, _) => reply.extend_from_slice(&[IAC, DONT, option]),
        _ => {}
    }
}

/// The window size in a finished `NAWS` subnegotiation; clients that do not know their size yet
/// report 0 in one or both dimensions, which says nothing.
fn naws(payload: &[u8]) -> Option<TelnetItem> {
    let [OPT_NAWS, w1, w0, h1, h0] = *payload else {
        return None;
    };
    let cols = u16::from_be_bytes([w1, w0]);
    let rows = u16::from_be_bytes([h1, h0]);
    (cols > 0 && rows > 0).then_some(TelnetItem::Naws { cols, rows })
}

/// Double every `0xFF` so it is data, not `IAC`.
pub fn escape(bytes: &[u8]) -> Vec<u8> {
    let iacs = bytes.iter().filter(|&&b| b == IAC).count();
    let mut out = Vec::with_capacity(bytes.len() + iacs);
    for &b in bytes {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> (Vec<TelnetItem>, Vec<u8>) {
        let mut reply = Vec::new();
        let items = TelnetParser::default().feed(bytes, &mut reply);
        (items, reply)
    }

    fn data(bytes: &[u8]) -> Vec<TelnetItem> {
        bytes.iter().map(|&b| TelnetItem::Data(b)).collect()
    }

    #[test]
    fn hello_asks_for_character_mode_and_the_window_size() {
        assert_eq!(
            HELLO,
            [255, 251, 1, 255, 251, 3, 255, 253, 3, 255, 253, 31],
            "WILL ECHO, WILL SGA, DO SGA, DO NAWS"
        );
    }

    #[test]
    fn options_we_did_not_ask_for_are_declined() {
        // TERMINAL-TYPE (24), the way BSD telnet offers and asks for it.
        assert_eq!(parse(&[IAC, DO, 24]), (vec![], vec![IAC, WONT, 24]));
        assert_eq!(parse(&[IAC, WILL, 24]), (vec![], vec![IAC, DONT, 24]));
    }

    #[test]
    fn what_hello_asked_for_and_refusals_get_no_answer() {
        for seq in [
            [IAC, DO, OPT_ECHO],
            [IAC, DO, OPT_SGA],
            [IAC, WILL, OPT_SGA],
            [IAC, WILL, OPT_NAWS],
            [IAC, WONT, 24],
            [IAC, DONT, OPT_ECHO],
        ] {
            assert_eq!(parse(&seq), (vec![], vec![]), "{seq:?}");
        }
    }

    #[test]
    fn a_naws_block_reports_the_window_even_when_split_across_reads() {
        let block = [IAC, SB, OPT_NAWS, 0, 80, 0, 25, IAC, SE];
        assert_eq!(
            parse(&block).0,
            vec![TelnetItem::Naws { cols: 80, rows: 25 }]
        );

        let mut parser = TelnetParser::default();
        let mut reply = Vec::new();
        let mut items = Vec::new();
        for byte in block {
            items.extend(parser.feed(&[byte], &mut reply));
        }
        assert_eq!(items, vec![TelnetItem::Naws { cols: 80, rows: 25 }]);
        assert!(reply.is_empty());
    }

    #[test]
    fn naws_dimensions_may_contain_escaped_iac_bytes() {
        // 255 columns by 300 rows (0x012C).
        let block = [IAC, SB, OPT_NAWS, 0, IAC, IAC, 1, 0x2c, IAC, SE];
        assert_eq!(
            parse(&block).0,
            vec![TelnetItem::Naws {
                cols: 255,
                rows: 300
            }]
        );
    }

    #[test]
    fn a_window_of_zero_says_nothing() {
        for (w, h) in [(0, 0), (80, 0), (0, 25)] {
            let block = [IAC, SB, OPT_NAWS, 0, w, 0, h, IAC, SE];
            assert!(parse(&block).0.is_empty(), "{w}x{h}");
        }
    }

    #[test]
    fn other_subnegotiations_and_commands_are_ignored() {
        // TERMINAL-TYPE IS "xterm", then IAC NOP, then a typed key.
        let mut bytes = vec![IAC, SB, 24, 0];
        bytes.extend_from_slice(b"xterm");
        bytes.extend_from_slice(&[IAC, SE, IAC, 241, b'q']);
        assert_eq!(parse(&bytes), (data(b"q"), vec![]));
    }

    #[test]
    fn a_runaway_subnegotiation_is_skipped_and_the_stream_recovers() {
        let mut parser = TelnetParser::default();
        let mut reply = Vec::new();
        let mut bytes = vec![IAC, SB, OPT_NAWS];
        bytes.extend(std::iter::repeat_n(7, 10_000));
        assert!(parser.feed(&bytes, &mut reply).is_empty());
        let State::Sb(payload) = &parser.state else {
            panic!("still inside the subnegotiation: {:?}", parser.state);
        };
        assert_eq!(payload.len(), SB_MAX, "the payload stops growing");
        // The end of the block yields no window (the payload is not a NAWS report), and typing
        // works again after it.
        assert!(parser.feed(&[IAC, SE], &mut reply).is_empty());
        assert_eq!(parser.feed(b"a", &mut reply), data(b"a"));
    }

    #[test]
    fn a_doubled_iac_is_the_data_byte_255() {
        assert_eq!(parse(&[b'a', IAC, IAC, b'b']).0, data(&[b'a', 255, b'b']));
    }

    #[test]
    fn every_spelling_of_enter_is_one_carriage_return() {
        for enter in [&b"\r\n"[..], b"\r\0", b"\r"] {
            assert_eq!(parse(enter).0, data(b"\r"), "{enter:?}");
        }
        // Two Enters stay two, and a CR followed by a letter keeps the letter.
        assert_eq!(parse(b"\r\n\r\0").0, data(b"\r\r"));
        assert_eq!(parse(b"\ra").0, data(b"\ra"));
        // The pair may be split across reads; a bare LF is not part of an Enter.
        let mut parser = TelnetParser::default();
        let mut reply = Vec::new();
        assert_eq!(parser.feed(b"\r", &mut reply), data(b"\r"));
        assert!(parser.feed(b"\n", &mut reply).is_empty());
        assert_eq!(parser.feed(b"\n", &mut reply), data(b"\n"));
    }

    #[test]
    fn escape_doubles_every_iac() {
        assert_eq!(escape(&[1, 255, 2]), [1, 255, 255, 2]);
        assert_eq!(escape(b"plain"), b"plain");
        assert_eq!(escape(&[255, 255]), [255, 255, 255, 255]);
    }
}
