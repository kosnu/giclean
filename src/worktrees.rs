use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::decision::{Decision, Outcome, ReasonCode, RunReport, Target, record_outcome};
use crate::{Action, git, git_command_text, git_text, progress::Progress, valid_oid};

pub const HELP: &str = "giclean wt — Delete inactive merged worktrees

Usage: giclean wt (-d <days> | -h <hours>) [--dry-run]

Options:
  -d, --days <days>    Wait this many days after merge or later activity
  -h, --hours <hours>  Wait this many hours after merge or later activity
  --dry-run           Show deletion candidates only
  --help              Show help (-h means hours for wt)

Requires a GitHub origin and authenticated GitHub CLI (gh).
Uses PR merge time, verifies inclusion with Git, and keeps branches.
Protects main/current/locked worktrees and worktrees with local files or changes.
Deletes without confirmation. Does not fetch automatically.
";

pub fn parse_args(args: &[OsString]) -> Result<Action, String> {
    if args == ["--help"] {
        return Ok(Action::WorktreesHelp);
    }
    let mut age = None;
    let mut dry_run = false;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        if option == "--dry-run" && !dry_run {
            dry_run = true;
            continue;
        }
        let unit = match option.to_str() {
            Some("-d" | "--days") => 86400,
            Some("-h" | "--hours") => 3600,
            _ => return Err("use wt (-d <days> | -h <hours>) [--dry-run]".into()),
        };
        if age.is_some() {
            return Err("specify exactly one of --days or --hours".into());
        }
        let seconds = args
            .next()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .and_then(|value| value.checked_mul(unit))
            .ok_or("days/hours must be a positive integer within range")?;
        age = Some(Duration::from_secs(seconds));
    }
    Ok(Action::Worktrees {
        age: age.ok_or("specify --days or --hours")?,
        dry_run,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Worktree {
    path: PathBuf,
    oid: String,
    branch: Option<String>,
    protection: Option<ReasonCode>,
}

fn snapshot(progress: &Progress) -> Result<Vec<Worktree>, String> {
    let output = git_text(&["worktree", "list", "--porcelain", "-z"], progress)?;
    let mut worktrees = Vec::new();
    let mut fields = output.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() && fields.clone().next().is_none() {
            break;
        }
        let path = field
            .strip_prefix("worktree ")
            .filter(|path| Path::new(path).is_absolute())
            .ok_or("cannot parse worktree path")?;
        let mut worktree = Worktree {
            path: PathBuf::from(path),
            oid: String::new(),
            branch: None,
            protection: worktrees.is_empty().then_some(ReasonCode::MainWorktree),
        };
        let mut ended = false;
        for field in fields.by_ref() {
            if field.is_empty() {
                ended = true;
                break;
            }
            if let Some(oid) = field.strip_prefix("HEAD ") {
                let unborn = matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b == b'0');
                if !worktree.oid.is_empty() || (!valid_oid(oid) && !unborn) {
                    return Err("cannot parse worktree HEAD".into());
                }
                if unborn {
                    worktree.protection.get_or_insert(ReasonCode::UnbornHead);
                }
                worktree.oid = oid.into();
            } else if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                if branch.is_empty() || worktree.branch.replace(branch.into()).is_some() {
                    return Err("cannot parse worktree branch".into());
                }
            } else if matches!(field, "detached" | "bare" | "locked" | "prunable")
                || field.starts_with("locked ")
                || field.starts_with("prunable ")
            {
                let reason = if field == "detached" {
                    ReasonCode::DetachedHead
                } else if field == "bare" {
                    ReasonCode::BareWorktree
                } else if field.starts_with("locked") {
                    ReasonCode::LockedWorktree
                } else {
                    ReasonCode::PrunableWorktree
                };
                worktree.protection.get_or_insert(reason);
            } else {
                return Err("cannot parse worktree state".into());
            }
        }
        if !ended || (worktree.oid.is_empty() && worktree.protection.is_none()) {
            return Err("incomplete worktree state".into());
        }
        worktrees.push(worktree);
    }
    if worktrees.is_empty() {
        return Err("cannot find any worktrees".into());
    }
    Ok(worktrees)
}

// Only a literal GitHub origin is accepted; GH_REPO/default gh repository settings
// must never redirect merge-time lookups to another repository.
fn github_repo(url: &str) -> Result<String, String> {
    let path = [
        "git@github.com:",
        "https://github.com/",
        "ssh://git@github.com/",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix))
    .ok_or("wt requires origin to use a github.com SSH or HTTPS URL")?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    {
        return Err("cannot determine GitHub repository from origin".into());
    }
    Ok(path.to_owned())
}

