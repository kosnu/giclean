//! Deliberately separate from directory cleanup. Git prune is repository-wide,
//! so every potentially prunable registration must pass before executing it.
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

const HELP: &str = "giclean wt-prune — Remove stale Git worktree registrations only

Usage: giclean wt-prune (-d <days> | -h <hours>) (--dry-run | --execute)

Requires an explicit positive age and mode. Does not delete worktree directories,
branches, directly delete commit objects, fetch, or archive unreachable commits. Git's prunable signal
is required. Age is the latest modification of registration metadata, not PR age.
All prunable registrations must pass: one unsafe or recent registration blocks the
entire operation because Git prune is repository-wide. Unique HEAD/reflog commits,
staged changes and unverifiable metadata are retained. Stop other Git/worktree
operations while executing; Git offers no atomic scoped prune API.
";

pub fn hook(args: &[OsString]) -> Option<ExitCode> {
    if args.first().is_none_or(|a| a != "wt-prune") {
        return None;
    }
    if args[1..] == ["--help"] {
        print!("{HELP}");
        return Some(ExitCode::SUCCESS);
    }
    Some(match run(&args[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("giclean wt-prune: {error}");
            ExitCode::FAILURE
        }
    })
}

fn command() -> std::process::Command {
    let mut command = crate::git::command();
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1");
    command
}

fn git(args: &[&str]) -> Result<String, String> {
    let output = command().args(args).output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    String::from_utf8(output.stdout).map_err(|_| "non-UTF-8 Git output; retained".into())
}

fn run(args: &[OsString]) -> Result<(), String> {
    let mut age = None;
    let mut execute = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--dry-run" | "--execute") if execute.is_none() => {
                execute = Some(arg == "--execute")
            }
            Some("-d" | "--days" | "-h" | "--hours") if age.is_none() => {
                let unit = if arg == "-d" || arg == "--days" {
                    86400
                } else {
                    3600
                };
                age = Some(Duration::from_secs(
                    iter.next()
                        .and_then(|v| v.to_str())
                        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|v| v.parse::<u64>().ok())
                        .filter(|v| *v > 0)
                        .and_then(|v| v.checked_mul(unit))
                        .ok_or("invalid positive age")?,
                ));
            }
            _ => return Err(HELP.into()),
        }
    }
    let age = age.ok_or(HELP)?;
    let execute = execute.ok_or(HELP)?;
    let initial = inspect(age)?;
    if initial.candidates.is_empty() {
        println!("No stale registrations to prune.");
        return Ok(());
    }
    for name in &initial.candidates {
        println!(
            "{} registration {name}",
            if execute { "Candidate:" } else { "Would prune" }
        );
    }
    if !execute {
        return Ok(());
    }
    // Re-read *all* registrations and their contents, refs and Git's own prune
    // preview, not just the selected entries, immediately before execution.
    let current = inspect(age)?;
    if initial != current {
        return Err("repository changed during verification; nothing pruned".into());
    }
    git(&["worktree", "prune", "--expire=now"])?;
    let remaining = git(&["worktree", "list", "--porcelain", "-z"])?;
    if remaining != initial.after_list {
        return Err(
            "post-prune registration list differs from expected; inspect repository".into(),
        );
    }
    println!(
        "Pruned {} stale registrations; no worktree directories removed.",
        initial.candidates.len()
    );
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    candidates: BTreeSet<String>,
    files: Vec<(String, Vec<u8>, SystemTime)>,
    refs: String,
    ref_state: String,
    list: String,
    after_list: String,
    preview: String,
}

