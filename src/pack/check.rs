//! SCAFFOLD: `abraxas pack check`; implemented by the pack-validation step.

use std::path::Path;
use std::process::ExitCode;

pub fn run(_dir: Option<&Path>) -> anyhow::Result<ExitCode> {
    anyhow::bail!("pack check is not implemented yet")
}
