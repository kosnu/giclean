//! Agent observations are an additional veto, never permission to delete.
//!
//! The aggregation API does not discover homes, read sessions, invoke Git, or
//! change the filesystem. Callers supply detectors and Git worktree roots.
//! Provider-specific discovery lives in submodules such as [`codex`].

use std::path::Path;

pub mod codex;

/// An observation about one worktree within the detector's documented scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// A live reference, including loaded-idle and approval/input waiting.
    Active,
    /// A past reference; this does not establish that the worktree is unused.
    Historical,
    /// A complete inspection found no reference in the configured scope.
    ///
    /// An empty live list, old timestamp, missing lock, or partial scan is not
    /// sufficient. This is not a claim about other homes, hosts, or agents.
    NoReference,
    /// The detector cannot determine whether the worktree is referenced.
    Unknown,
}

/// Fixed failure categories keep raw session data and OS errors out of reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionFailure {
    ReadFailed,
    InvalidData,
    UnsupportedFormat,
    IncompleteCoverage,
    PathUncertain,
    Unavailable,
}

impl DetectionFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadFailed => "read_failed",
            Self::InvalidData => "invalid_data",
            Self::UnsupportedFormat => "unsupported_format",
            Self::IncompleteCoverage => "incomplete_coverage",
            Self::PathUncertain => "path_uncertain",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Implementations must inspect metadata only, without modifying agent state.
///
/// `worktree` is an absolute root from Git's worktree inventory, not a branch
/// name or origin URL. Match paths by components and treat unresolved aliases,
/// moved paths, foreign hosts, and incomplete coverage as Unknown or an error.
/// A detector must not convert read/parse failures into NoReference. Its name
/// is a fixed provider identifier, never a session title or user content.
///
/// Each call must obtain a fresh observation. Callers must inspect during both
/// preview and execution, and again immediately before deleting each worktree;
/// an earlier Clear result cannot authorize a later deletion.
pub trait AgentDetector {
    fn name(&self) -> &'static str;
    fn detect(&self, worktree: &Path) -> Result<AgentState, DetectionFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentObservation {
    pub agent: &'static str,
    pub result: Result<AgentState, DetectionFailure>,
}

/// Aggregated agent protection, independent of Git deletion eligibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    Active,
    Unknown,
    /// All supplied detectors completed and found no reference in their scope.
    /// Existing Git, age, file, lock and pre-deletion checks still apply.
    Clear,
}

impl Protection {
    pub fn blocks_deletion(self) -> bool {
        self != Self::Clear
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    observations: Vec<AgentObservation>,
}

impl Inspection {
    pub fn observations(&self) -> &[AgentObservation] {
        &self.observations
    }

    /// Active wins over uncertainty; neither permits deletion. An empty
    /// detector set is Unknown, so missing configuration cannot clear a veto.
    pub fn protection(&self) -> Protection {
        if self
            .observations
            .iter()
            .any(|entry| entry.result == Ok(AgentState::Active))
        {
            Protection::Active
        } else if !self.observations.is_empty()
            && self
                .observations
                .iter()
                .all(|entry| entry.result == Ok(AgentState::NoReference))
        {
            Protection::Clear
        } else {
            Protection::Unknown
        }
    }
}

/// Inspect every supplied provider, retaining failures even if another is active.
/// No detector registry or implicit home-directory access is performed here.
pub fn inspect(worktree: &Path, detectors: &[&dyn AgentDetector]) -> Inspection {
    Inspection {
        observations: detectors
            .iter()
            .map(|detector| AgentObservation {
                agent: detector.name(),
                result: detector.detect(worktree),
            })
            .collect(),
    }
}
