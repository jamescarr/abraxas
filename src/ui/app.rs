//! SCAFFOLD: the TUI app; implemented by the TUI core step.

use crate::cli::GlobalOpts;
use crate::config::{Config, Paths};

pub fn run(_global: &GlobalOpts, _config: Config, _paths: Paths) -> anyhow::Result<()> {
    anyhow::bail!("the TUI is not implemented yet")
}
