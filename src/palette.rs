//! DOS VGA colours and the act palettes.

use crate::config::TruecolorMode;
use crate::pack::model::PaletteId;
use crate::text::parse_pipe;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// The 16 DOS colours, discriminants in DOS index order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dos {
    Black,
    Blue,
    Green,
    Cyan,
    Red,
    Magenta,
    Brown,
    LightGray,
    DarkGray,
    LightBlue,
    LightGreen,
    LightCyan,
    LightRed,
    LightMagenta,
    Yellow,
    White,
}

const RGB: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0x00, 0x00, 0xAA),
    (0x00, 0xAA, 0x00),
    (0x00, 0xAA, 0xAA),
    (0xAA, 0x00, 0x00),
    (0xAA, 0x00, 0xAA),
    (0xAA, 0x55, 0x00),
    (0xAA, 0xAA, 0xAA),
    (0x55, 0x55, 0x55),
    (0x55, 0x55, 0xFF),
    (0x55, 0xFF, 0x55),
    (0x55, 0xFF, 0xFF),
    (0xFF, 0x55, 0x55),
    (0xFF, 0x55, 0xFF),
    (0xFF, 0xFF, 0x55),
    (0xFF, 0xFF, 0xFF),
];

const ANSI: [u8; 16] = [0, 4, 2, 6, 1, 5, 3, 7, 8, 12, 10, 14, 9, 13, 11, 15];

impl Dos {
    pub const ALL: [Dos; 16] = [
        Dos::Black,
        Dos::Blue,
        Dos::Green,
        Dos::Cyan,
        Dos::Red,
        Dos::Magenta,
        Dos::Brown,
        Dos::LightGray,
        Dos::DarkGray,
        Dos::LightBlue,
        Dos::LightGreen,
        Dos::LightCyan,
        Dos::LightRed,
        Dos::LightMagenta,
        Dos::Yellow,
        Dos::White,
    ];

    /// DOS colour index 0..=15.
    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Option<Dos> {
        Dos::ALL.get(usize::from(i)).copied()
    }

    pub fn rgb(self) -> (u8, u8, u8) {
        RGB[usize::from(self.index())]
    }

    /// Index in the 16-colour ANSI palette.
    pub fn ansi_index(self) -> u8 {
        ANSI[usize::from(self.index())]
    }
}

/// Six-stop gradient for an act palette, dark to bright.
pub fn gradient_stops(id: PaletteId) -> [Dos; 6] {
    use Dos::*;
    match id {
        PaletteId::Nigredo => [Black, DarkGray, Red, LightRed, Brown, Yellow],
        PaletteId::Albedo => [Black, Blue, DarkGray, LightGray, LightCyan, White],
        PaletteId::Citrinitas => [Black, Brown, Red, LightRed, Yellow, White],
        PaletteId::Rubedo => [Black, Red, Magenta, LightRed, LightMagenta, White],
        PaletteId::Prism => [Black, Blue, Magenta, Cyan, LightGreen, White],
    }
}

/// Chrome (borders, accents) colour for an act palette.
pub fn chrome_dos(id: PaletteId) -> Dos {
    match id {
        PaletteId::Nigredo => Dos::Red,
        PaletteId::Albedo => Dos::LightCyan,
        PaletteId::Citrinitas => Dos::Yellow,
        PaletteId::Rubedo => Dos::LightMagenta,
        PaletteId::Prism => Dos::LightGreen,
    }
}

/// Colour mapper: truecolor RGB or the 16 indexed ANSI colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub truecolor: bool,
}

impl Palette {
    pub fn new(truecolor: bool) -> Self {
        Self { truecolor }
    }

    /// Reads `COLORTERM` from the process environment.
    pub fn detect(mode: TruecolorMode) -> Self {
        let colorterm = std::env::var("COLORTERM").ok();
        Self::from_env(mode, colorterm.as_deref())
    }

    pub fn from_env(mode: TruecolorMode, colorterm: Option<&str>) -> Self {
        let truecolor = match mode {
            TruecolorMode::Always => true,
            TruecolorMode::Never => false,
            TruecolorMode::Auto => matches!(colorterm, Some("truecolor" | "24bit")),
        };
        Self { truecolor }
    }

