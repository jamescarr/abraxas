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
    pub const ALL: [Program; 8] = [
        Program::Decrypt,
        Program::Sniffer,
        Program::Buffer,
        Program::Ghost,
        Program::IcePick,
        Program::Mirror,
        Program::Mandala,
        Program::Metronom,
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
            Program::Sniffer | Program::Buffer | Program::Metronom => 64,
            Program::IcePick => 80,
            Program::Ghost | Program::Mirror => 96,
        }
    }

    pub fn kind(self) -> ProgramKind {
        match self {
            Program::Decrypt | Program::Sniffer => ProgramKind::Charges(2),
            Program::Ghost | Program::Mirror | Program::Mandala => ProgramKind::Charges(1),
            Program::Buffer | Program::IcePick | Program::Metronom => ProgramKind::Persistent,
        }
    }

    pub fn is_persistent(self) -> bool {
        self.kind() == ProgramKind::Persistent
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
        }
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
        }
    }
}
