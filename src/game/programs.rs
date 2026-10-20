//! Engine-defined programs the player can buy with RATIO and use in hosts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A program in the player's deck. Serialised as its DOS file name (`GHOST.COM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Program {
    #[serde(rename = "DECRYPT.COM")]
    Decrypt,
    #[serde(rename = "SNIFFER.EXE")]
    Sniffer,
    #[serde(rename = "BUFFER.EXE")]
    Buffer,
    #[serde(rename = "GHOST.COM")]
    Ghost,
    #[serde(rename = "ICEPICK.EXE")]
    IcePick,
    #[serde(rename = "MIRROR.EXE")]
    Mirror,
    #[serde(rename = "MANDALA.SYS")]
    Mandala,
    #[serde(rename = "METRONOM.EXE")]
    Metronom,
    #[serde(rename = "TEARDROP.SH")]
    Teardrop,
    #[serde(rename = "WINNUKE.EXE")]
    WinNuke,
    #[serde(rename = "SMURF.EXE")]
    Smurf,
    #[serde(rename = "SYNFLOOD.EXE")]
    SynFlood,
    #[serde(rename = "PINGOD.EXE")]
    PingOfDeath,
}

/// How a program is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// Bought once, kept forever.
    Persistent,
    /// Each purchase adds this many charges.
    Charges(u8),
}

impl Program {
    pub const ALL: [Program; 13] = [
        Program::Decrypt,
        Program::Sniffer,
        Program::Buffer,
        Program::Ghost,
        Program::IcePick,
        Program::Mirror,
        Program::Mandala,
        Program::Metronom,
        Program::Teardrop,
        Program::WinNuke,
        Program::Smurf,
        Program::SynFlood,
        Program::PingOfDeath,
    ];

    /// The DOS file name shown in the file library.
    pub fn file_name(self) -> &'static str {
        match self {
            Program::Decrypt => "DECRYPT.COM",
            Program::Sniffer => "SNIFFER.EXE",
            Program::Buffer => "BUFFER.EXE",
            Program::Ghost => "GHOST.COM",
            Program::IcePick => "ICEPICK.EXE",
            Program::Mirror => "MIRROR.EXE",
            Program::Mandala => "MANDALA.SYS",
            Program::Metronom => "METRONOM.EXE",
            Program::Teardrop => "TEARDROP.SH",
            Program::WinNuke => "WINNUKE.EXE",
            Program::Smurf => "SMURF.EXE",
            Program::SynFlood => "SYNFLOOD.EXE",
            Program::PingOfDeath => "PINGOD.EXE",
        }
    }

    pub fn from_file_name(name: &str) -> Option<Program> {
        Program::ALL
            .into_iter()
            .find(|p| p.file_name().eq_ignore_ascii_case(name))
    }

    /// Price in RATIO (KB).
    pub fn cost_kb(self) -> u32 {
        match self {
            Program::Decrypt | Program::Mandala => 48,
            Program::Sniffer
            | Program::Buffer
            | Program::Metronom
            | Program::Teardrop
            | Program::WinNuke
            | Program::Smurf
            | Program::SynFlood
            | Program::PingOfDeath => 64,
            Program::IcePick => 80,
            Program::Ghost | Program::Mirror => 96,
        }
    }

    pub fn kind(self) -> ProgramKind {
        match self {
            Program::Decrypt | Program::Sniffer => ProgramKind::Charges(2),
            Program::Ghost | Program::Mirror | Program::Mandala | Program::Teardrop => {
                ProgramKind::Charges(1)
            }
            Program::Buffer
            | Program::IcePick
            | Program::Metronom
            | Program::WinNuke
            | Program::Smurf
            | Program::SynFlood
            | Program::PingOfDeath => ProgramKind::Persistent,
        }
    }

    pub fn is_persistent(self) -> bool {
        self.kind() == ProgramKind::Persistent
    }

    /// Whether the BBS file library may offer it. TEARDROP.SH is IRC-only; the zine attacks are
    /// learned from articles on the hidden line.
    pub fn sold_on_bbs(self) -> bool {
        !matches!(
            self,
            Program::Teardrop
                | Program::WinNuke
                | Program::Smurf
                | Program::SynFlood
                | Program::PingOfDeath
        )
    }

    /// One-line description shown in the file library and by `deck`.
    pub fn effect(self) -> &'static str {
        match self {
            Program::Decrypt => "WORDLOCK: remove one dud word (key 1)",
            Program::Sniffer => "WORDLOCK: show likeness of the highlighted word (key 2)",
            Program::Buffer => "BREACH: buffer +1",
            Program::Ghost => "run ghost: trace rate x0.6 for this run",
            Program::IcePick => "DUEL: attack +6",
            Program::Mirror => "DUEL: reflect the next ICE hit",
            Program::Mandala => "restore 30 EGO (Self screen U, or run mandala)",
            Program::Metronom => "PHREAK: R replays the sequence once per round",
            Program::Teardrop => "IRC: /teardrop crashes one locked fserv (+15 trace)",
            Program::WinNuke => "DUEL [1]: WINNUKE, 20 damage, once per duel",
            Program::Smurf => {
                "DUEL [2]: SMURF, twice the ICE's announced hit (min 8), once per duel"
            }
            Program::SynFlood => {
                "DUEL [3]: SYNFLOOD, 6 damage and the ICE loses its turn, once per duel"
            }
            Program::PingOfDeath => "DUEL [4]: PINGOD, 30 damage, costs 8 EGO, once per duel",
        }
    }

    /// The DUEL attack this program teaches, when it is a zine attack.
    pub fn attack(self) -> Option<Attack> {
        Attack::ALL.into_iter().find(|a| a.program() == self)
    }
}

