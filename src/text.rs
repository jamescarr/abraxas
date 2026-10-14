//! Text helpers: CP437, pipe codes, wrapping, placeholders.

/// The Shadow's handle: the player's handle reversed with ASCII case swapped, plus `_` when the
/// result would equal the original.
pub fn shadow_handle(handle: &str) -> String {
    let swapped: String = handle
        .chars()
        .rev()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    if swapped == handle {
        format!("{swapped}_")
    } else {
        swapped
    }
}

/// A player handle: 2-16 characters of `[A-Za-z0-9_-]`.
pub fn valid_handle(s: &str) -> bool {
    (2..=16).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A DOS 8.3 name: `^[A-Z0-9_-]{1,8}\.[A-Z0-9]{1,3}$`.
pub fn is_83_name(s: &str) -> bool {
    let Some((base, ext)) = s.split_once('.') else {
        return false;
    };
    (1..=8).contains(&base.len())
        && (1..=3).contains(&ext.len())
        && base
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && ext
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Byte value to Unicode for code page 437, with the graphic glyphs for 0x01-0x1F and 0x7F.
/// 0x00 and 0xFF (NBSP) are rendered as a plain space.
#[rustfmt::skip]
pub const CP437_TABLE: [char; 256] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂',
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ',
];

/// True for any character in the CP437 table, and for `\n`.
pub fn is_cp437(c: char) -> bool {
    if c == '\n' || ('\u{20}'..='\u{7e}').contains(&c) {
        return true;
    }
    CP437_TABLE.contains(&c)
}

/// Maps arbitrary text into the CP437 repertoire: curly quotes to straight, en/em dash to `-`,
/// ellipsis to `...`, NBSP to space, tab to 4 spaces, other unknown characters to `?`, control
/// characters stripped except `\n`.
pub fn to_cp437_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\u{a0}' => out.push(' '),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            '\u{2013}' | '\u{2014}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            c if c.is_control() => {}
            c if is_cp437(c) => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

/// A pipe code at the start of `chars` (`|` plus two digits 00-23), as its value.
fn pipe_code(rest: &[char]) -> Option<u8> {
    match rest {
        ['|', a, b, ..] if a.is_ascii_digit() && b.is_ascii_digit() => {
            let v = (*a as u8 - b'0') * 10 + (*b as u8 - b'0');
            (v <= 23).then_some(v)
        }
        _ => None,
    }
}

/// Removes valid `|00`-`|23` pipe codes; any other `|` is kept.
pub fn strip_pipe_codes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if pipe_code(&chars[i..]).is_some() {
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// A run of text with one colour state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeSpan {
    pub text: String,
    /// DOS foreground 0-15.
    pub fg: Option<u8>,
    /// DOS background 0-7.
    pub bg: Option<u8>,
}

/// Parses pipe-coded text into lines of coloured spans. Colour state persists across lines.
/// A trailing newline does not produce an extra empty line.
pub fn parse_pipe(text: &str) -> Vec<Vec<PipeSpan>> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines: Vec<Vec<PipeSpan>> = Vec::new();
    let mut spans: Vec<PipeSpan> = Vec::new();
    let mut buf = String::new();
    let (mut fg, mut bg): (Option<u8>, Option<u8>) = (None, None);
    let mut pending_line = false;
    let mut i = 0;

    fn flush(buf: &mut String, spans: &mut Vec<PipeSpan>, fg: Option<u8>, bg: Option<u8>) {
        if !buf.is_empty() {
            spans.push(PipeSpan {
                text: std::mem::take(buf),
                fg,
                bg,
            });
        }
    }

    while i < chars.len() {
        if let Some(code) = pipe_code(&chars[i..]) {
            flush(&mut buf, &mut spans, fg, bg);
            if code <= 15 {
                fg = Some(code);
            } else {
                bg = Some(code - 16);
            }
            pending_line = true;
            i += 3;
        } else if chars[i] == '\n' {
            flush(&mut buf, &mut spans, fg, bg);
            lines.push(std::mem::take(&mut spans));
            pending_line = false;
            i += 1;
        } else {
            buf.push(chars[i]);
            pending_line = true;
            i += 1;
        }
    }
    flush(&mut buf, &mut spans, fg, bg);
    if pending_line {
        lines.push(spans);
    }
    lines
}