    pub fn color(&self, d: Dos) -> Color {
        if self.truecolor {
            let (r, g, b) = d.rgb();
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(d.ansi_index())
        }
    }

    pub fn gradient(&self, id: PaletteId) -> [Color; 6] {
        gradient_stops(id).map(|d| self.color(d))
    }

    pub fn chrome(&self, id: PaletteId) -> Color {
        self.color(chrome_dos(id))
    }

    pub fn style(&self, fg: Dos, bg: Dos) -> Style {
        Style::new().fg(self.color(fg)).bg(self.color(bg))
    }

    /// Pipe-coded text as styled lines; unset foreground is light gray, unset background black.
    pub fn pipe_lines(&self, text: &str) -> Vec<Line<'static>> {
        parse_pipe(text)
            .into_iter()
            .map(|spans| {
                Line::from(
                    spans
                        .into_iter()
                        .map(|s| {
                            let fg = s.fg.and_then(Dos::from_index).unwrap_or(Dos::LightGray);
                            let bg = s.bg.and_then(Dos::from_index).unwrap_or(Dos::Black);
                            Span::styled(s.text, self.style(fg, bg))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_truth_table() {
        use TruecolorMode::*;
        assert!(Palette::from_env(Always, None).truecolor);
        assert!(Palette::from_env(Always, Some("")).truecolor);
        assert!(!Palette::from_env(Never, Some("truecolor")).truecolor);
        assert!(Palette::from_env(Auto, Some("truecolor")).truecolor);
        assert!(Palette::from_env(Auto, Some("24bit")).truecolor);
        assert!(!Palette::from_env(Auto, Some("256color")).truecolor);
        assert!(!Palette::from_env(Auto, None).truecolor);
    }

    #[test]
    fn dos_to_rgb_and_ansi() {
        assert_eq!(Dos::Brown.rgb(), (0xAA, 0x55, 0x00));
        assert_eq!(Dos::Brown.ansi_index(), 3);
        assert_eq!(Dos::Yellow.index(), 14);
        assert_eq!(Dos::Yellow.rgb(), (0xFF, 0xFF, 0x55));
        assert_eq!(Dos::Yellow.ansi_index(), 11);
        assert_eq!(Dos::Blue.ansi_index(), 4);
        for (i, d) in Dos::ALL.iter().enumerate() {
            assert_eq!(usize::from(d.index()), i);
            assert_eq!(Dos::from_index(i as u8), Some(*d));
        }
        assert_eq!(Dos::from_index(16), None);
    }

    #[test]
    fn color_modes() {
        assert_eq!(
            Palette::new(true).color(Dos::Brown),
            Color::Rgb(0xAA, 0x55, 0x00)
        );
        assert_eq!(Palette::new(false).color(Dos::Brown), Color::Indexed(3));
        assert_eq!(Palette::new(false).color(Dos::Yellow), Color::Indexed(11));
    }

    #[test]
    fn gradients_follow_plan() {
        use Dos::*;
        assert_eq!(
            gradient_stops(PaletteId::Nigredo),
            [Black, DarkGray, Red, LightRed, Brown, Yellow]
        );
        assert_eq!(
            gradient_stops(PaletteId::Prism),
            [Black, Blue, Magenta, Cyan, LightGreen, White]
        );
        assert_eq!(chrome_dos(PaletteId::Rubedo), LightMagenta);
        let g = Palette::new(false).gradient(PaletteId::Albedo);
        assert_eq!(g[0], Color::Indexed(0));
        assert_eq!(g[4], Color::Indexed(Dos::LightCyan.ansi_index()));
        assert_eq!(g[5], Color::Indexed(15));
    }

    #[test]
    fn pipe_lines_colours() {
        let p = Palette::new(true);
        let lines = p.pipe_lines("a|12b|17c\nd");
        assert_eq!(lines.len(), 2);
        let s = &lines[0].spans;
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].style, p.style(Dos::LightGray, Dos::Black));
        assert_eq!(s[1].content, "b");
        assert_eq!(s[1].style, p.style(Dos::LightRed, Dos::Black));
        assert_eq!(s[2].style, p.style(Dos::LightRed, Dos::Blue));
        assert_eq!(lines[1].spans[0].style, p.style(Dos::LightRed, Dos::Blue));
    }
}
