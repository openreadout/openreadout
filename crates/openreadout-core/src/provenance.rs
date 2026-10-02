//! Honest confidence: where each normalized field's meaning came from.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How we learned what a field means. Ordered from most to least authoritative.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Source {
    /// A published specification or open standard (e.g. OME-XML, FCS 3.1).
    Spec,
    /// The vendor's own published implementation or documentation (e.g. ZEISS libCZI docs).
    VendorImpl,
    /// Permissively licensed community readers whose documentation is public (czifile, liffile, nd2).
    PriorArt,
    /// Our own differential analysis of files in the corpus.
    Inferred,
}

impl Source {
    /// One-line English description of the source, for reports.
    pub fn describe(self) -> &'static str {
        match self {
            Source::Spec => "published specification or open standard",
            Source::VendorImpl => "vendor-published implementation or documentation",
            Source::PriorArt => "permissively licensed community reader documentation",
            Source::Inferred => "inferred from corpus files by differential analysis",
        }
    }
}

/// Per-format confidence summary shown by `info` and `self formats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Confidence {
    /// Validated against an oracle across the corpus; structure from vendor docs or a spec.
    High,
    /// Validated on the corpus; some fields inferred.
    Medium,
    /// Early support; expect gaps.
    Low,
}

impl Confidence {
    /// The JSON name: `high`, `medium` or `low`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// JSON-path → source. Keys look like `images[0].physical_size.x`.
pub type ProvenanceMap = BTreeMap<String, Source>;
