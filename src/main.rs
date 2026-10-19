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
    init_logging(&paths);

    match cli.cmd {
        Some(Cmd::Pack {
            cmd: PackCmd::Dream { levels, choose },
        }) => {
            abraxas::dream::run(&cli.global, cfg, &paths, levels, choose)?;
        }
        Some(Cmd::Pack {
            cmd: PackCmd::Check { .. },
        }) => unreachable!("handled above"),
        Some(Cmd::Serve { listen }) => {
            abraxas::net::server::run(&cli.global, cfg, paths, &listen)?;
        }
        Some(Cmd::Play) | None => abraxas::ui::run(&cli.global, cfg, paths)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Log to `abraxas.log` (truncated at each launch); never to the terminal, which the TUI owns.
/// The filter comes from `ABRAXAS_LOG` (default `info`). Logging failures are not fatal.
fn init_logging(paths: &Paths) {
    use std::sync::Mutex;
    use tracing_subscriber::EnvFilter;

    if std::fs::create_dir_all(&paths.data_dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::File::create(paths.log_file()) else {
        return;
    };
    let filter = EnvFilter::try_from_env("ABRAXAS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // A second initialisation (tests, nested runs) is harmless; ignore the error.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .try_init();
}