fn inspect(age: Duration) -> Result<Snapshot, String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    if git(&["rev-parse", "--is-shallow-repository"])?.trim() != "false"
        || Path::new(common.trim()).join("info/grafts").exists()
    {
        return Err("shallow or grafted history cannot verify retention; nothing pruned".into());
    }
    let admin = Path::new(common.trim()).join("worktrees");
    let list = git(&["worktree", "list", "--porcelain", "-z"])?;
    let refs = git(&["for-each-ref", "--format=%(refname)"])?;
    let mut result = Snapshot {
        candidates: BTreeSet::new(),
        files: vec![],
        refs,
        ref_state: git(&["for-each-ref", "--format=%(refname)%09%(objectname)"])?,
        list: list.clone(),
        after_list: String::new(),
        preview: String::new(),
    };
    let mut stale_paths = BTreeSet::new();
    for record in list.split_inclusive("\0\0") {
        let fields: Vec<_> = record.split('\0').filter(|s| !s.is_empty()).collect();
        let stale = fields
            .iter()
            .any(|f| *f == "prunable" || f.starts_with("prunable "));
        if stale {
            if fields
                .iter()
                .any(|f| *f == "locked" || f.starts_with("locked "))
            {
                return Err("locked prunable registration; entire operation retained".into());
            }
            let path = fields
                .first()
                .and_then(|s| s.strip_prefix("worktree "))
                .ok_or("unverifiable worktree list")?;
            stale_paths.insert(Path::new(path).join(".git"));
        } else {
            result.after_list.push_str(record);
        }
    }
    if !admin.exists() {
        if !stale_paths.is_empty() {
            return Err("missing registration metadata".into());
        }
        return Ok(result);
    }
    let mut dirs: Vec<_> = fs::read_dir(&admin)
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    dirs.sort_by_key(|e| e.file_name());
    for entry in dirs {
        let path = entry.path();
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            return Err("unexpected registration entry; retained".into());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 registration")?;
        let gitdir = fs::read_to_string(path.join("gitdir"))
            .map_err(|_| "unreadable registration; entire operation retained")?;
        let is_stale = stale_paths.remove(Path::new(gitdir.trim_end_matches('\n')));
        // Even non-candidates must be accounted for in porcelain. Malformed
        // metadata can otherwise be removed by a repository-wide Git prune.
        if !is_stale {
            let workpath = Path::new(gitdir.trim_end_matches('\n'))
                .parent()
                .ok_or("invalid gitdir")?;
            let field = format!(
                "worktree {}\0",
                workpath.to_str().ok_or("invalid worktree path")?
            );
            if !list.split("\0\0").any(|record| record.starts_with(&field)) {
                return Err("registration absent from Git list; entire operation retained".into());
            }
        }
        let start = result.files.len();
        collect(&path, &name, &mut result.files)?;
        if !is_stale {
            continue;
        }
        let now = SystemTime::now();
        if result.files[start..].iter().any(|(_, _, modified)| {
            now.duration_since(*modified)
                .map_or(true, |elapsed| elapsed < age)
        }) {
            return Err(format!(
                "registration {name} is younger than selected age; entire operation retained"
            ));
        }
        verify_commits(&path, &result.refs)
            .map_err(|error| format!("registration {name}: {error}"))?;
        result.candidates.insert(name);
    }
    if !stale_paths.is_empty() {
        return Err("unmatched prunable worktree; retained".into());
    }
    // Git dry-run prints to stderr. Require its exact candidate IDs to match.
    let output = command()
        .env("LC_ALL", "C")
        .args([
            "worktree",
            "prune",
            "--dry-run",
            "--verbose",
            "--expire=now",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Git prune preview failed".into());
    }
    let preview = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut ids = BTreeSet::new();
    for line in preview.lines() {
        let id = line
            .strip_prefix("Removing worktrees/")
            .and_then(|s| s.split_once(": ").map(|(id, _)| id))
            .ok_or("unrecognized Git prune preview; retained")?;
        if !ids.insert(id.to_owned()) {
            return Err("duplicate prune preview; retained".into());
        }
    }
    if ids != result.candidates {
        return Err("Git prune scope differs from verified candidates; retained".into());
    }
    result.preview = preview;
    Ok(result)
}

fn collect(
    path: &Path,
    label: &str,
    files: &mut Vec<(String, Vec<u8>, SystemTime)>,
) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let modified = meta.modified().map_err(|e| e.to_string())?;
    if meta.is_dir() {
        files.push((label.into(), vec![], modified));
        let mut entries: Vec<_> = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "non-UTF-8 metadata")?;
            collect(&entry.path(), &format!("{label}/{name}"), files)?;
        }
    } else if meta.is_file() {
        files.push((
            label.into(),
            fs::read(path).map_err(|e| e.to_string())?,
            modified,
        ));
    } else {
        return Err("symlink or special metadata; retained".into());
    }
    Ok(())
}

fn verify_commits(path: &Path, refs: &str) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if name == "refs" && empty_directories(&path.join(&name))? {
            continue;
        }
        if ![
            "HEAD",
            "index",
            "commondir",
            "gitdir",
            "logs",
            "ORIG_HEAD",
            "COMMIT_EDITMSG",
        ]
        .iter()
        .any(|allowed| name == *allowed)
        {
            return Err(format!(
                "operation state or unknown registration metadata {name:?}; retained"
            ));
        }
    }
    let dir = path.to_str().ok_or("non-UTF-8 registration")?;
    let head = git(&["--git-dir", dir, "rev-parse", "--verify", "HEAD^{commit}"])?;
    git(&[
        "--git-dir",
        dir,
        "diff-index",
        "--cached",
        "--quiet",
        "HEAD",
        "--",
    ])
    .map_err(|_| "staged or unverifiable index; retained")?;
    let mut commits = BTreeSet::from([head.trim().to_owned()]);
    if let Ok(orig) = fs::read_to_string(path.join("ORIG_HEAD")) {
        commits.insert(orig.trim().into());
    }
    let logs = path.join("logs");
    if logs.exists() {
        for entry in fs::read_dir(&logs).map_err(|e| e.to_string())? {
            if entry.map_err(|e| e.to_string())?.file_name() != "HEAD" {
                return Err("unknown worktree reflog; retained".into());
            }
        }
        let log = fs::read_to_string(logs.join("HEAD")).map_err(|_| "unreadable reflog")?;
        for line in log.lines() {
            let ids: Vec<_> = line.split_whitespace().take(2).collect();
            if ids.len() != 2 {
                return Err("invalid reflog; retained".into());
            }
            for id in ids {
                if !id.bytes().all(|b| b == b'0') {
                    commits.insert(id.into());
                }
            }
        }
    }
    if commits.iter().any(|id| !crate::valid_oid(id)) {
        return Err("invalid registered commit; retained".into());
    }
    let mut args = vec!["rev-list", "--max-count=1"];
    args.extend(commits.iter().map(String::as_str));
    args.push("--not");
    args.extend(refs.lines());
    args.push("--");
    if !git(&args)?.trim().is_empty() {
        return Err("unique registered HEAD/reflog commits are not reachable from retained refs; entire operation retained".into());
    }
    Ok(())
}

fn empty_directories(path: &Path) -> Result<bool, String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Ok(false);
    }
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        if !empty_directories(&entry.map_err(|e| e.to_string())?.path())? {
            return Ok(false);
        }
    }
    Ok(true)
}
