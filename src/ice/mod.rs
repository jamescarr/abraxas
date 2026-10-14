//! ICE mini-games: pure logic, driven by the UI through `IceInput` and `tick`.

pub mod breach;
pub mod duel;
pub mod phreak;
pub mod router;
pub mod wordlock;

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
