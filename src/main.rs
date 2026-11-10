use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command, ExitCode, Output, Stdio};

mod bulk;
pub mod decision;
mod explain;
mod git;
mod progress;
mod render;
mod ssh;
#[cfg(unix)]
mod terminal;
mod worktrees;

use decision::{Decision, Outcome, ReasonCode, RunReport, Target, record_outcome};
use progress::Progress;

const HELP: &str = "giclean — Git cleanup

Usage: giclean <command> [options]

Commands:
  br             Delete merged local branches
  wt             Delete inactive merged worktrees

Options:
  -h, --help     Show help
  -V, --version  Show version

Run giclean <command> --help for command usage.
";

const BRANCHES_HELP: &str = "giclean br — Delete merged local branches

Usage: giclean br [--dry-run] [--explain]

Delete local branches merged into origin's latest default branch.
Includes squash changes when all changed paths match in content and permissions
at one point in the default branch's first-parent history.

Options:
  --dry-run   Show deletion candidates only
  --explain   Explain decisions, including kept branches
  -h, --help  Show help

Deletes without confirmation. Use --dry-run --explain to preview with reasons.
Does not fetch automatically. GitHub CLI is not required.
";

const BRANCH_FORMAT: &str = "--format=%(refname)%09%(objectname)%09%(HEAD)%09%(if)%(worktreepath)%(then)used%(else)free%(end)%09%(symref)";

enum Action {
    Branches {
        dry_run: bool,
        explain: bool,
    },
    Worktrees {
        age: std::time::Duration,
        dry_run: bool,
        explain: bool,
    },
    Help,
    BranchesHelp,
    WorktreesHelp,
    Version,
}

#[derive(Clone, Debug)]
struct Branch {
    name: String,
    oid: String,
    in_use: bool,
    symbolic: bool,
}

impl Branch {
    fn protection(&self) -> Option<ReasonCode> {
        if self.in_use {
            Some(ReasonCode::BranchInUse)
        } else if self.symbolic {
            Some(ReasonCode::SymbolicBranch)
        } else if matches!(self.name.as_str(), "main" | "master" | "production") {
            Some(ReasonCode::ProtectedBranch)
        } else {
            None
        }
    }

    fn protected(&self) -> bool {
        self.protection().is_some()
    }

    fn decision(&self, reason: ReasonCode) -> Decision {
        Decision::new(
            Target::Branch {
                name: self.name.clone(),
                oid: self.oid.clone(),
            },
            reason,
        )
    }
}

struct RemoteHead {
    branch: String,
    oid: String,
}