/// `(max columns, rows)` of pipe-coded text after stripping codes.
pub fn pipe_size(text: &str) -> (usize, usize) {
    let lines = parse_pipe(text);
    let cols = lines
        .iter()
        .map(|l| l.iter().map(|s| s.text.chars().count()).sum::<usize>())
        .max()
        .unwrap_or(0);
    (cols, lines.len())
}

/// Greedy word wrap by character width. Explicit newlines are preserved (blank lines kept),
/// words longer than `width` are split, and width 0 gives one line per word.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut words = para.split_whitespace().peekable();
        if words.peek().is_none() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for word in words {
            let mut word_chars: Vec<char> = word.chars().collect();
            if width == 0 {
                out.push(word.to_string());
                continue;
            }
            if cur_len > 0 && cur_len + 1 + word_chars.len() <= width {
                cur.push(' ');
                cur.push_str(word);
                cur_len += 1 + word_chars.len();
                continue;
            }
            if cur_len > 0 {
                out.push(std::mem::take(&mut cur));
            }
            while word_chars.len() > width {
                let rest = word_chars.split_off(width);
                out.push(word_chars.iter().collect());
                word_chars = rest;
            }
            cur_len = word_chars.len();
            cur = word_chars.into_iter().collect();
        }
        if cur_len > 0 {
            out.push(cur);
        }
    }
    out
}

/// Cuts to `max_chars` characters; when cut, the result is exactly `max_chars` long and ends in
/// `...` (a plain cut when `max_chars < 3`).
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars < 3 {
        return text.chars().take(max_chars).collect();
    }
    let mut out: String = text.chars().take(max_chars - 3).collect();
    out.push_str("...");
    out
}

/// Values for authored-text placeholders.
pub struct Subst<'a> {
    pub handle: &'a str,
    pub shadow: &'a str,
    pub real_name: &'a str,
    pub motif: Option<&'a str>,
    pub exit: Option<&'a str>,
}