fn ancestor(oid: &str, target: &str, progress: &Progress) -> Result<bool, String> {
    let output = progress
        .output(&mut git(&["merge-base", "--is-ancestor", oid, target]))
        .map_err(|e| format!("could not check commit ancestry: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err("cannot check commit ancestry; run git fetch origin and retry".into()),
    }
}

fn merge_time(
    worktree: &Worktree,
    repo: &str,
    remote: &crate::RemoteHead,
    progress: &Progress,
) -> Result<Option<SystemTime>, String> {
    let mut command = Command::new("gh");
    command
        .args([
            "api",
            "--hostname",
            "github.com",
            "--method",
            "GET",
            "--paginate",
            &format!("repos/{repo}/commits/{}/pulls?per_page=100", worktree.oid),
            "--jq",
            ".[] | select(.merged_at != null) | [(.merged_at | fromdateiso8601), .head.sha, .head.ref, .base.ref, .base.repo.full_name, .merge_commit_sha] | @tsv",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .stderr(Stdio::inherit());
    let output = progress
        .output(&mut command)
        .map_err(|e| format!("could not run gh; install and authenticate GitHub CLI: {e}"))?;
    if !output.status.success() {
        return Err(
            "could not read PR merge times using gh; check authentication and repository access"
                .into(),
        );
    }
    let output = String::from_utf8(output.stdout).map_err(|_| "cannot parse PR merge times")?;
    let mut latest = None;
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 6 || !valid_oid(fields[1]) || !valid_oid(fields[5]) {
            return Err("cannot parse PR merge information".into());
        }
        let timestamp = fields[0]
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .ok_or("cannot parse PR merge timestamp")?;
        if fields[1] != worktree.oid
            || Some(fields[2]) != worktree.branch.as_deref()
            || fields[3] != remote.branch
            || !fields[4].eq_ignore_ascii_case(repo)
            || !ancestor(fields[5], &remote.oid, progress)?
        {
            continue;
        }
        // PR data supplies a timestamp, not proof of inclusion. Independently
        // verify ancestry or exact changes against this immutable merge commit.
        if ancestor(&worktree.oid, fields[5], progress)?
            || crate::squash_included(&[&worktree.oid], fields[5], progress)?
                .contains(&worktree.oid)
        {
            latest = Some(latest.map_or(timestamp, |old: SystemTime| old.max(timestamp)));
        }
    }
    Ok(latest)
}

fn at(path: &Path, args: &[&str]) -> Command {
    let mut command = git(args);
    command.current_dir(path).env("GIT_OPTIONAL_LOCKS", "0");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
    ] {
        command.env_remove(key);
    }
    command
}

fn worktree_safety(path: &Path, progress: &Progress) -> Result<Option<ReasonCode>, String> {
    let git_dir = private_git_dir(path, progress)?;
    for marker in [
        "MERGE_HEAD",
        "REBASE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
        "index.lock",
        "HEAD.lock",
    ] {
        match fs::symlink_metadata(git_dir.join(marker)) {
            Ok(_) => return Ok(Some(ReasonCode::GitOperation)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect Git operation state: {e}")),
        }
    }
    // An empty status is not proof of a clean worktree when index flags suppress
    // change detection. Preserve those worktrees without modifying their index.
    let indexed = progress
        .output(&mut at(path, &["ls-files", "--cached", "-v", "-z"]))
        .map_err(|e| format!("could not inspect worktree index flags: {e}"))?;
    if !indexed.status.success() {
        return Err("could not inspect worktree index flags".into());
    }
    let mut inspectable = true;
    for entry in indexed.stdout.split_inclusive(|byte| *byte == 0) {
        if entry.len() < 4 || entry[1] != b' ' || entry.last() != Some(&0) {
            return Err("cannot parse worktree index flags".into());
        }
        // --cached -v reports H for ordinary entries, S for skip-worktree,
        // and M for unmerged entries; lowercase means assume-unchanged.
        match entry[0] {
            b'H' => {}
            b'h' | b'S' | b's' | b'M' | b'm' => inspectable = false,
            _ => return Err("cannot parse worktree index flags".into()),
        }
    }
    if !inspectable {
        return Ok(Some(ReasonCode::IndexFlags));
    }
    // Git normally permits deleting ignored files. Include them here so no
    // local-only files are discarded, even when their timestamps are old.
    let status = git_command_text(
        at(
            path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
                "--ignore-submodules=none",
            ],
        ),
        "status",
        progress,
    )?;
    Ok((!status.is_empty()).then_some(ReasonCode::LocalChanges))
}

fn modified(path: &Path) -> Result<SystemTime, String> {
    fs::symlink_metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|e| format!("cannot read activity time for {}: {e}", path.display()))
}

