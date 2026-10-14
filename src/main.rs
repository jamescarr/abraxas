use std::process::ExitCode;

use abraxas::cli::{Cli, Cmd, PackCmd};
use abraxas::config::{self, Paths};
use clap::Parser;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("abraxas: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();

    // Pack checking needs no config and no logging.
    if let Some(Cmd::Pack {
        cmd: PackCmd::Check { dir },
    }) = &cli.cmd
    {
        return abraxas::pack::check::run(dir.as_deref());
    }

    let paths = Paths::resolve();
    let mut cfg = config::load(&paths)?;
    cfg.apply_cli(&cli.global);

    match cli.cmd {
        Some(Cmd::Pack {
            cmd: PackCmd::Dream { levels, choose },
        }) => {
            abraxas::dream::run(&cli.global, cfg, &paths, levels, choose)?;
        }
        Some(Cmd::Pack {
            cmd: PackCmd::Check { .. },
        }) => unreachable!("handled above"),
        Some(Cmd::Play) | None => abraxas::ui::run(&cli.global, cfg, paths)?,
    }
    Ok(ExitCode::SUCCESS)
}
