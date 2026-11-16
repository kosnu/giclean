//! Registration-only cleanup. Default Git prune remains all-or-nothing; the
//! explicit selective mode isolates verified metadata and never removes worktrees.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

const HELP: &str = "giclean wt prune — Remove stale Git worktree registrations only

Usage: giclean wt prune (-d <days> | -h <hours>) (--dry-run | --execute) [--skip-retained]

  -d, --days <days>    Wait this many days since registration activity
  -h, --hours <hours>  Wait this many hours since registration activity
  --dry-run           Preview candidates without removing registrations
  --execute           Remove registrations without confirmation
  --skip-retained     Skip unsafe/recent registrations; prune safe ones only
  --help              Show help

Deprecated alias: giclean wt-prune (warns on stderr).

Requires an explicit positive age and mode. Does not delete worktree directories,
branches, directly delete commit objects, fetch, or archive unreachable commits. Git's prunable signal
is required. Age is the latest modification of registration metadata, not PR age.
By default all prunable registrations must pass: one unsafe or recent registration
blocks the entire operation. --skip-retained selects safe stale registrations
only; skipped registrations and their reasons are reported. Structural uncertainty
still stops everything. Unique HEAD/reflog commits, staged changes and unverifiable
metadata are retained. Stop other Git/worktree operations while executing; Git offers no atomic scoped prune API.
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
    let mut skip_retained = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--skip-retained") if !skip_retained => skip_retained = true,
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
    let initial = inspect(age, skip_retained)?;
    for (name, reason) in &initial.retained {
        eprintln!(
            "Retained registration {name}: {}",
            reason.replace("; entire operation retained", "")
        );
    }
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
    let current = inspect(age, skip_retained)?;
    if initial != current {
        return Err("repository changed during verification; nothing pruned".into());
    }
    if skip_retained {
        selective_prune(&initial, age)?;
    } else {
        git(&["worktree", "prune", "--expire=now"])?;
    }
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
struct Target {
    admin: PathBuf,
    workpath: PathBuf,
    identity: (u64, u64),
}

fn path_absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

// A stale directory may still hold valuable files. We never remove or
// traverse it. Only its ordinary type and missing registration marker matter.
fn registration_missing(workpath: &Path) -> bool {
    workpath.is_absolute()
        && match fs::symlink_metadata(workpath) {
            Ok(meta) => meta.is_dir(),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
        && path_absent(&workpath.join(".git"))
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("registration is not a directory; retained".into());
    }
    Ok((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn directory_identity(_path: &Path) -> Result<(u64, u64), String> {
    Err("selective registration cleanup is unsupported on this platform".into())
}

fn selective_prune(initial: &Snapshot, age: Duration) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    verify_parents(initial)?;
    let quarantine = loop {
        let path = initial.common.join(format!(
            "giclean-prune-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not create recovery directory: {error}; nothing pruned"
                ));
            }
        }
    };
    let quarantine_identity = directory_identity(&quarantine)?;
    let mut isolated_names = BTreeSet::new();
    let mut removed_names = BTreeSet::new();
    let result = (|| {
        for (name, target) in &initial.targets {
            verify_parents(initial)?;
            if !registration_missing(&target.workpath)
                || directory_identity(&target.admin)? != target.identity
                || directory_identity(&quarantine)? != quarantine_identity
            {
                return Err("registration or working directory changed before isolation".into());
            }
            let mut files = vec![];
            collect(&target.admin, name, &mut files)?;
            if files != registration_files(initial, name) {
                return Err("registration changed before isolation".into());
            }
            // Only a Git administrative directory is moved, never a worktree
            // directory. Same depth keeps its relative commondir meaningful.
            let isolated = quarantine.join(name);
            if !path_absent(&isolated) {
                return Err("unexpected recovery destination; nothing overwritten".into());
            }
            // Destination is inside a fresh owner-only directory, never an
            // existing registration name. Do not attempt automatic rollback:
            // restoring would require a portable atomic no-replace rename.
            fs::rename(&target.admin, &isolated).map_err(|e| e.to_string())?;
            isolated_names.insert(name.clone());
            verify_isolated(initial, name, target, &quarantine, quarantine_identity)?;
        }
        verify_remaining(initial, age)?;
        for (name, target) in &initial.targets {
            verify_isolated(initial, name, target, &quarantine, quarantine_identity)?;
        }
        // No scoped Git prune exists. Delete only isolated administrative
        // metadata after validating it, never a recorded working directory.
        // This is not a lock against concurrent writers holding open files.
        for (name, target) in &initial.targets {
            verify_remaining(initial, age)?;
            verify_isolated(initial, name, target, &quarantine, quarantine_identity)?;
            fs::remove_dir_all(quarantine.join(name)).map_err(|e| e.to_string())?;
            removed_names.insert(name.clone());
        }
        verify_parents(initial)?;
        if directory_identity(&quarantine)? != quarantine_identity {
            return Err("recovery parent changed".into());
        }
        fs::remove_dir(&quarantine).map_err(|e| e.to_string())?;
        Ok(())
    })();
    result.map_err(|error: String| {
        let pending: Vec<_> = isolated_names.difference(&removed_names).cloned().collect();
        format!(
            "selective prune stopped: {error}. Confirmed removed registrations: {removed_names:?}; isolated but not confirmed removed: {pending:?}. recovery metadata, if any, is at {} (it may be incomplete after a deletion error). No worktree directories were removed. Stop Git operations and inspect before retrying; no automatic rollback was attempted.", quarantine.display())
    })
}