/// DUEL moves learned from zine articles on the hidden line; fired with keys 1-4, once per duel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Attack {
    WinNuke,
    Smurf,
    SynFlood,
    PingOfDeath,
}

impl Attack {
    /// In key order: `a as usize` indexes the loadout and the duel's fired flags.
    pub const ALL: [Attack; 4] = [
        Attack::WinNuke,
        Attack::Smurf,
        Attack::SynFlood,
        Attack::PingOfDeath,
    ];

    /// The deck program a zine article adds for this attack.
    pub fn program(self) -> Program {
        match self {
            Attack::WinNuke => Program::WinNuke,
            Attack::Smurf => Program::Smurf,
            Attack::SynFlood => Program::SynFlood,
            Attack::PingOfDeath => Program::PingOfDeath,
        }
    }

    /// The DUEL key.
    pub fn key(self) -> char {
        match self {
            Attack::WinNuke => '1',
            Attack::Smurf => '2',
            Attack::SynFlood => '3',
            Attack::PingOfDeath => '4',
        }
    }

    /// Name used in duel log lines.
    pub fn label(self) -> &'static str {
        match self {
            Attack::WinNuke => "WINNUKE",
            Attack::Smurf => "SMURF",
            Attack::SynFlood => "SYNFLOOD",
            Attack::PingOfDeath => "PINGOD",
        }
    }

    /// Short name for the duel panel.
    pub fn short(self) -> &'static str {
        match self {
            Attack::WinNuke => "NUKE",
            Attack::Smurf => "SMURF",
            Attack::SynFlood => "SYN",
            Attack::PingOfDeath => "PING",
        }
    }

    pub fn from_key(c: char) -> Option<Attack> {
        Attack::ALL.into_iter().find(|a| a.key() == c)
    }
}

/// The player's deck: persistent programs hold 1; consumables hold their remaining charges.
pub type Deck = BTreeMap<Program, u8>;

/// True when the deck holds the program (any charges for a consumable).
pub fn owns(deck: &Deck, program: Program) -> bool {
    deck.get(&program).is_some_and(|n| *n > 0)
}

pub fn charges(deck: &Deck, program: Program) -> u8 {
    deck.get(&program).copied().unwrap_or(0)
}

/// Add one purchase. Persistent programs are not bought twice: returns false (deck unchanged).
pub fn add(deck: &mut Deck, program: Program) -> bool {
    match program.kind() {
        ProgramKind::Persistent => {
            if owns(deck, program) {
                return false;
            }
            deck.insert(program, 1);
        }
        ProgramKind::Charges(n) => {
            let slot = deck.entry(program).or_insert(0);
            *slot = slot.saturating_add(n);
        }
    }
    true
}

/// Spend one charge of a consumable. Returns false when none is left; persistent programs are
/// never consumed here.
pub fn spend(deck: &mut Deck, program: Program) -> bool {
    if program.is_persistent() {
        return false;
    }
    match deck.get_mut(&program) {
        Some(n) if *n > 0 => {
            *n -= 1;
            if *n == 0 {
                deck.remove(&program);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumables_stack_and_deplete() {
        let mut deck = Deck::new();
        assert!(add(&mut deck, Program::Decrypt));
        assert!(add(&mut deck, Program::Decrypt));
        assert_eq!(charges(&deck, Program::Decrypt), 4);
        for _ in 0..4 {
            assert!(spend(&mut deck, Program::Decrypt));
        }
        assert!(!spend(&mut deck, Program::Decrypt));
        assert!(!owns(&deck, Program::Decrypt));
    }

    #[test]
    fn persistent_programs_are_bought_once_and_never_spent() {
        let mut deck = Deck::new();
        assert!(add(&mut deck, Program::IcePick));
        assert!(!add(&mut deck, Program::IcePick));
        assert!(!spend(&mut deck, Program::IcePick));
        assert!(owns(&deck, Program::IcePick));
    }

    #[test]
    fn serializes_as_file_name() {
        let json = serde_json::to_string(&Program::Ghost).unwrap();
        assert_eq!(json, "\"GHOST.COM\"");
        assert_eq!(Program::from_file_name("ghost.com"), Some(Program::Ghost));
        for p in Program::ALL {
            assert_eq!(Program::from_file_name(p.file_name()), Some(p));
            assert_eq!(
                serde_json::to_string(&p).unwrap(),
                format!("\"{}\"", p.file_name())
            );
        }
    }

    #[test]
    fn zine_attacks_are_persistent_unsold_programs_on_keys_one_to_four() {
        for (i, a) in Attack::ALL.into_iter().enumerate() {
            assert_eq!(a as usize, i);
            assert_eq!(a.program().attack(), Some(a));
            assert_eq!(Attack::from_key(a.key()), Some(a));
            assert!(a.program().is_persistent());
            assert!(!a.program().sold_on_bbs());
        }
        assert_eq!(Attack::from_key('5'), None);
        assert_eq!(Program::Ghost.attack(), None);
        assert!(Program::Ghost.sold_on_bbs());
    }
}
