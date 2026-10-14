//! Story packs: model, loading from the embedded pack or a directory, validation.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

pub mod check;
pub mod model;
pub mod source;
pub mod validate;

pub use model::StoryPack;
pub use source::PackSource;

use model::{Anchor, AnchorEntry, CastFile, PackMeta, Pool};

/// One problem found in a pack. Printed as `{file}: {field}: {message}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackIssue {
    pub file: String,
    pub field: String,
    pub message: String,
}

impl PackIssue {
    pub fn new(
        file: impl Into<String>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PackIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.field, self.message)
    }
}

/// Every problem found in a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackErrors(pub Vec<PackIssue>);

impl fmt::Display for PackErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, issue) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PackErrors {}

fn parse_toml<T: serde::de::DeserializeOwned>(
    file: &str,
    text: &str,
    issues: &mut Vec<PackIssue>,
) -> Option<T> {
    match toml::from_str::<T>(text) {
        Ok(v) => Some(v),
        Err(e) => {
            let line = e
                .span()
                .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
            let field = match line {
                Some(l) => format!("line {l}"),
                None => "toml".to_string(),
            };
            issues.push(PackIssue::new(file, field, e.message().to_string()));
            None
        }
    }
}

fn read_or_issue(source: &PackSource, rel: &str, issues: &mut Vec<PackIssue>) -> Option<String> {
    match source.read(rel) {
        Ok(text) => text,
        Err(e) => {
            issues.push(PackIssue::new(rel, "file", format!("cannot read: {e}")));
            None
        }
    }
}

fn list_or_issue(source: &PackSource, dir: &str, issues: &mut Vec<PackIssue>) -> Vec<String> {
    match source.list(dir) {
        Ok(names) => names,
        Err(e) => {
            issues.push(PackIssue::new(dir, "dir", format!("cannot list: {e}")));
            Vec::new()
        }
    }
}

/// The embedded pack's pool, used as the base every other pack inherits from.
fn embedded_pool() -> Pool {
    PackSource::Embedded
        .read("pool.toml")
        .ok()
        .flatten()
        .and_then(|text| toml::from_str::<Pool>(&text).ok())
        .unwrap_or_default()
}

/// Parse a pack from `source`. Reports every parse problem found; does not run the content
/// rules in `validate`. Pools inherit missing lists from the embedded pool.
pub fn load(source: &PackSource) -> Result<StoryPack, PackErrors> {
    let mut issues = Vec::new();

    let meta = match read_or_issue(source, "pack.toml", &mut issues) {
        Some(text) => parse_toml::<PackMeta>("pack.toml", &text, &mut issues),
        None => {
            if issues.is_empty() {
                issues.push(PackIssue::new("pack.toml", "file", "missing"));
            }
            None
        }
    };

    let canon = read_or_issue(source, "canon.md", &mut issues).unwrap_or_default();

    let cast = read_or_issue(source, "cast.toml", &mut issues)
        .and_then(|text| parse_toml::<CastFile>("cast.toml", &text, &mut issues))
        .map(|c| c.character)
        .unwrap_or_default();

    let mut anchors = Vec::new();
    for name in list_or_issue(source, "anchors", &mut issues) {
        if !name.ends_with(".toml") {
            continue;
        }
        let rel = format!("anchors/{name}");
        if let Some(text) = read_or_issue(source, &rel, &mut issues)
            && let Some(anchor) = parse_toml::<Anchor>(&rel, &text, &mut issues)
        {
            anchors.push(AnchorEntry { file: rel, anchor });
        }
    }

    let mut pool = read_or_issue(source, "pool.toml", &mut issues)
        .and_then(|text| parse_toml::<Pool>("pool.toml", &text, &mut issues))
        .unwrap_or_default();
    if *source != PackSource::Embedded {
        pool.inherit_from(&embedded_pool());
    }

    let mut art = BTreeMap::new();
    for name in list_or_issue(source, "art", &mut issues) {
        if name.ends_with(".txt")
            && let Some(text) = read_or_issue(source, &format!("art/{name}"), &mut issues)
        {
            art.insert(name, text);
        }
    }

    match (meta, issues.is_empty()) {
        (Some(meta), true) => Ok(StoryPack {
            meta,
            canon,
            cast,
            anchors,
            pool,
            art,
        }),
        _ => Err(PackErrors(issues)),
    }
}

/// Load a pack and run every content rule; any issue is an error.
pub fn open(source: &PackSource) -> Result<StoryPack, PackErrors> {
    let pack = load(source)?;
    let issues = validate::validate(&pack);
    if issues.is_empty() {
        Ok(pack)
    } else {
        Err(PackErrors(issues))
    }
}

/// The built-in story pack.
pub fn embedded() -> Result<StoryPack, PackErrors> {
    open(&PackSource::Embedded)
}

/// The pack a run was started with: its directory, or the embedded pack.
pub fn open_for_run(pack_dir: Option<&Path>) -> Result<StoryPack, PackErrors> {
    match pack_dir {
        Some(dir) => open(&PackSource::Dir(dir.to_path_buf())),
        None => embedded(),
    }
}