fn private_git_dir(path: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let git_dir = git_command_text(
        at(path, &["rev-parse", "--absolute-git-dir"]),
        "rev-parse",
        progress,
    )?;
    let path = PathBuf::from(
        git_dir
            .strip_suffix('\n')
            .ok_or("cannot parse Git directory")?,
    );
    if !path.is_absolute() {
        return Err("Git directory is not absolute".into());
    }
    Ok(path)
}

fn latest_modified(path: &Path, skip_git: bool) -> Result<SystemTime, String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| format!("cannot inspect {}: {e}", path.display()))?
        .is_dir()
    {
        return Err(format!("expected a directory at {}", path.display()));
    }
    let mut latest = UNIX_EPOCH;
    let mut pending = vec![path.to_path_buf()];
    while let Some(entry) = pending.pop() {
        let metadata = fs::symlink_metadata(&entry)
            .map_err(|e| format!("cannot inspect {}: {e}", entry.display()))?;
        latest = latest.max(modified(&entry)?);
        if metadata.is_dir() {
            for child in fs::read_dir(&entry)
                .map_err(|e| format!("cannot inspect {}: {e}", entry.display()))?
            {
                let child = child.map_err(|e| e.to_string())?.path();
                if !skip_git || entry != path || child.file_name().is_none_or(|name| name != ".git")
                {
                    pending.push(child);
                }
            }
        }
    }
    Ok(latest)
}

fn activity(path: &Path, progress: &Progress) -> Result<SystemTime, String> {
    let git_dir = private_git_dir(path, progress)?;
    Ok(latest_modified(path, true)?
        .max(latest_modified(&git_dir, false)?)
        .max(modified(&path.join(".git"))?))
}

struct WorktreeInspection {
    entries: Vec<(Worktree, Decision)>,
    cutoff: Option<SystemTime>,
}

fn inspect_worktrees(
    age: Duration,
    current: &Path,
    progress: &Progress,
    report: &mut RunReport,
) -> Result<WorktreeInspection, String> {
    let mut entries: Vec<_> = snapshot(progress)?
        .into_iter()
        .map(|tree| {
            let reason = if let Some(reason) = tree.protection {
                reason
            } else if tree
                .branch
                .as_deref()
                .is_none_or(|branch| matches!(branch, "main" | "master" | "production"))
            {
                ReasonCode::ProtectedBranch
            } else {
                match fs::canonicalize(&tree.path) {
                    Ok(path) if path == current => ReasonCode::CurrentWorktree,
                    Ok(_) => ReasonCode::NotInspected,
                    Err(_) => ReasonCode::PathUnavailable,
                }
            };
            let decision = Decision::new(
                Target::Worktree {
                    path: tree.path.clone(),
                    branch: tree.branch.clone(),
                    oid: tree.oid.clone(),
                },
                reason,
            );
            (tree, decision)
        })
        .collect();
    for (_, decision) in &entries {
        report.record(decision.clone());
    }
    if !entries
        .iter()
        .any(|(_, d)| d.reason == ReasonCode::NotInspected)
    {
        return Ok(WorktreeInspection {
            entries,
            cutoff: None,
        });
    }
    let url = git_text(&["config", "--get", "remote.origin.url"], progress)?;
    let repo = github_repo(url.trim())?;
    let remote = crate::latest_remote_head(progress)?;
    let cutoff = SystemTime::now()
        .checked_sub(age)
        .ok_or("requested waiting period exceeds the system clock range")?;
    progress.update("Checking PR merge times and worktree activity...");
    let branches = crate::branch_snapshot(None, None, progress)?;
    // Finish all initial eligibility checks before deleting anything.
    for (tree, decision) in &mut entries {
        if decision.reason != ReasonCode::NotInspected {
            continue;
        }
        decision.evidence.cutoff = Some(cutoff);
        let result = inspect_worktree(tree, &branches, &repo, &remote, cutoff, progress, decision);
        match result {
            Ok(reason) => decision.revise(reason),
            Err(error) => {
                decision.inspection_failed(error.clone());
                report.record(decision.clone());
                return Err(error);
            }
        }
        report.record(decision.clone());
    }
    Ok(WorktreeInspection {
        entries,
        cutoff: Some(cutoff),
    })
}

