//! SCAFFOLD: pack content rules; implemented by the pack-validation step.

use super::{PackIssue, model::StoryPack};

/// Every content-rule violation in `pack`. Empty when the pack is valid.
pub fn validate(_pack: &StoryPack) -> Vec<PackIssue> {
    Vec::new()
}
