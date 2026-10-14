//! Where a pack's files come from: the embedded default pack or a directory.

use std::path::PathBuf;

use include_dir::{Dir, include_dir};

static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/packs/rollover");

/// A readable collection of pack files addressed by pack-relative paths (`pack.toml`,
/// `anchors/00-x.toml`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackSource {
    Embedded,
    Dir(PathBuf),
}

impl PackSource {
    /// Read a UTF-8 file. `Ok(None)` when it does not exist.
    pub fn read(&self, rel: &str) -> std::io::Result<Option<String>> {
        match self {
            PackSource::Embedded => Ok(EMBEDDED
                .get_file(rel)
                .map(|f| String::from_utf8_lossy(f.contents()).into_owned())),
            PackSource::Dir(root) => match std::fs::read_to_string(root.join(rel)) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// File names (not paths) directly inside `rel_dir`, sorted. Empty when the directory does
    /// not exist.
    pub fn list(&self, rel_dir: &str) -> std::io::Result<Vec<String>> {
        let mut names = match self {
            PackSource::Embedded => EMBEDDED
                .get_dir(rel_dir)
                .map(|d| {
                    d.files()
                        .filter_map(|f| f.path().file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            PackSource::Dir(root) => match std::fs::read_dir(root.join(rel_dir)) {
                Ok(rd) => {
                    let mut out = Vec::new();
                    for entry in rd {
                        let entry = entry?;
                        if entry.file_type()?.is_file() {
                            out.push(entry.file_name().to_string_lossy().into_owned());
                        }
                    }
                    out
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e),
            },
        };
        names.sort();
        Ok(names)
    }
}