fn verify_remaining(initial: &Snapshot, age: Duration) -> Result<(), String> {
    verify_parents(initial)?;
    let current = inspect(age, true)?;
    let expected_files: Vec<_> = initial
        .files
        .iter()
        .filter(|(label, _, _)| {
            !initial
                .candidates
                .iter()
                .any(|name| label == name || label.starts_with(&format!("{name}/")))
        })
        .cloned()
        .collect();
    if current.list != initial.after_list
        || current.refs != initial.refs
        || current.ref_state != initial.ref_state
        || current.files != expected_files
        || current.retained != initial.retained
        || !current.candidates.is_empty()
    {
        return Err("repository changed after isolation".into());
    }
    Ok(())
}

fn verify_parents(snapshot: &Snapshot) -> Result<(), String> {
    let expected = snapshot
        .parent_identities
        .ok_or("missing parent identity")?;
    if directory_identity(&snapshot.common)? != expected.0
        || directory_identity(&snapshot.common.join("worktrees"))? != expected.1
    {
        return Err("registration parent identity changed".into());
    }
    Ok(())
}

fn registration_files(snapshot: &Snapshot, name: &str) -> Vec<(String, Vec<u8>, SystemTime)> {
    snapshot
        .files
        .iter()
        .filter(|(label, _, _)| label == name || label.starts_with(&format!("{name}/")))
        .cloned()
        .collect()
}