fn inspect_worktree(
    tree: &Worktree,
    branches: &std::collections::BTreeMap<String, crate::Branch>,
    repo: &str,
    remote: &crate::RemoteHead,
    cutoff: SystemTime,
    progress: &Progress,
    decision: &mut Decision,
) -> Result<ReasonCode, String> {
    let branch = tree
        .branch
        .as_deref()
        .expect("unprotected worktree has a branch");
    if branch == remote.branch {
        return Ok(ReasonCode::DefaultBranch);
    }
    let Some(branch) = branches.get(branch) else {
        return Ok(ReasonCode::TargetMissing);
    };
    if branch.symbolic {
        return Ok(ReasonCode::SymbolicBranch);
    }
    if branch.oid != tree.oid {
        return Ok(ReasonCode::StateChanged);
    }
    if let Some(reason) = worktree_safety(&tree.path, progress)? {
        return Ok(reason);
    }
    let merged_at = merge_time(tree, repo, remote, progress)?;
    decision.evidence.merged_at = merged_at;
    let Some(merged_at) = merged_at else {
        return Ok(ReasonCode::NoVerifiedMerge);
    };
    if merged_at > cutoff {
        return Ok(ReasonCode::WaitingPeriod);
    }
    let latest_activity = activity(&tree.path, progress)?;
    decision.evidence.latest_activity = Some(latest_activity);
    if latest_activity > cutoff {
        return Ok(ReasonCode::RecentActivity);
    }
    Ok(ReasonCode::InactiveMerged)
}

pub fn clean(
    age: Duration,
    dry_run: bool,
    out: &mut impl Write,
    report: &mut RunReport,
) -> Result<ExitCode, String> {
    let result = execute(age, dry_run, out, report);
    report.finish(&result);
    result
}

fn execute(
    age: Duration,
    dry_run: bool,
    out: &mut impl Write,
    report: &mut RunReport,
) -> Result<ExitCode, String> {
    let progress = Progress::start("Checking worktrees...");
    if git_text(&["rev-parse", "--is-inside-work-tree"], &progress)?.trim() != "true" {
        return Err("run this command inside a Git working tree".into());
    }
    let current = git_text(&["rev-parse", "--show-toplevel"], &progress)?;
    let current = fs::canonicalize(
        current
            .strip_suffix('\n')
            .ok_or("cannot find current worktree")?,
    )
    .map_err(|e| e.to_string())?;
    let inspection = inspect_worktrees(age, &current, &progress, report)?;
    let cutoff = inspection.cutoff;
    if cutoff.is_none() {
        progress.finish();
        return Ok(ExitCode::SUCCESS);
    }
    progress.update(if dry_run {
        "Checking deletion candidates..."
    } else {
        "Deleting worktrees..."
    });
    let mut failed = false;
    for (tree, initial_decision) in &inspection.entries {
        let decision = report
            .decisions
            .iter_mut()
            .find(|d| d.target == initial_decision.target)
            .expect("inspected worktree has a decision");
        if !decision.is_deletable() {
            continue;
        }
        let recheck: Result<Option<ReasonCode>, String> = (|| {
            let latest = snapshot(&progress)?;
            if !latest.iter().any(|current| current == tree) {
                return Ok(Some(ReasonCode::StateChanged));
            }
            if let Some(reason) = worktree_safety(&tree.path, &progress)? {
                return Ok(Some(reason));
            }
            let latest_activity = activity(&tree.path, &progress)?;
            decision.evidence.latest_activity = Some(latest_activity);
            Ok((Some(latest_activity) > cutoff).then_some(ReasonCode::RecentActivity))
        })();
        match recheck {
            Ok(Some(reason)) => {
                decision.revise(reason);
                record_outcome(&mut report.results, &decision.target, Outcome::Skipped);
                continue;
            }
            Ok(None) => decision.revise(decision.reason),
            Err(error) => {
                decision.inspection_failed(error.clone());
                return Err(error);
            }
        }
        progress.suspend();
        if dry_run {
            record_outcome(&mut report.results, &decision.target, Outcome::Candidate);
            writeln!(out, "{}", crate::render::candidate(tree.path.display()))
                .map_err(|e| e.to_string())?;
            continue;
        }
        let output = git(&["worktree", "remove", "--"])
            .arg(&tree.path)
            .stderr(Stdio::piped())
            .output();
        match output {
            Ok(output) if output.status.success() => {
                record_outcome(&mut report.results, &decision.target, Outcome::Deleted);
                writeln!(out, "{}", crate::render::deleted(tree.path.display()))
                    .map_err(|e| e.to_string())?;
            }
            result => {
                failed = true;
                let reason = match result {
                    Ok(output) => format!(
                        "{} ({})",
                        String::from_utf8_lossy(&output.stderr).trim(),
                        output.status
                    ),
                    Err(error) => format!("could not run git: {error}"),
                };
                record_outcome(
                    &mut report.results,
                    &decision.target,
                    Outcome::Failed(reason.clone()),
                );
                writeln!(
                    out,
                    "{}",
                    crate::render::failed(tree.path.display(), reason)
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    progress.finish();
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_are_literal_repository_names() {
        for url in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(github_repo(url).unwrap(), "owner/repo");
        }
        for url in [
            "/tmp/repo",
            "https://elsewhere/owner/repo",
            "https://github.com/owner/repo/more",
            "https://github.com/../repo",
            "https://github.com/owner/repo?x=y",
        ] {
            assert!(github_repo(url).is_err(), "{url}");
        }
    }
}
