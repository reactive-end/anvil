//! Trust tiers: how much a concept has been confirmed since it was generated.

use std::fmt;

use serde::Serialize;

use crate::frontmatter::Document;

/// Trust tier of a concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// No verification on record.
    Unverified,
    /// Confirmed by a tool, not by a person.
    MachineConfirmed,
    /// A person verified it at or after the content was generated.
    HumanReviewed,
}

impl fmt::Display for Trust {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unverified => "unverified",
            Self::MachineConfirmed => "machine-confirmed",
            Self::HumanReviewed => "human-reviewed",
        })
    }
}

/// Computes the tier. A `human:` verification only counts when it is at or after `generated.at`:
/// content regenerated after a review is unreviewed again.
pub fn trust_of(doc: &Document) -> Trust {
    let generated = doc.generated().and_then(|g| g.instant());
    let mut tier = Trust::Unverified;
    for stamp in doc.verified() {
        if !stamp.is_human() {
            tier = tier.max(Trust::MachineConfirmed);
            continue;
        }
        let counts = match (generated, stamp.instant()) {
            (Some(made), Some(checked)) => checked >= made,
            (None, Some(_)) => true,
            (_, None) => false,
        };
        if counts {
            tier = tier.max(Trust::HumanReviewed);
        }
    }
    tier
}