fn verify_isolated(
    snapshot: &Snapshot,
    name: &str,
    target: &Target,
    quarantine: &Path,
    quarantine_identity: (u64, u64),
) -> Result<(), String> {
    verify_parents(snapshot)?;
    if directory_identity(quarantine)? != quarantine_identity {
        return Err("recovery parent changed".into());
    }
    let isolated = quarantine.join(name);
    if !path_absent(&target.admin)
        || !registration_missing(&target.workpath)
        || directory_identity(&isolated)? != target.identity
    {
        return Err("isolated registration identity or working directory changed".into());
    }
    let mut files = vec![];
    collect(&isolated, name, &mut files)?;
    if files != registration_files(snapshot, name) {
        return Err("isolated registration changed; recovery metadata preserved".into());
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    candidates: BTreeSet<String>,
    retained: BTreeMap<String, String>,
    targets: BTreeMap<String, Target>,
    common: PathBuf,
    parent_identities: Option<((u64, u64), (u64, u64))>,
    files: Vec<(String, Vec<u8>, SystemTime)>,
    refs: String,
    ref_state: String,
    list: String,
    after_list: String,
    preview: String,
}

fn inspect(age: Duration, skip_retained: bool) -> Result<Snapshot, String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    if git(&["rev-parse", "--is-shallow-repository"])?.trim() != "false"
        || Path::new(common.trim()).join("info/grafts").exists()
    {
        return Err("shallow or grafted history cannot verify retention; nothing pruned".into());
    }
    let common_path = if skip_retained {
        fs::canonicalize(common.trim()).map_err(|e| e.to_string())?
    } else {
        PathBuf::from(common.trim())
    };
    let admin = common_path.join("worktrees");
    let list = git(&["worktree", "list", "--porcelain", "-z"])?;
    let refs = git(&["for-each-ref", "--format=%(refname)"])?;
    let mut result = Snapshot {
        candidates: BTreeSet::new(),
        retained: BTreeMap::new(),
        targets: BTreeMap::new(),
        common: common_path,
        parent_identities: None,
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
        }
    }
    if !admin.exists() {
        if !stale_paths.is_empty() {
            return Err("missing registration metadata".into());
        }
        result.after_list = list;
        return Ok(result);
    }
    if skip_retained
        && (!fs::symlink_metadata(&admin)
            .map_err(|e| e.to_string())?
            .is_dir()
            || !fs::symlink_metadata(&result.common)
                .map_err(|e| e.to_string())?
                .is_dir())
    {
        return Err("non-directory registration parent; nothing pruned".into());
    }
    if skip_retained {
        result.parent_identities = Some((
            directory_identity(&result.common)?,
            directory_identity(&admin)?,
        ));
    }
    let mut selected_paths = BTreeSet::new();
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
        if skip_retained {
            let marker = Path::new(gitdir.trim_end_matches('\n'));
            if !marker.is_absolute() || marker.file_name().is_none_or(|name| name != ".git") {
                return Err(
                    "invalid absolute registration marker; entire operation retained".into(),
                );
            }
        }
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
        let workpath = Path::new(gitdir.trim_end_matches('\n'))
            .parent()
            .ok_or("invalid gitdir")?
            .to_owned();
        let verification = if result.files[start..].iter().any(|(_, _, modified)| {
            now.duration_since(*modified)
                .map_or(true, |elapsed| elapsed < age)
        }) {
            Err("younger than selected age; entire operation retained".into())
        } else if skip_retained && !registration_missing(&workpath) {
            Err("worktree registration marker or path cannot be verified stale; retained".into())
        } else {
            verify_commits(&path, &result.refs)
        };
        if let Err(error) = verification {
            if skip_retained {
                result.retained.insert(name, error);
                continue;
            }
            return Err(format!("registration {name}: {error}"));
        }
        selected_paths.insert(workpath.clone());
        if skip_retained {
            result.targets.insert(
                name.clone(),
                Target {
                    admin: path.clone(),
                    workpath,
                    identity: directory_identity(&path)?,
                },
            );
        }
        result.candidates.insert(name);
    }
    if !stale_paths.is_empty() {
        return Err("unmatched prunable worktree; retained".into());
    }
    // Preserve porcelain order, omitting only registrations selected for removal.
    for record in list.split_inclusive("\0\0") {
        let selected = record
            .split('\0')
            .next()
            .and_then(|field| field.strip_prefix("worktree "))
            .is_some_and(|path| selected_paths.contains(Path::new(path)));
        if !selected {
            result.after_list.push_str(record);
        }
    }
    // Git dry-run prints to stderr. Account for retained candidates too; its
    // repository-wide prune must NEVER be invoked in selective mode.
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
    let expected_ids = result
        .candidates
        .iter()
        .chain(result.retained.keys())
        .cloned()
        .collect();
    if ids != expected_ids {
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
    // Validate before any command opens this registration as a repository.
    // Per-worktree config is loaded before -c overrides, including includes.
    if path.join("config.worktree").exists() {
        verify_worktree_config(&path.join("config.worktree"))?;
    }
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if name == "info" && sparse_checkout_only(&path.join(&name))? {
            continue;
        }
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
            "config.worktree",
            "FETCH_HEAD",
        ]
        .iter()
        .any(|allowed| name == *allowed)
        {
            return Err(format!(
                "operation state or unknown registration metadata {name:?}; retained"
            ));
        }
    }
    // --git-dir alone does not override GIT_INDEX_FILE or GIT_COMMON_DIR.
    // Bind every registration inspection to its own metadata/object database,
    // not to repo-local overrides inherited from the invoking Git process.
    let index = path.join("index");
    if !fs::symlink_metadata(&index)
        .map_err(|_| "missing or unreadable registration index; retained")?
        .is_file()
    {
        return Err("registration index is not a regular file; retained".into());
    }
    fs::File::open(&index).map_err(|_| "unreadable registration index; retained")?;
    let head = registration_git(path, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    registration_git(
        path,
        &["diff-index", "--cached", "--quiet", head.trim(), "--"],
    )
    .map_err(|_| "staged or unverifiable index; retained")?;
    let mut commits = BTreeSet::from([head.trim().to_owned()]);
    add_fetch_heads(path, &mut commits)?;
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
    if !registration_git(path, &args)?.trim().is_empty() {
        return Err("unique registered HEAD/reflog/FETCH_HEAD commits are not reachable from retained refs; entire operation retained".into());
    }
    Ok(())
}