fn main() -> ExitCode {
    if let Some(status) = bulk::hook() {
        return status;
    }
    let action = match parse_args(env::args_os().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("giclean: {error}\nUsage: giclean <command> [options]");
            return ExitCode::from(2);
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut report = RunReport::default();
    let show_explanation = matches!(
        &action,
        Action::Branches { explain: true, .. } | Action::Worktrees { explain: true, .. }
    );
    let result = match action {
        Action::Help => write!(out, "{HELP}")
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string()),
        Action::BranchesHelp => write!(out, "{BRANCHES_HELP}")
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string()),
        Action::WorktreesHelp => write!(out, "{}", worktrees::HELP)
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string()),
        Action::Version => writeln!(out, "giclean {}", env!("CARGO_PKG_VERSION"))
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string()),
        Action::Branches { dry_run, .. } => clean_branches(dry_run, &mut out, &mut report),
        Action::Worktrees { age, dry_run, .. } => {
            worktrees::clean(age, dry_run, &mut out, &mut report)
        }
    };
    let result = if show_explanation {
        match explain::write(&report, &mut out) {
            Ok(()) => result,
            Err(error) => Err(error.to_string()),
        }
    } else {
        result
    };
    match result.and_then(|status| out.flush().map(|()| status).map_err(|e| e.to_string())) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("giclean: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Action, String> {
    let args: Vec<_> = args.collect();
    match args.as_slice() {
        [] => Ok(Action::Help),
        [arg] if arg == "--help" || arg == "-h" => Ok(Action::Help),
        [arg] if arg == "--version" || arg == "-V" => Ok(Action::Version),
        [command, option] if command == "br" && (option == "--help" || option == "-h") => {
            Ok(Action::BranchesHelp)
        }
        [command, options @ ..] if command == "br" => {
            let mut dry_run = false;
            let mut explain = false;
            for option in options {
                if option == "--dry-run" && !dry_run {
                    dry_run = true;
                } else if option == "--explain" && !explain {
                    explain = true;
                } else {
                    return Err("use br [--dry-run] [--explain]".into());
                }
            }
            Ok(Action::Branches { dry_run, explain })
        }
        [command, options @ ..] if command == "wt" => worktrees::parse_args(options),
        _ => Err(format!(
            "invalid arguments: {}",
            args.iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

fn git(args: &[&str]) -> Command {
    let mut command = git::command();
    command
        // Unsupported Git versions fail before doing any work, instead of silently
        // ignoring the no-lazy-fetch environment variable.
        .arg("--no-lazy-fetch")
        .args(args)
        // Ancestry must reflect the real commits, not local replacement objects.
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("LC_ALL", "C")
        .stderr(Stdio::inherit());
    command
}

fn git_text(args: &[&str], progress: &Progress) -> Result<String, String> {
    git_command_text(git(args), args[0], progress)
}

fn git_command_text(
    mut command: Command,
    operation: &str,
    progress: &Progress,
) -> Result<String, String> {
    let output = if operation == "ls-remote" {
        progress.remote_output(&mut command)
    } else {
        progress.output(&mut command)
    }
    .map_err(|e| format!("could not run git {operation}: {e}"))?;
    if !output.status.success() {
        return Err(format!("git {operation} failed ({})", output.status));
    }
    String::from_utf8(output.stdout).map_err(|_| "Git output is not valid UTF-8".into())
}

type TreeChanges = BTreeMap<Vec<u8>, Vec<u8>>;

// Compare tree objects in one Git process. Each response must have the requested
// pair header, followed by raw NUL records; filenames are never decoded as text.
fn tree_changes(
    pairs: &BTreeSet<(String, String)>,
    progress: &Progress,
) -> Result<BTreeMap<(String, String), TreeChanges>, String> {
    let input: String = pairs
        .iter()
        .map(|(base, tip)| format!("{base} {tip}\n"))
        .collect();
    let mut command = git(&[
        "diff-tree",
        "--stdin",
        "--raw",
        "-r",
        "-z",
        "--no-abbrev",
        "--no-renames",
        "--no-relative",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
    ]);
    let output = progress
        .output_with_input(&mut command, input.as_bytes())
        .map_err(|e| format!("could not run git diff-tree: {e}"))?;
    if !output.status.success() {
        return Err(format!("git diff-tree failed ({})", output.status));
    }
    let error = "cannot parse tree changes";
    let mut remaining = output.stdout.as_slice();
    let mut results = BTreeMap::new();
    for pair in pairs {
        let header = format!("{} {}\n", pair.0, pair.1);
        remaining = remaining.strip_prefix(header.as_bytes()).ok_or(error)?;
        let mut changes = BTreeMap::new();
        while remaining.starts_with(b":") {
            let mut fields = remaining.splitn(3, |byte| *byte == 0);
            let change = fields.next().ok_or(error)?;
            let path = fields.next().filter(|path| !path.is_empty()).ok_or(error)?;
            remaining = fields.next().ok_or(error)?;
            if changes.insert(path.to_vec(), change.to_vec()).is_some() {
                return Err(error.into());
            }
        }
        results.insert(pair.clone(), changes);
    }
    if !remaining.is_empty() {
        return Err(error.into());
    }
    Ok(results)
}

fn reachable_objects(target: &str, progress: &Progress) -> Result<HashSet<String>, String> {
    let output = git_text(
        &["rev-list", "--objects", "--no-object-names", target, "--"],
        progress,
    )?;
    let mut lines = output.lines().peekable();
    if lines.peek().copied() != Some(target) {
        return Err("cannot parse reachable object IDs".into());
    }
    lines
        .map(|oid| {
            if valid_oid(oid) {
                Ok(oid.to_owned())
            } else {
                Err("cannot parse reachable object IDs".into())
            }
        })
        .collect()
}

fn squash_included(
    tips: &[&str],
    target: &str,
    progress: &Progress,
) -> Result<BTreeSet<String>, String> {
    // Bound argv size and the number of tree diffs retained at once. All reuse is
    // scoped to this run and immutable object IDs, never branch names or refs.
    const BATCH_SIZE: usize = 128;
    let error = "cannot parse merge bases";
    let mut bases = BTreeMap::new();
    for chunk in tips.chunks(BATCH_SIZE) {
        // A...B expands to B, A, and every merge-base prefixed by ^. Git performs
        // the same merge-base --all calculation, without one process per branch.
        let mut command = git(&["rev-parse", "--revs-only", "--end-of-options"]);
        command.args(chunk.iter().map(|tip| format!("{tip}...{target}")));
        let output = git_command_text(command, "rev-parse", progress)?;
        let mut lines = output.lines().peekable();
        for tip in chunk {
            if lines.next() != Some(target) || lines.next() != Some(*tip) {
                return Err(error.into());
            }
            let mut ancestors = Vec::new();
            while let Some(base) = lines.peek().and_then(|line| line.strip_prefix('^')) {
                if !valid_oid(base) {
                    return Err(error.into());
                }
                ancestors.push(base);
                lines.next();
            }
            // Unrelated histories and multiple best ancestors remain ineligible.
            if ancestors.len() == 1 {
                bases.insert((*tip).to_owned(), ancestors[0].to_owned());
            }
        }
        if lines.next().is_some() {
            return Err(error.into());
        }
    }
    if bases.is_empty() {
        return Ok(BTreeSet::new());
    }
    let commits: BTreeSet<_> = bases
        .iter()
        .flat_map(|(tip, base)| [tip.as_str(), base.as_str()])
        .chain(std::iter::once(target))
        .collect();
    let commits: Vec<_> = commits.into_iter().collect();
    let mut trees = BTreeMap::new();
    for chunk in commits.chunks(BATCH_SIZE) {
        let mut command = git(&["rev-parse", "--revs-only", "--end-of-options"]);
        command.args(chunk.iter().map(|commit| format!("{commit}^{{tree}}")));
        let output = git_command_text(command, "rev-parse", progress)?;
        let ids: Vec<_> = output.lines().collect();
        if ids.len() != chunk.len() || ids.iter().any(|oid| !valid_oid(oid)) {
            return Err("cannot parse tree IDs".into());
        }
        trees.extend(
            chunk
                .iter()
                .zip(ids)
                .map(|(commit, tree)| (*commit, tree.to_owned())),
        );
    }
    let mut included = BTreeSet::new();
    let mut reachable = None;
    let branches: Vec<_> = bases.iter().collect();
    for chunk in branches.chunks(BATCH_SIZE) {
        let pairs: BTreeSet<_> = chunk
            .iter()
            .filter(|(tip, base)| {
                trees[tip.as_str()] != trees[base.as_str()]
                    && trees[tip.as_str()] != trees[target]
                    && trees[base.as_str()] != trees[target]
            })
            .flat_map(|(tip, base)| {
                [
                    (trees[base.as_str()].clone(), trees[tip.as_str()].clone()),
                    (trees[base.as_str()].clone(), trees[target].clone()),
                ]
            })
            .collect();
        let mut diffs = if pairs.is_empty() {
            BTreeMap::new()
        } else {
            tree_changes(&pairs, progress)?
        };
        for (tip, base) in chunk {
            // Identical immutable trees settle these cases without walking files:
            // no net changes stays ineligible; an unchanged target contains none
            // of a divergent tip's changes; an identical target contains them all.
            if trees[tip.as_str()] == trees[base.as_str()] || trees[base.as_str()] == trees[target]
            {
                continue;
            }
            if trees[tip.as_str()] == trees[target] {
                included.insert((*tip).clone());
                continue;
            }
            let changes = &diffs[&(trees[base.as_str()].clone(), trees[tip.as_str()].clone())];
            let target_changes = &diffs[&(trees[base.as_str()].clone(), trees[target].clone())];
            if !changes.is_empty()
                && changes
                    .iter()
                    .all(|(path, change)| target_changes.get(path) == Some(change))
            {
                included.insert((*tip).clone());
            }
        }
        // Reuse this batch's branch diffs; do not walk the same trees again.
        let mut pending = BTreeMap::<&str, Vec<&str>>::new();
        for (tip, base) in chunk {
            if !included.contains(*tip) && trees[tip.as_str()] != trees[base.as_str()] {
                pending.entry(base.as_str()).or_default().push(tip.as_str());
            }
        }
        for (base, mut tips) in pending {
            let range = format!("{base}..{target}");
            let output = git_text(
                &[
                    "rev-list",
                    "--format=%T",
                    "--no-commit-header",
                    "--first-parent",
                    "--ancestry-path",
                    &range,
                    "--",
                ],
                progress,
            )?;
            let mut history = BTreeSet::new();
            let mut oldest = None;
            for tree in output.lines() {
                if !valid_oid(tree) {
                    return Err("cannot parse historical tree IDs".into());
                }
                oldest = Some(tree);
                if tree != trees[target] && tree != trees[base] {
                    history.insert(tree.to_owned());
                }
            }
            if history.is_empty() {
                continue;
            }
            if trees[base] == trees[target] {
                // The latest-tree shortcut did not need these branch diffs.
                let pairs = tips
                    .iter()
                    .map(|tip| (trees[base].clone(), trees[*tip].clone()))
                    .collect();
                diffs.extend(tree_changes(&pairs, progress)?);
            }
            if history.len() > BATCH_SIZE {
                // Try the oldest eligible state before comparing a large batch.
                // This often finds integrations followed by unrelated changes.
                if let Some(tree) = oldest.filter(|tree| history.contains(*tree)) {
                    let pair = (trees[base].clone(), tree.to_owned());
                    let snapshots = tree_changes(&BTreeSet::from([pair.clone()]), progress)?;
                    let snapshot = &snapshots[&pair];
                    for tip in &tips {
                        let changes = &diffs[&(trees[base].clone(), trees[*tip].clone())];
                        if !changes.is_empty()
                            && changes
                                .iter()
                                .all(|(path, change)| snapshot.get(path) == Some(change))
                        {
                            included.insert((*tip).to_owned());
                        }
                    }
                    history.remove(tree);
                    tips.retain(|tip| !included.contains(*tip));
                    if tips.is_empty() {
                        continue;
                    }
                }
                // A matching snapshot must reference each non-deleted blob.
                // Enumerate the target's entire reachable history (a superset
                // of eligible snapshots) once. Absence can only rule a tip out;
                // presence never proves inclusion. Gitlinks are not traversed
                // by rev-list --objects, so their OIDs must not be required.
                let mut required = BTreeMap::new();
                for tip in &tips {
                    let changes = &diffs[&(trees[base].clone(), trees[*tip].clone())];
                    let mut objects = BTreeSet::new();
                    for change in changes.values() {
                        let fields: Vec<_> = change.split(|byte| *byte == b' ').collect();
                        if fields.len() != 5 {
                            return Err("cannot parse tree changes".into());
                        }
                        if !matches!(fields[1], b"000000" | b"160000") {
                            let oid = std::str::from_utf8(fields[3])
                                .ok()
                                .filter(|oid| valid_oid(oid))
                                .ok_or("cannot parse tree changes")?;
                            objects.insert(oid);
                        }
                    }
                    required.insert(*tip, objects);
                }
                if required.values().any(|objects| !objects.is_empty()) {
                    if reachable.is_none() {
                        reachable = Some(reachable_objects(target, progress)?);
                    }
                    let reachable = reachable.as_ref().unwrap();
                    tips.retain(|tip| required[tip].iter().all(|oid| reachable.contains(*oid)));
                    if tips.is_empty() {
                        continue;
                    }
                }
            }
            let history: Vec<_> = history.into_iter().collect();
            for snapshots in history.chunks(BATCH_SIZE) {
                let pairs = snapshots
                    .iter()
                    .map(|tree| (trees[base].clone(), tree.clone()))
                    .collect();
                let historical_changes = tree_changes(&pairs, progress)?;
                for tip in &tips {
                    let changes = &diffs[&(trees[base].clone(), trees[*tip].clone())];
                    if !changes.is_empty()
                        && historical_changes.values().any(|snapshot| {
                            changes
                                .iter()
                                .all(|(path, change)| snapshot.get(path) == Some(change))
                        })
                    {
                        included.insert((*tip).to_owned());
                    }
                }
                if tips.iter().all(|tip| included.contains(*tip)) {
                    break;
                }
            }
        }
    }
    Ok(included)
}

fn branch_snapshot(
    merged_into: Option<&str>,
    name: Option<&str>,
    progress: &Progress,
) -> Result<BTreeMap<String, Branch>, String> {
    let mut args = vec!["for-each-ref", "--sort=refname", BRANCH_FORMAT];
    let merged_arg;
    if let Some(oid) = merged_into {
        merged_arg = format!("--merged={oid}");
        args.push(&merged_arg);
    }
    let reference = name.map_or_else(|| "refs/heads/".to_owned(), |n| format!("refs/heads/{n}"));
    args.push(&reference);
    let output = git_text(&args, progress)?;
    let mut branches = BTreeMap::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 5
            || !fields[0].starts_with("refs/heads/")
            || !valid_oid(fields[1])
            || !matches!(fields[2], "*" | " ")
            || !matches!(fields[3], "used" | "free")
        {
            return Err("cannot parse local branch state".into());
        }
        let branch = Branch {
            name: fields[0]["refs/heads/".len()..].to_owned(),
            oid: fields[1].to_owned(),
            in_use: fields[2] == "*" || fields[3] == "used",
            symbolic: !fields[4].is_empty(),
        };
        branches.insert(branch.name.clone(), branch);
    }
    Ok(branches)
}

fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid.bytes().all(|b| b.is_ascii_hexdigit())
        && oid.bytes().any(|b| b != b'0')
}

fn parse_remote_head(output: &str) -> Result<RemoteHead, String> {
    let mut branch = None;
    let mut oid = None;
    for line in output.lines() {
        let Some((value, reference)) = line.split_once('\t') else {
            return Err("cannot parse origin refs".into());
        };
        if reference != "HEAD" {
            continue;
        }
        if let Some(name) = value.strip_prefix("ref: refs/heads/") {
            if name.is_empty() || branch.replace(name.to_owned()).is_some() {
                return Err("cannot determine a unique default branch for origin".into());
            }
        } else if valid_oid(value) {
            if oid.replace(value.to_owned()).is_some() {
                return Err("cannot determine a unique latest commit for origin".into());
            }
        } else {
            return Err("origin HEAD does not point to a branch".into());
        }
    }
    match (branch, oid) {
        (Some(branch), Some(oid)) => Ok(RemoteHead { branch, oid }),
        _ => Err(
            "cannot determine origin's default branch and latest commit; check the remote HEAD"
                .into(),
        ),
    }
}

fn latest_remote_head(progress: &Progress) -> Result<RemoteHead, String> {
    // Require a configured remote, rather than accidentally treating "origin" as a path.
    progress.update("Checking origin's default branch and latest commit...");
    let origin = git_text(&["remote", "get-url", "origin"], progress)?;
    let mut query = git(&[]);
    ssh::configure(&mut query, origin.trim());
    query.args(["ls-remote", "--symref", "--exit-code", "origin", "HEAD"]);
    let remote = parse_remote_head(&git_command_text(query, "ls-remote", progress)?)?;
    git_text(
        &["check-ref-format", &format!("refs/heads/{}", remote.branch)],
        progress,
    )?;
    let commit = git(&["cat-file", "-t", &remote.oid])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("could not check the target commit: {e}"))?;
    if !commit.status.success() || commit.stdout != b"commit\n" {
        return Err(format!(
            "the latest commit of origin/{} ({}) is not available locally; run git fetch origin and retry",
            remote.branch, remote.oid
        ));
    }
    Ok(remote)
}

struct BranchInspection {
    candidates: BTreeMap<String, Branch>,
}

fn inspect_branches(
    progress: &Progress,
    report: &mut RunReport,
) -> Result<BranchInspection, String> {
    let all = branch_snapshot(None, None, progress)?;
    for branch in all.values() {
        report.record(branch.decision(branch.protection().unwrap_or(ReasonCode::NotInspected)));
    }
    let initial: BTreeMap<_, _> = all
        .into_iter()
        .filter(|(_, branch)| !branch.protected())
        .collect();
    if initial.is_empty() {
        return Ok(BranchInspection {
            candidates: BTreeMap::new(),
        });
    }
    let remote = latest_remote_head(progress)?;
    progress.update("Checking merged branches...");
    let mut merged = branch_snapshot(Some(&remote.oid), None, progress)?;
    let tips: BTreeSet<_> = initial
        .iter()
        .filter(|(name, _)| *name != &remote.branch && !merged.contains_key(*name))
        .map(|(_, branch)| branch.oid.as_str())
        .collect();
    let included = squash_included(&tips.into_iter().collect::<Vec<_>>(), &remote.oid, progress)?;
    let ancestry: BTreeSet<_> = merged.keys().cloned().collect();
    for (name, branch) in &initial {
        if name != &remote.branch && !merged.contains_key(name) && included.contains(&branch.oid) {
            merged.insert(name.clone(), branch.clone());
        }
    }
    let mut candidates = BTreeMap::new();
    for (name, branch) in &initial {
        let reason = if name == &remote.branch {
            ReasonCode::DefaultBranch
        } else if let Some(current) = merged.get(name) {
            if let Some(reason) = current.protection() {
                reason
            } else if current.oid != branch.oid {
                ReasonCode::StateChanged
            } else if ancestry.contains(name) {
                ReasonCode::Merged
            } else {
                ReasonCode::SquashIncluded
            }
        } else {
            ReasonCode::NotMerged
        };
        let decision = branch.decision(reason);
        if decision.is_deletable() {
            candidates.insert(name.clone(), merged[name].clone());
        }
        report.record(decision);
    }
    Ok(BranchInspection { candidates })
}

fn clean_branches(
    dry_run: bool,
    out: &mut impl Write,
    report: &mut RunReport,
) -> Result<ExitCode, String> {
    let result = execute_branches(dry_run, out, report);
    report.finish(&result);
    result
}

fn execute_branches(
    dry_run: bool,
    out: &mut impl Write,
    report: &mut RunReport,
) -> Result<ExitCode, String> {
    let progress = Progress::start("Checking local branches...");
    if git_text(&["rev-parse", "--is-inside-work-tree"], &progress)?.trim() != "true" {
        return Err("run this command inside a Git working tree".into());
    }
    let inspection = inspect_branches(&progress, report)?;
    let candidates = inspection.candidates;
    if candidates.is_empty() {
        progress.finish();
        return Ok(ExitCode::SUCCESS);
    }
    progress.update(if dry_run {
        "Checking deletion candidates..."
    } else {
        "Deleting branches..."
    });
    let mut results = Vec::new();
    let mut failed = false;
    let batch_failure = if dry_run {
        None
    } else {
        match bulk::delete(&candidates, &progress)? {
            bulk::Outcome::Deleted => {
                progress.finish();
                for branch in candidates.values() {
                    record_outcome(
                        &mut report.results,
                        &branch.decision(ReasonCode::Merged).target,
                        Outcome::Deleted,
                    );
                }
                for name in candidates.keys() {
                    writeln!(out, "{}", render::deleted(name)).map_err(|e| e.to_string())?;
                }
                return Ok(ExitCode::SUCCESS);
            }
            bulk::Outcome::Retry(output) => Some(output),
            bulk::Outcome::Individual => None,
        }
    };
    for (name, branch) in candidates {
        let decision = report
            .decisions
            .iter_mut()
            .find(|d| matches!(&d.target, Target::Branch { name: n, .. } if n == &name))
            .expect("candidate has a decision");
        // Recheck each branch immediately before acting, including changes made after
        // the remote query. Git also refuses to delete branches checked out in worktrees.
        let latest = match branch_snapshot(None, Some(&name), &progress) {
            Ok(latest) => latest,
            Err(error) => {
                decision.inspection_failed(error.clone());
                if !dry_run {
                    progress.suspend();
                    for result in &results {
                        writeln!(out, "{result}").map_err(|e| e.to_string())?;
                    }
                }
                return Err(error);
            }
        };
        // A failed batch may have removed refs before branch-config cleanup
        // failed. Preserve those failure rows instead of silently losing them.
        if !latest.contains_key(&name) {
            decision.revise(ReasonCode::TargetMissing);
            record_outcome(&mut report.results, &decision.target, Outcome::Skipped);
            if let Some(output) = &batch_failure {
                failed = true;
                let detail = deletion_reason(&name, output);
                record_outcome(
                    &mut report.results,
                    &decision.target,
                    Outcome::Failed(detail.clone()),
                );
                results.push(render::failed(&name, detail));
            }
            continue;
        }
        let current = &latest[&name];
        if let Some(reason) = current.protection() {
            decision.revise(reason);
            record_outcome(&mut report.results, &decision.target, Outcome::Skipped);
            continue;
        }
        if current.oid != branch.oid {
            decision.revise(ReasonCode::StateChanged);
            record_outcome(&mut report.results, &decision.target, Outcome::Skipped);
            continue;
        }
        decision.revise(decision.reason);
        if dry_run {
            progress.suspend();
            record_outcome(&mut report.results, &decision.target, Outcome::Candidate);
            writeln!(out, "{}", render::candidate(&name)).map_err(|e| e.to_string())?;
        } else {
            // Capture diagnostics for the result row instead of interrupting the spinner.
            // Git's English diagnostics also keep failure reasons independent of locale.
            let deletion = git(&["branch", "-D", "--quiet", "--", &name])
                .stderr(Stdio::piped())
                .output();
            match deletion {
                Ok(output) if output.status.success() => {
                    record_outcome(&mut report.results, &decision.target, Outcome::Deleted);
                    results.push(render::deleted(&name));
                }
                Ok(output) => {
                    failed = true;
                    let detail = deletion_reason(&name, &output);
                    record_outcome(
                        &mut report.results,
                        &decision.target,
                        Outcome::Failed(detail.clone()),
                    );
                    results.push(render::failed(&name, detail));
                }
                Err(error) => {
                    failed = true;
                    let detail = format!("could not run git: {error}");
                    record_outcome(
                        &mut report.results,
                        &decision.target,
                        Outcome::Failed(detail.clone()),
                    );
                    results.push(render::failed(&name, detail));
                }
            }
        }
    }
    progress.finish();
    for result in &results {
        writeln!(out, "{result}").map_err(|e| e.to_string())?;
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn deletion_reason(name: &str, output: &Output) -> String {
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    let diagnostics = diagnostics.trim();
    if diagnostics.lines().count() == 1
        && (diagnostics.starts_with(&format!(
            "error: cannot delete branch '{name}' used by worktree at '"
        )) || diagnostics.starts_with(&format!(
            "error: Cannot delete branch '{name}' checked out at '"
        )))
    {
        return "checked out in a worktree".into();
    }
    if diagnostics.is_empty() {
        return format!("git branch failed with {}", output.status);
    }
    diagnostics
        .lines()
        .map(|line| line.trim().strip_prefix("error: ").unwrap_or(line.trim()))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_protection_has_structured_reasons() {
        let mut branch = Branch {
            name: "topic".into(),
            oid: "a".repeat(40),
            in_use: false,
            symbolic: false,
        };
        assert_eq!(branch.protection(), None);
        branch.symbolic = true;
        assert_eq!(branch.protection(), Some(ReasonCode::SymbolicBranch));
        branch.in_use = true;
        assert_eq!(branch.protection(), Some(ReasonCode::BranchInUse));
        branch.in_use = false;
        branch.symbolic = false;
        for name in ["main", "master", "production"] {
            branch.name = name.into();
            let d = branch.decision(branch.protection().unwrap());
            assert_eq!(d.reason, ReasonCode::ProtectedBranch);
            assert!(!d.is_deletable());
        }
    }

    #[test]
    fn remote_head_requires_both_a_symbolic_branch_and_a_valid_oid() {
        let oid = "a".repeat(40);
        let parsed =
            parse_remote_head(&format!("ref: refs/heads/trunk\tHEAD\n{oid}\tHEAD\n")).unwrap();
        assert_eq!(parsed.branch, "trunk");
        assert_eq!(parsed.oid, oid);
        for invalid in [
            String::new(),
            format!("{oid}\tHEAD\n"),
            "ref: refs/heads/main\tHEAD\n".into(),
            format!("ref: refs/tags/v1\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\n{oid}\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\nref: refs/heads/other\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\n{}\tHEAD\n", "0".repeat(40)),
            "ref: refs/heads/main\tHEAD\ninvalid\tHEAD\n".into(),
        ] {
            assert!(parse_remote_head(&invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn remote_head_ignores_refs_whose_names_only_end_in_head() {
        let oid = "a".repeat(64);
        let parsed = parse_remote_head(&format!(
            "ref: refs/heads/main\tHEAD\n{oid}\tHEAD\n{}\trefs/heads/HEAD\n",
            "b".repeat(64)
        ))
        .unwrap();
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.oid, oid);
    }
}
