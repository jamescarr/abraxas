//! `abraxas pack check [DIR]`.

use std::path::Path;
use std::process::ExitCode;

use super::{PackIssue, PackSource, load, validate::validate};

/// Load and validate the pack in `source`. On success the one-line summary, otherwise every
/// issue found (parse problems from the loader, or content problems from the validator).
pub fn check_source(source: &PackSource) -> Result<String, Vec<PackIssue>> {
    let pack = load(source).map_err(|e| e.0)?;
    let issues = validate(&pack);
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(format!(
        "pack {}: OK ({} acts, {} anchors, {} characters, endless {})",
        pack.meta.id,
        pack.meta.act.len(),
        pack.anchors.len(),
        pack.cast.len(),
        if pack.endless_enabled() { "on" } else { "off" }
    ))
}

/// Check `dir`, or the embedded pack when `None`. Prints every issue (or the OK line) to stdout.
pub fn run(dir: Option<&Path>) -> anyhow::Result<ExitCode> {
    let source = match dir {
        Some(d) => PackSource::Dir(d.to_path_buf()),
        None => PackSource::Embedded,
    };
    match check_source(&source) {
        Ok(summary) => {
            println!("{summary}");
            Ok(ExitCode::SUCCESS)
        }
        Err(issues) => {
            for issue in &issues {
                println!("{issue}");
            }
            Ok(ExitCode::from(1))
        }
    }
}
