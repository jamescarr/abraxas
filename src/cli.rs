//! Command-line interface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "abraxas",
    version,
    about = "A 1999 BBS hacker game that dreams its own levels."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOpts,

    /// Defaults to `play`.
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

/// Flags that override the config file, which overrides built-in defaults.
#[derive(Debug, Clone, Default, Args)]
pub struct GlobalOpts {
    /// Never call the AI; build every level from the pack's pool.
    #[arg(long, global = true)]
    pub offline: bool,

    /// Run seed (random when absent).
    #[arg(long, global = true)]
    pub seed: Option<u64>,

    /// Load a story pack from a directory instead of the built-in one.
    #[arg(long, global = true, value_name = "DIR")]
    pub pack: Option<PathBuf>,

    /// AI model name.
    #[arg(long, global = true)]
    pub model: Option<String>,

    /// OpenAI-compatible API base URL.
    #[arg(long, global = true, value_name = "URL")]
    pub base_url: Option<String>,

    /// Calm mode: no flicker, glitch, dissolve or CRT effects; visions are frozen.
    #[arg(long, global = true)]
    pub calm: bool,

    /// Modem speed in baud; text arrives at baud/10 characters per second. 0 = instant.
    #[arg(long, global = true)]
    pub baud: Option<u32>,

    /// Developer mode: `xyzzy` completes the objective, generation sources are shown.
    #[arg(long, global = true, hide = true)]
    pub dev: bool,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Play the game (the default).
    Play,
    /// Story-pack tools.
    Pack {
        #[command(subcommand)]
        cmd: PackCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum PackCmd {
    /// Validate a story pack (the built-in pack when DIR is omitted).
    Check { dir: Option<PathBuf> },
    /// Generate levels headlessly with the real director and print them.
    Dream {
        /// How many levels to generate.
        #[arg(long, default_value_t = 5)]
        levels: u32,
        /// Which stance to pick at each choice.
        #[arg(long, value_enum, default_value_t = Choose::Random)]
        choose: Choose,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Choose {
    Integrate,
    Repress,
    Neutral,
    Random,
}
