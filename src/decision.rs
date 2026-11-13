//! Inspection facts shared by both cleanup commands. These are not deletion results.
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Branch {
        name: String,
        oid: String,
    },
    Worktree {
        path: PathBuf,
        branch: Option<String>,
        oid: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    Deletable,
    Protected,
    Unknown,
}

/// Codes are explicit strings, independent of Rust variant names and CLI wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonCode {
    Merged,
    SquashIncluded,
    InactiveMerged,
    ProtectedBranch,
    BranchInUse,
    SymbolicBranch,
    DefaultBranch,
    NotMerged,
    MainWorktree,
    UnbornHead,
    DetachedHead,
    BareWorktree,
    LockedWorktree,
    PrunableWorktree,
    CurrentWorktree,
    GitOperation,
    IndexFlags,
    LocalChanges,
    WaitingPeriod,
    RecentActivity,
    StateChanged,
    TargetMissing,
    PathUnavailable,
    NoVerifiedMerge,
    InspectionFailed,
    NotInspected,
    AgentActive,
    AgentUnknown,
}

impl ReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::SquashIncluded => "squash_included",
            Self::InactiveMerged => "inactive_merged",
            Self::ProtectedBranch => "protected_branch",
            Self::BranchInUse => "branch_in_use",
            Self::SymbolicBranch => "symbolic_branch",
            Self::DefaultBranch => "default_branch",
            Self::NotMerged => "not_merged",
            Self::MainWorktree => "main_worktree",
            Self::UnbornHead => "unborn_head",
            Self::DetachedHead => "detached_head",
            Self::BareWorktree => "bare_worktree",
            Self::LockedWorktree => "locked_worktree",
            Self::PrunableWorktree => "prunable_worktree",
            Self::CurrentWorktree => "current_worktree",
            Self::GitOperation => "git_operation",
            Self::IndexFlags => "index_flags",
            Self::LocalChanges => "local_changes",
            Self::WaitingPeriod => "waiting_period",
            Self::RecentActivity => "recent_activity",
            Self::StateChanged => "state_changed",
            Self::TargetMissing => "target_missing",
            Self::PathUnavailable => "path_unavailable",
            Self::NoVerifiedMerge => "no_verified_merge",
            Self::InspectionFailed => "inspection_failed",
            Self::NotInspected => "not_inspected",
            Self::AgentActive => "agent_active",
            Self::AgentUnknown => "agent_unknown",
        }
    }

    pub fn eligibility(self) -> Eligibility {
        match self {
            Self::Merged | Self::SquashIncluded | Self::InactiveMerged => Eligibility::Deletable,
            Self::StateChanged
            | Self::TargetMissing
            | Self::PathUnavailable
            | Self::NoVerifiedMerge
            | Self::InspectionFailed
            | Self::AgentUnknown
            | Self::NotInspected => Eligibility::Unknown,
            _ => Eligibility::Protected,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub merged_at: Option<SystemTime>,
    pub latest_activity: Option<SystemTime>,
    pub cutoff: Option<SystemTime>,
}

/// What execution actually did, kept separate from an inspection's eligibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    NotAttempted,
    Candidate,
    Skipped,
    Deleted,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionResult {
    pub target: Target,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub target: Target,
    pub reason: ReasonCode,
    /// Time this observation was made, not a promise that it remains valid.
    pub inspected_at: SystemTime,
    pub detail: Option<String>,
    pub evidence: Evidence,
}

impl Decision {
    pub fn new(target: Target, reason: ReasonCode) -> Self {
        Self {
            target,
            reason,
            inspected_at: SystemTime::now(),
            detail: None,
            evidence: Evidence::default(),
        }
    }

    pub fn eligibility(&self) -> Eligibility {
        self.reason.eligibility()
    }
    pub fn is_deletable(&self) -> bool {
        self.eligibility() == Eligibility::Deletable
    }

    pub fn revise(&mut self, reason: ReasonCode) {
        self.reason = reason;
        self.inspected_at = SystemTime::now();
        self.detail = None;
    }

    pub fn inspection_failed(&mut self, detail: String) {
        self.revise(ReasonCode::InspectionFailed);
        self.detail = Some(detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_preserves_partial_execution_on_error() {
        let mut report = RunReport::default();
        let deleted = Decision::new(
            Target::Branch {
                name: "done".into(),
                oid: "a".repeat(40),
            },
            ReasonCode::Merged,
        );
        let pending = Decision::new(
            Target::Branch {
                name: "pending".into(),
                oid: "b".repeat(40),
            },
            ReasonCode::NotInspected,
        );
        report.record(deleted.clone());
        report.record(pending);
        record_outcome(&mut report.results, &deleted.target, Outcome::Deleted);
        report.finish(&Err("inspection failed".into()));
        assert_eq!(report.results[0].outcome, Outcome::Deleted);
        assert_eq!(report.results[1].outcome, Outcome::NotAttempted);
        assert_eq!(report.decisions[0].reason, ReasonCode::Merged);
        assert_eq!(report.decisions[1].reason, ReasonCode::InspectionFailed);
        assert_eq!(report.error.as_deref(), Some("inspection failed"));
    }

    #[test]
    fn revisions_preserve_target_and_evidence_not_stale_details() {
        let mut d = Decision::new(
            Target::Branch {
                name: "topic".into(),
                oid: "a".repeat(40),
            },
            ReasonCode::Merged,
        );
        let target = d.target.clone();
        d.evidence.cutoff = Some(SystemTime::UNIX_EPOCH);
        d.detail = Some("old error".into());
        d.revise(ReasonCode::StateChanged);
        assert_eq!(d.target, target);
        assert_eq!(d.evidence.cutoff, Some(SystemTime::UNIX_EPOCH));
        assert_eq!(d.detail, None);
        assert!(!d.is_deletable());
    }

    #[test]
    fn unknown_is_never_a_deletion_candidate() {
        for reason in [
            ReasonCode::StateChanged,
            ReasonCode::TargetMissing,
            ReasonCode::PathUnavailable,
            ReasonCode::NoVerifiedMerge,
            ReasonCode::InspectionFailed,
        ] {
            let decision = Decision::new(
                Target::Branch {
                    name: "topic".into(),
                    oid: "a".repeat(40),
                },
                reason,
            );
            assert_eq!(decision.eligibility(), Eligibility::Unknown);
            assert!(!decision.is_deletable());
        }
    }

    #[test]
    fn inspection_failures_replace_stale_eligibility() {
        let mut decision = Decision::new(
            Target::Worktree {
                path: "/tmp/topic".into(),
                branch: Some("topic".into()),
                oid: "b".repeat(40),
            },
            ReasonCode::InactiveMerged,
        );
        let before = decision.inspected_at;
        assert!(decision.is_deletable());
        decision.inspection_failed("query failed".into());
        assert!(!decision.is_deletable());
        assert_eq!(decision.reason.as_str(), "inspection_failed");
        assert_eq!(decision.detail.as_deref(), Some("query failed"));
        assert!(decision.inspected_at >= before);
    }
}

/// Retained by the caller even when inspection/execution returns an error.
#[derive(Clone, Debug, Default)]
pub struct RunReport {
    pub decisions: Vec<Decision>,
    pub results: Vec<ExecutionResult>,
    pub error: Option<String>,
}

impl RunReport {
    pub fn record(&mut self, decision: Decision) {
        if let Some(old) = self
            .decisions
            .iter_mut()
            .find(|old| old.target == decision.target)
        {
            *old = decision;
        } else {
            self.results.push(ExecutionResult {
                target: decision.target.clone(),
                outcome: Outcome::NotAttempted,
            });
            self.decisions.push(decision);
        }
    }

    pub fn finish(&mut self, result: &Result<std::process::ExitCode, String>) {
        if let Err(error) = result {
            self.error = Some(error.clone());
            for decision in &mut self.decisions {
                if decision.reason == ReasonCode::NotInspected {
                    decision.inspection_failed(error.clone());
                }
            }
        }
    }
}

pub fn record_outcome(results: &mut [ExecutionResult], target: &Target, outcome: Outcome) {
    let result = results
        .iter_mut()
        .find(|result| &result.target == target)
        .expect("inspected target has execution result");
    result.outcome = outcome;
}
