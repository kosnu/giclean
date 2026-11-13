//! Public API contract tests using synthetic observations only.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use giclean::agents::{AgentDetector, AgentState, DetectionFailure, Protection, inspect};

struct FixtureDetector {
    name: &'static str,
    result: Cell<Result<AgentState, DetectionFailure>>,
    inspected: RefCell<Vec<PathBuf>>,
}

impl FixtureDetector {
    fn new(name: &'static str, result: Result<AgentState, DetectionFailure>) -> Self {
        Self {
            name,
            result: Cell::new(result),
            inspected: RefCell::new(Vec::new()),
        }
    }
}

impl AgentDetector for FixtureDetector {
    fn name(&self) -> &'static str {
        self.name
    }

    fn detect(&self, worktree: &Path) -> Result<AgentState, DetectionFailure> {
        self.inspected.borrow_mut().push(worktree.to_path_buf());
        self.result.get()
    }
}

#[test]
fn only_complete_no_reference_clears_the_agent_guard() {
    for (state, expected) in [
        (AgentState::Active, Protection::Active),
        (AgentState::Historical, Protection::Unknown),
        (AgentState::Unknown, Protection::Unknown),
        (AgentState::NoReference, Protection::Clear),
    ] {
        let detector = FixtureDetector::new("fixture", Ok(state));
        let result = inspect(Path::new("/synthetic/worktree"), &[&detector]);
        assert_eq!(result.protection(), expected);
        assert_eq!(expected.blocks_deletion(), state != AgentState::NoReference);
        assert_eq!(result.observations()[0].result, Ok(state));
    }
    let empty = inspect(Path::new("/synthetic/worktree"), &[]);
    assert_eq!(empty.protection(), Protection::Unknown);
    assert!(empty.protection().blocks_deletion());
}

#[test]
fn every_failure_is_retained_and_blocks_even_with_a_clear_provider() {
    let clear = FixtureDetector::new("complete", Ok(AgentState::NoReference));
    for (failure, code) in [
        (DetectionFailure::ReadFailed, "read_failed"),
        (DetectionFailure::InvalidData, "invalid_data"),
        (DetectionFailure::UnsupportedFormat, "unsupported_format"),
        (DetectionFailure::IncompleteCoverage, "incomplete_coverage"),
        (DetectionFailure::PathUncertain, "path_uncertain"),
        (DetectionFailure::Unavailable, "unavailable"),
    ] {
        let failed = FixtureDetector::new("failed", Err(failure));
        for detectors in [
            [&clear as &dyn AgentDetector, &failed],
            [&failed as &dyn AgentDetector, &clear],
        ] {
            let result = inspect(Path::new("/synthetic/worktree"), &detectors);
            assert_eq!(result.protection(), Protection::Unknown);
            assert!(result.protection().blocks_deletion());
            assert!(
                result
                    .observations()
                    .iter()
                    .any(|entry| { entry.agent == "failed" && entry.result == Err(failure) })
            );
        }
        assert_eq!(failure.as_str(), code);
    }
}

#[test]
fn mixed_providers_preserve_vetoes_independent_of_order() {
    use AgentState::{Active, Historical, NoReference, Unknown};
    for (left, right, expected) in [
        (NoReference, NoReference, Protection::Clear),
        (NoReference, Historical, Protection::Unknown),
        (NoReference, Unknown, Protection::Unknown),
        (Historical, Unknown, Protection::Unknown),
        (Historical, Historical, Protection::Unknown),
        (Unknown, Unknown, Protection::Unknown),
        (Active, NoReference, Protection::Active),
        (Active, Historical, Protection::Active),
        (Active, Unknown, Protection::Active),
        (Active, Active, Protection::Active),
    ] {
        let left = FixtureDetector::new("left", Ok(left));
        let right = FixtureDetector::new("right", Ok(right));
        for detectors in [
            [&left as &dyn AgentDetector, &right],
            [&right as &dyn AgentDetector, &left],
        ] {
            let result = inspect(Path::new("/synthetic/worktree"), &detectors);
            assert_eq!(result.protection(), expected);
            assert_eq!(result.observations().len(), 2);
        }
    }
}

#[test]
fn active_does_not_hide_another_providers_failure_or_skip_inspection() {
    let active = FixtureDetector::new("active", Ok(AgentState::Active));
    let failed = FixtureDetector::new("failed", Err(DetectionFailure::ReadFailed));
    let root = Path::new("/synthetic/日本語 worktree");
    let result = inspect(root, &[&active, &failed]);
    assert_eq!(result.protection(), Protection::Active);
    assert!(result.protection().blocks_deletion());
    assert_eq!(
        result.observations()[1].result,
        Err(DetectionFailure::ReadFailed)
    );
    assert_eq!(*active.inspected.borrow(), [root]);
    assert_eq!(*failed.inspected.borrow(), [root]);
}

#[test]
fn reinspection_calls_detectors_again_and_keeps_prior_observations() {
    let detector = FixtureDetector::new("changing", Ok(AgentState::NoReference));
    let root = Path::new("/synthetic/worktree");
    let preview = inspect(root, &[&detector]);
    assert_eq!(preview.protection(), Protection::Clear);
    for next in [
        Ok(AgentState::Active),
        Ok(AgentState::Historical),
        Ok(AgentState::Unknown),
        Err(DetectionFailure::ReadFailed),
    ] {
        detector.result.set(next);
        let before_delete = inspect(root, &[&detector]);
        assert!(before_delete.protection().blocks_deletion());
        assert_eq!(before_delete.observations()[0].result, next);
        assert_eq!(preview.protection(), Protection::Clear);
    }
    assert_eq!(detector.inspected.borrow().len(), 5);
}

#[test]
fn worktree_roots_are_passed_through_without_branch_or_url_matching() {
    let detector = FixtureDetector::new("recording", Ok(AgentState::Unknown));
    for root in [
        "/synthetic/repo",
        "/synthetic/repo-old",
        "/synthetic/repo-wt",
    ] {
        assert!(
            inspect(Path::new(root), &[&detector])
                .protection()
                .blocks_deletion()
        );
    }
    assert_eq!(
        *detector.inspected.borrow(),
        [
            "/synthetic/repo",
            "/synthetic/repo-old",
            "/synthetic/repo-wt"
        ]
        .map(PathBuf::from)
    );
}
