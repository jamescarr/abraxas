//! ICE mini-games: pure logic, driven by the UI through `IceInput` and `tick`.

pub mod breach;
pub mod daemon;
pub mod duel;
pub mod phreak;
pub mod router;
pub mod wordlock;

use std::time::Duration;

use crate::game::programs::{Deck, Program, charges, owns};

/// Normalised input handed to every mini-game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceInput {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Esc,
    Tab,
    Backspace,
    Char(char),
}

/// Result of a mini-game, once it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IceOutcome {
    Cleared,
    Failed { trace: f32, ego: i32 },
    Aborted { trace: f32 },
    Retreated,
    Integrated,
    Destroyed,
}

/// Behaviour shared by every mini-game; the UI drives games only through this trait.
pub trait IceGame {
    /// Feed one normalised input.
    fn handle(&mut self, input: IceInput);
    /// Advance game time by `dt`.
    fn tick(&mut self, dt: Duration);
    /// The result once the game is over.
    fn outcome(&self) -> Option<IceOutcome>;
}

/// Programs available to a mini-game, snapshotted from the deck when it starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Loadout {
    /// DECRYPT.COM charges (WORDLOCK key 1).
    pub decrypt: u8,
    /// SNIFFER.EXE charges (WORDLOCK key 2).
    pub sniffer: u8,
    /// BUFFER.EXE owned (BREACH buffer +1).
    pub buffer_plus: bool,
    /// ICEPICK.EXE owned (DUEL attack +6).
    pub icepick: bool,
    /// MIRROR.EXE charges (DUEL).
    pub mirror: u8,
    /// METRONOM.EXE owned (PHREAK replay).
    pub metronom: bool,
}

impl Loadout {
    /// Snapshot the relevant programs from a deck.
    pub fn from_deck(deck: &Deck) -> Self {
        Self {
            decrypt: charges(deck, Program::Decrypt),
            sniffer: charges(deck, Program::Sniffer),
            buffer_plus: owns(deck, Program::Buffer),
            icepick: owns(deck, Program::IcePick),
            mirror: charges(deck, Program::Mirror),
            metronom: owns(deck, Program::Metronom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadout_reads_charges_and_ownership() {
        let mut deck = Deck::new();
        deck.insert(Program::Decrypt, 2);
        deck.insert(Program::IcePick, 1);
        deck.insert(Program::Mirror, 1);
        let l = Loadout::from_deck(&deck);
        assert_eq!(l.decrypt, 2);
        assert_eq!(l.sniffer, 0);
        assert!(l.icepick && !l.buffer_plus && !l.metronom);
        assert_eq!(l.mirror, 1);
    }
}
