//! Human-readable explanations from the same decisions used for execution.
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::decision::{Decision, Outcome, ReasonCode, RunReport, Target};

fn reason_text(reason: ReasonCode) -> &'static str {
    match reason {
        ReasonCode::Merged => "tip is an ancestor of the latest default branch",
        ReasonCode::SquashIncluded => {
            "all changed paths match at one point in default-branch history"
        }
        ReasonCode::InactiveMerged => {
            "verified PR merge and subsequent activity are older than the waiting period"
        }
        ReasonCode::ProtectedBranch => "protected branch name",
        ReasonCode::BranchInUse => "branch is checked out in a worktree",
        ReasonCode::SymbolicBranch => "symbolic branch reference",
        ReasonCode::DefaultBranch => "origin's default branch",
        ReasonCode::NotMerged => "neither ancestry nor exact changed-path inclusion was verified",
        ReasonCode::MainWorktree => "primary worktree",
        ReasonCode::UnbornHead => "worktree has an unborn HEAD",
        ReasonCode::DetachedHead => "worktree has a detached HEAD",
        ReasonCode::BareWorktree => "bare worktree",
        ReasonCode::LockedWorktree => "worktree is locked",
        ReasonCode::PrunableWorktree => "worktree is marked prunable",
        ReasonCode::CurrentWorktree => "current worktree",
        ReasonCode::GitOperation => "Git operation or lock is present",
        ReasonCode::IndexFlags => "index flags may hide local changes",
        ReasonCode::LocalChanges => "local changes or untracked/ignored files are present",
        ReasonCode::WaitingPeriod => "verified PR merge is too recent for the waiting period",
        ReasonCode::RecentActivity => "later worktree activity restarted the waiting period",
        ReasonCode::StateChanged => "state changed during inspection or before deletion",
        ReasonCode::TargetMissing => "target disappeared before deletion",
        ReasonCode::PathUnavailable => "worktree path could not be resolved safely",
        ReasonCode::NoVerifiedMerge => "no matching PR merge time with verified Git inclusion",
        ReasonCode::NotInspected => "inspection was not completed; deletion was not authorized",
        ReasonCode::InspectionFailed => "inspection failed; deletion was not authorized",
        ReasonCode::AgentActive => "agent has a live worktree reference",
        ReasonCode::AgentUnknown => "agent usage could not be ruled out safely",
    }
}

fn timestamp(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(value) => format!("{}s since Unix epoch", value.as_secs()),
        Err(value) => format!("-{}s since Unix epoch", value.duration().as_secs()),
    }
}

fn write_decision(decision: &Decision, out: &mut impl Write) -> io::Result<()> {
    let target = match &decision.target {
        Target::Branch { name, .. } => format!("branch {name:?}"),
        Target::Worktree { path, .. } => format!("worktree {:?}", path.display().to_string()),
    };
    let state = if decision.is_deletable() {
        "eligible"
    } else {
        "kept"
    };
    writeln!(
        out,
        "explain: {target}: {state} ({})",
        reason_text(decision.reason)
    )?;
    if let Some(detail) = &decision.detail {
        writeln!(out, "  detail: {detail:?}")?;
    }
    let evidence = &decision.evidence;
    for (label, time) in [
        ("verified PR merge", evidence.merged_at),
        ("latest local activity", evidence.latest_activity),
        ("eligible at or before", evidence.cutoff),
    ] {
        if let Some(time) = time {
            writeln!(out, "  {label}: {}", timestamp(time))?;
        }
    }
    for (label, time) in [
        ("since verified merge", evidence.merged_at),
        ("since latest activity", evidence.latest_activity),
    ] {
        if let Some(time) = time {
            match decision.inspected_at.duration_since(time) {
                Ok(elapsed) => writeln!(out, "  elapsed {label}: {}s", elapsed.as_secs())?,
                Err(_) => writeln!(out, "  elapsed {label}: timestamp is in the future")?,
            }
        }
    }
    Ok(())
}

pub fn write(report: &RunReport, out: &mut impl Write) -> io::Result<()> {
    for decision in &report.decisions {
        write_decision(decision, out)?;
        if let Some(result) = report
            .results
            .iter()
            .find(|result| result.target == decision.target)
        {
            let outcome = match &result.outcome {
                Outcome::NotAttempted => "not attempted",
                Outcome::Candidate => "dry-run candidate; not deleted",
                Outcome::Skipped => "skipped; not deleted",
                Outcome::Deleted => "deleted",
                Outcome::Failed(_) => "failed; see deletion result",
            };
            writeln!(out, "  execution: {outcome}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_details_cannot_inject_explanation_lines() {
        let mut decision = Decision::new(
            Target::Worktree {
                path: "/tmp/line\nnext".into(),
                branch: None,
                oid: "a".repeat(40),
            },
            ReasonCode::InspectionFailed,
        );
        decision.detail = Some("failure\nsecond line".into());
        let mut output = Vec::new();
        write_decision(&decision, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 2);
        assert!(output.contains("line\\nnext"));
        assert!(output.contains("failure\\nsecond line"));
    }
}
