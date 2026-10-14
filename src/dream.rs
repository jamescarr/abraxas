//! SCAFFOLD: headless `pack dream`; implemented by the director step.

use crate::cli::{Choose, GlobalOpts};
use crate::config::{Config, Paths};

pub fn run(
    _global: &GlobalOpts,
    _config: Config,
    _paths: &Paths,
    _levels: u32,
    _choose: Choose,
) -> anyhow::Result<()> {
    anyhow::bail!("pack dream is not implemented yet")
}