// FETCH_HEAD can be the only remaining name for a fetched commit. Include
// every record, including not-for-merge, rather than discarding a known file.
fn add_fetch_heads(path: &Path, commits: &mut BTreeSet<String>) -> Result<(), String> {
    let file = path.join("FETCH_HEAD");
    match fs::symlink_metadata(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.is_file() => {}
        _ => return Err("unreadable or nonregular FETCH_HEAD; retained".into()),
    }
    let contents = fs::read_to_string(&file).map_err(|_| "unreadable FETCH_HEAD; retained")?;
    if contents.contains('\r') || (!contents.is_empty() && !contents.ends_with('\n')) {
        return Err("malformed FETCH_HEAD; retained".into());
    }
    let mut fetched = BTreeSet::new();
    for line in contents.lines() {
        let mut fields = line.splitn(3, '\t');
        let oid = fields.next().unwrap_or("");
        let merge = fields.next();
        let description = fields.next();
        if !crate::valid_oid(oid)
            || !matches!(merge, Some("" | "not-for-merge"))
            || description.is_none_or(|value| value.is_empty() || value.contains(['\t', '\r']))
        {
            return Err("malformed FETCH_HEAD; retained".into());
        }
        fetched.insert(oid.to_owned());
    }
    for oid in fetched {
        // A tag object's distinct identity is not proven safe by reachability
        // of its peeled commit alone. Keep non-commit objects conservatively.
        if registration_git(path, &["cat-file", "-t", &oid])?.trim() != "commit" {
            return Err("non-commit FETCH_HEAD object cannot be verified; retained".into());
        }
        commits.insert(oid);
    }
    Ok(())
}

// Git's sparse-checkout writes this exact worktree-local file. Other info
// files, split-index files and per-worktree refs remain unverified and retained.
fn sparse_checkout_only(path: &Path) -> Result<bool, String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Ok(false);
    }
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() != "sparse-checkout"
            || !entry.file_type().map_err(|e| e.to_string())?.is_file()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn verify_worktree_config(path: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("config.worktree is not a regular file; retained".into());
    }
    // Explicit file scope and --no-includes parse syntax without following any
    // includes. Accept only vetted checkout booleans and inert Codex metadata;
    // unknown keys are not harmless merely because Git recognizes their syntax.
    let output = command()
        .args(["config", "--no-includes", "--null", "--list", "--file"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("invalid or unreadable config.worktree; retained".into());
    }
    let contents =
        String::from_utf8(output.stdout).map_err(|_| "non-UTF-8 config.worktree; retained")?;
    for record in contents.split_terminator('\0') {
        let (key, value) = record.split_once('\n').unwrap_or((record, "true"));
        // Git does not interpret this Codex-owned pointer as a Git config
        // include or repository/index setting. Never open its target; pruning
        // removes the registration, not the referenced environment file.
        // Git normalizes section/key casing; do not allow other codex.* keys.
        if key == "codex.localenvironmentconfigpath" {
            continue;
        }
        if !matches!(
            key,
            "core.bare" | "core.sparsecheckout" | "core.sparsecheckoutcone" | "index.sparse"
        ) {
            return Err(format!("unsupported config.worktree key {key:?}; retained"));
        }
        let value = value.to_ascii_lowercase();
        let boolean = match value.as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" | "" => false,
            _ => {
                return Err(format!(
                    "unverifiable config.worktree boolean {key:?}; retained"
                ));
            }
        };
        if key == "core.bare" && boolean {
            return Err("bare config.worktree cannot be verified; retained".into());
        }
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

// Preserve the caller's top-level repository selection, but never let it
// redirect inspection of a specific registration to another index or repo.
fn registration_git(path: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = command();
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_GRAFT_FILE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_PREFIX",
        "GIT_INTERNAL_SUPER_PREFIX",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_REPLACE_REF_BASE",
    ] {
        cmd.env_remove(key);
    }
    let output = cmd
        .arg("--git-dir")
        .arg(path)
        .env("GIT_INDEX_FILE", path.join("index"))
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    String::from_utf8(output.stdout).map_err(|_| "non-UTF-8 registration output; retained".into())
}