/// Replaces `{handle}`, `{shadow}`, `{real_name}`, `{motif}`, `{exit}`; `{{` and `}}` become
/// literal braces. Unknown or unavailable placeholders are left verbatim.
pub fn fill_placeholders(text: &str, s: &Subst) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(['{', '}']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push('{');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("}}") {
            out.push('}');
            rest = after;
        } else if tail.starts_with('{') {
            let Some(end) = tail.find('}') else {
                out.push_str(tail);
                return out;
            };
            let value = match &tail[1..end] {
                "handle" => Some(s.handle),
                "shadow" => Some(s.shadow),
                "real_name" => Some(s.real_name),
                "motif" => s.motif,
                "exit" => s.exit,
                _ => None,
            };
            match value {
                Some(v) => out.push_str(v),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push('}');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Validates placeholders: every `{name}` must be in `allowed` (names without braces).
/// `{{` and `}}` are literal braces.
pub fn check_placeholders(text: &str, allowed: &[&str]) -> Result<(), String> {
    let mut rest = text;
    while let Some(pos) = rest.find(['{', '}']) {
        let tail = &rest[pos..];
        if let Some(after) = tail.strip_prefix("{{").or_else(|| tail.strip_prefix("}}")) {
            rest = after;
        } else if tail.starts_with('{') {
            let Some(end) = tail.find('}') else {
                return Err(format!("unterminated placeholder '{}'", truncate(tail, 20)));
            };
            let name = &tail[1..end];
            if !allowed.contains(&name) {
                return Err(format!("unknown placeholder '{{{name}}}'"));
            }
            rest = &tail[end + 1..];
        } else {
            rest = &tail[1..];
        }
    }
    Ok(())
}

/// Sanitises AI text: CP437-safe, no pipe codes or control characters, collapsed whitespace
/// (per line, trimmed), at most one blank line when `keep_newlines`, newlines turned into
/// spaces otherwise, then truncated to `max_chars`.
pub fn clean_ai_text(s: &str, max_chars: usize, keep_newlines: bool) -> String {
    let safe = strip_pipe_codes(&to_cp437_safe(s));
    let lines = safe
        .split('\n')
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "));
    let joined = if keep_newlines {
        let mut out = String::with_capacity(safe.len());
        let mut blanks = 0usize;
        for line in lines {
            if line.is_empty() {
                blanks += 1;
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
                if blanks > 0 {
                    out.push('\n');
                }
            }
            blanks = 0;
            out.push_str(&line);
        }
        out
    } else {
        lines
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    truncate(&joined, max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, fg: Option<u8>, bg: Option<u8>) -> PipeSpan {
        PipeSpan {
            text: text.into(),
            fg,
            bg,
        }
    }

    #[test]
    fn cp437_safe_cases() {
        let cases = [
            ("\u{201c}hi\u{201d} \u{2018}x\u{2019}", "\"hi\" 'x'"),
            ("a\u{2013}b\u{2014}c", "a-b-c"),
            ("wait\u{2026}", "wait..."),
            ("a\u{a0}b", "a b"),
            ("a\tb", "a    b"),
            ("smile \u{1f600}!", "smile ?!"),
            ("a\u{7}b\u{0}c\r\n", "abc\n"),
            ("line1\nline2", "line1\nline2"),
            ("caf\u{e9} \u{2593}\u{263a}", "caf\u{e9} \u{2593}\u{263a}"),
            ("\u{4e2d}", "?"),
        ];
        for (input, expected) in cases {
            assert_eq!(to_cp437_safe(input), expected, "input {input:?}");
        }
    }

    #[test]
    fn cp437_safe_output_is_always_cp437() {
        let nasty: String = (0u32..0x3000)
            .filter_map(char::from_u32)
            .chain("\u{1f600}\u{200d}\u{fe0f}".chars())
            .collect();
        assert!(to_cp437_safe(&nasty).chars().all(is_cp437));
    }

    #[test]
    fn cp437_table_invariants() {
        for (b, &c) in CP437_TABLE.iter().enumerate() {
            let w = ratatui::text::Span::raw(c.to_string()).width();
            assert_eq!(w, 1, "byte {b:#x} char {c:?}");
            assert!(is_cp437(c));
        }
        assert_eq!(CP437_TABLE[0x01], '\u{263a}');
        assert_eq!(CP437_TABLE[0x7f], '\u{2302}');
        assert_eq!(CP437_TABLE[0xc9], '\u{2554}');
        assert_eq!(CP437_TABLE[0xdb], '\u{2588}');
        assert_eq!(CP437_TABLE[0xe9], '\u{398}');
        assert_eq!(CP437_TABLE[0x41], 'A');
        // 0x00 and 0xFF are both spaces; every other entry is unique.
        let mut seen = std::collections::HashSet::new();
        for (b, &c) in CP437_TABLE.iter().enumerate() {
            if b != 0 && b != 0xff {
                assert!(seen.insert(c), "duplicate {c:?} at {b:#x}");
            }
        }
        assert!(!is_cp437('\u{1f600}'));
        assert!(is_cp437('\n'));
    }

    #[test]
    fn wrap_words_and_limits() {
        assert_eq!(wrap("the quick brown fox", 9), ["the quick", "brown fox"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("abcdefghij kl", 4), ["abcd", "efgh", "ij", "kl"]);
        assert_eq!(wrap("ab cd", 0), ["ab", "cd"]);
        assert_eq!(wrap("", 5), [""]);
        let text =
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit supercalifragilistic end";
        for w in 1..30 {
            for line in wrap(text, w) {
                assert!(line.chars().count() <= w, "width {w}: {line:?}");
            }
        }
        // exact fit stays on one line
        assert_eq!(wrap("abc def", 7), ["abc def"]);
    }

    #[test]
    fn parse_pipe_codes() {
        assert_eq!(
            parse_pipe("|12red|07 plain"),
            vec![vec![
                span("red", Some(12), None),
                span(" plain", Some(7), None)
            ]]
        );
        assert_eq!(
            parse_pipe("|17on blue|23x"),
            vec![vec![
                span("on blue", None, Some(1)),
                span("x", None, Some(7))
            ]]
        );
        assert_eq!(
            parse_pipe("|24 |1x |9 |"),
            vec![vec![span("|24 |1x |9 |", None, None)]]
        );
        // persistence across lines; empty spans are not emitted; trailing newline adds no line
        assert_eq!(
            parse_pipe("|14a\nb\n"),
            vec![
                vec![span("a", Some(14), None)],
                vec![span("b", Some(14), None)]
            ]
        );
        assert_eq!(parse_pipe("|01|02x"), vec![vec![span("x", Some(2), None)]]);
        assert_eq!(parse_pipe("a\n\nb").len(), 3);
        assert!(parse_pipe("").is_empty());
    }

    #[test]
    fn strip_and_size() {
        assert_eq!(strip_pipe_codes("|15ab|23c|24d|x"), "abc|24d|x");
        assert_eq!(pipe_size("|12abc\nde|07f\n"), (3, 2));
        assert_eq!(pipe_size(""), (0, 0));
    }

    #[test]
    fn truncate_rules() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("hello world", 8).chars().count(), 8);
        assert_eq!(truncate("hello", 2), "he");
        assert_eq!(truncate("h\u{e9}llo w\u{e9}rld", 6), "h\u{e9}l...");
        assert_eq!(truncate("abcd", 3), "...");
    }

    #[test]
    fn placeholders_fill() {
        let s = Subst {
            handle: "H",
            shadow: "h",
            real_name: "Jo",
            motif: Some("rain"),
            exit: None,
        };
        assert_eq!(
            fill_placeholders("{handle}/{shadow}/{real_name}/{motif}", &s),
            "H/h/Jo/rain"
        );
        assert_eq!(
            fill_placeholders("{{handle}} {exit} {bogus}", &s),
            "{handle} {exit} {bogus}"
        );
        assert_eq!(fill_placeholders("open { here", &s), "open { here");
        assert_eq!(fill_placeholders("a }} b", &s), "a } b");
    }

    #[test]
    fn placeholders_check() {
        let ok = ["handle", "shadow"];
        assert!(check_placeholders("{handle} and {{literal}}", &ok).is_ok());
        let e = check_placeholders("hi {nope}", &ok).unwrap_err();
        assert!(e.contains("nope"), "{e}");
        assert!(check_placeholders("{real_name}", &ok).is_err());
        let e = check_placeholders("broken {handle", &ok).unwrap_err();
        assert!(e.contains("unterminated"), "{e}");
    }

    #[test]
    fn clean_ai_text_rules() {
        assert_eq!(
            clean_ai_text(
                "  |12Hello\t\u{201c}world\u{201d}   again \u{1f600}  ",
                100,
                false
            ),
            "Hello \"world\" again ?"
        );
        assert_eq!(clean_ai_text("a\nb\n\n\nc", 100, false), "a b c");
        assert_eq!(
            clean_ai_text("\n a  b \n\n\n\n c \n", 100, true),
            "a b\n\nc"
        );
        assert_eq!(clean_ai_text("a\nb", 100, true), "a\nb");
        assert_eq!(clean_ai_text("0123456789", 8, false), "01234...");
        assert_eq!(clean_ai_text("x\u{7}y", 10, false), "xy");
    }

    #[test]
    fn shadow_handle_rules() {
        assert_eq!(shadow_handle("Abc"), "CBa");
        assert_eq!(shadow_handle("a1B"), "b1A");
        // reversal + case swap equals the original only for a case-swapped palindrome pair
        assert_eq!(shadow_handle("aA"), "aA_");
        assert_eq!(shadow_handle("11"), "11_");
    }

    #[test]
    fn handle_and_83() {
        assert!(valid_handle("ab"));
        assert!(valid_handle("Wren_99-x"));
        assert!(!valid_handle("a"));
        assert!(!valid_handle("seventeen_chars_x"));
        assert!(!valid_handle("bad name"));
        assert!(valid_handle("0123456789abcdef"));
        assert!(is_83_name("WREN-00.LOG"));
        assert!(is_83_name("A.B"));
        assert!(!is_83_name("toolongname.TXT"));
        assert!(!is_83_name("NOEXT"));
        assert!(!is_83_name("a.TXT"));
        assert!(!is_83_name("AB.TEXT"));
        assert!(!is_83_name(".TXT"));
        assert!(!is_83_name("A.B.C"));
    }
}
