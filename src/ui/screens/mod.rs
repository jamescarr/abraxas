//! One module per screen, the `Screen` enum that dispatches to them, and the `Nav` result every
//! screen returns.
//!
//! # Writing a screen
//!
//! A screen is a struct in `screens/<name>.rs` with `new(ctx: &mut Ctx) -> Self` (the Dialing
//! screen also takes its [`DialTarget`](dialer::DialTarget)) and an implementation of
//! [`ScreenImpl`]. The `Screen` enum has one variant per screen and forwards statically.
//!
//! * `handle_key` runs for every key *press* that no overlay (help, hang-up prompt) consumed.
//! * `tick` runs once per frame (not while an overlay is open) with the frame time.
//! * `render` draws into `area`, always the 80x25 canvas at its absolute position in the frame.
//!   Rows 0..24 are yours when [`ScreenImpl::wants_status`] is true (the app draws the status bar
//!   in row 24), otherwise all 25.
//! * Visual effects are requested through `ctx.fx` (see [`FxCommand`](crate::ui::app::FxCommand));
//!   overlays through `ctx.overlay_request`.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::ui::app::Ctx;

pub mod archive;
pub mod beat;
pub mod board;
pub mod boot;
pub mod chat;
pub mod choice;
pub mod death;
pub mod descent;
pub mod dialer;
pub mod dialing;
pub mod ending;
pub mod exits;
pub mod files;
pub mod help;
pub mod ice_view;
pub mod irc;
pub mod level_card;
pub mod login;
pub mod menu;
pub mod psyche;
pub mod run;
pub mod users;

pub use archive::ArchiveScreen;
pub use beat::BeatScreen;
pub use board::BoardScreen;
pub use boot::BootScreen;
pub use chat::ChatScreen;
pub use choice::ChoiceScreen;
pub use death::DeathScreen;
pub use descent::DescentScreen;
pub use dialer::DialerScreen;
pub use dialing::DialingScreen;
pub use ending::EndingScreen;
pub use exits::ExitsScreen;
pub use files::FilesScreen;
pub use irc::IrcScreen;
pub use level_card::LevelCardScreen;
pub use login::LoginScreen;
pub use menu::MenuScreen;
pub use psyche::PsycheScreen;
pub use run::RunScreen;
pub use users::UsersScreen;

/// What a screen wants the app to do after a key or a tick.
pub enum Nav {
    /// Stay on this screen.
    Stay,
    /// Replace this screen (the stack underneath is untouched).
    To(Box<Screen>),
    /// Keep this screen underneath and show the new one on top.
    Push(Box<Screen>),
    /// Pop the screen underneath. With nothing underneath: the Menu if a run exists, else the
    /// Dialer.
    Back,
    /// Autosave and leave the program at once (no prompt; for the `Hang up? [Y/n]` prompt set
    /// `ctx.overlay_request = Some(Overlay::ConfirmQuit)`).
    Quit,
}

/// The behaviour every screen implements.
pub trait ScreenImpl {
    /// A key press (never a release or repeat). Overlays and quit chords are handled before it.
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav;

    /// Advance by one frame. Not called while an overlay is open.
    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav;

    /// Draw into `area` (the 80x25 canvas). Pure with respect to game state.
    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx);

    /// Whether the status bar (canvas row 24) is shown below this screen. It also needs a run.
    fn wants_status(&self) -> bool {
        false
    }

    /// True while the screen has a text input focused, so `?` types a question mark instead of
    /// opening help.
    fn in_text_input(&self) -> bool {
        false
    }
}

macro_rules! screens {
    ($($variant:ident($ty:ty)),* $(,)?) => {
        /// Every screen of the game, dispatched statically.
        #[allow(clippy::large_enum_variant)]
        pub enum Screen {
            $($variant($ty)),*
        }

        impl Screen {
            /// The variant name, for logs and tests.
            pub fn name(&self) -> &'static str {
                match self {
                    $(Screen::$variant(_) => stringify!($variant)),*
                }
            }
        }

        impl ScreenImpl for Screen {
            fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
                match self {
                    $(Screen::$variant(s) => s.handle_key(key, ctx)),*
                }
            }

            fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
                match self {
                    $(Screen::$variant(s) => s.tick(dt, ctx)),*
                }
            }

            fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
                match self {
                    $(Screen::$variant(s) => s.render(frame, area, ctx)),*
                }
            }

            fn wants_status(&self) -> bool {
                match self {
                    $(Screen::$variant(s) => s.wants_status()),*
                }
            }

            fn in_text_input(&self) -> bool {
                match self {
                    $(Screen::$variant(s) => s.in_text_input()),*
                }
            }
        }
    };
}

screens! {
    Boot(BootScreen),
    Dialer(DialerScreen),
    Dialing(DialingScreen),
    Login(LoginScreen),
    LevelCard(LevelCardScreen),
    Menu(MenuScreen),
    Board(BoardScreen),
    Files(FilesScreen),
    Chat(ChatScreen),
    Irc(IrcScreen),
    Users(UsersScreen),
    Archive(ArchiveScreen),
    Psyche(PsycheScreen),
    Run(RunScreen),
    Beat(BeatScreen),
    Choice(ChoiceScreen),
    Exits(ExitsScreen),
    Descent(DescentScreen),
    Death(DeathScreen),
    Ending(EndingScreen),
}
