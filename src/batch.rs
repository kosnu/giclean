//! Run the existing cleanup entry points in isolated repository processes.
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use crate::discovery::{self, DiscoveryOptions};

pub const HELP: &str = "giclean batch — Clean multiple repositories

Usage: giclean batch <br|wt> [options] [paths...]

Options:
  --dry-run          Preview only (required unless --execute is supplied)
  --execute          Explicitly authorize cleanup without confirmation
  --max-depth N      Search depth from each root (default: 3)
  --exclude NAME     Skip directories with this exact basename (repeatable)
  --explain          Include existing cleanup explanations
  --days N, --hours N Worktree age options (wt only)
  --help             Show help
  --                 Treat remaining arguments as root paths

Defaults to the current directory. Symlinks are not followed. Bare repositories
are skipped. Each repository uses the existing br/wt safety checks, including
reinspection before deletion, and no automatic fetch. Failures do not stop
other repositories; any discovery or cleanup failure gives exit status 1.
Output is grouped by repository; batch currently supports text output only.
";

struct Options {
    discovery: DiscoveryOptions,
    cleanup: Vec<OsString>,
}

pub fn hook(args: &[OsString]) -> Option<ExitCode> {
    if args.first().is_none_or(|arg| arg != "batch") {
        return None;
    }
    if args.len() == 1
        || (args.len() == 2 && args[1] == "--help")
        || (args.len() == 3 && (args[1] == "br" || args[1] == "wt") && args[2] == "--help")
    {
        print!("{HELP}");
        return Some(ExitCode::SUCCESS);
    }
    let options = match parse(&args[1..]) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("giclean batch: {error}\nUse giclean batch --help for usage.");
            return Some(ExitCode::from(2));
        }
    };
    Some(match run(options) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("giclean batch: {error}");
            ExitCode::FAILURE
        }
    })
}

fn parse(args: &[OsString]) -> Result<Options, String> {
    if !args.first().is_some_and(|arg| arg == "br" || arg == "wt") {
        return Err("expected br or wt".into());
    }
    let mut cleanup = vec![args[0].clone()];
    let mut roots = Vec::new();
    let mut excludes = Vec::new();
    let mut max_depth = 3;
    let mut depth_seen = false;
    let mut mode = None;
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--") => {
                roots.extend(args.map(PathBuf::from));
                break;
            }
            Some("--dry-run" | "--execute") => {
                if mode.replace(arg == "--dry-run").is_some() {
                    return Err("specify exactly one of --dry-run or --execute".into());
                }
            }
            Some("--max-depth") => {
                if depth_seen {
                    return Err("specify --max-depth only once".into());
                }
                depth_seen = true;
                max_depth = args
                    .next()
                    .and_then(|v| v.to_str())
                    .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|v| v.parse().ok())
                    .ok_or("--max-depth requires a nonnegative integer")?;
            }
            Some("--exclude") => {
                let name = args
                    .next()
                    .ok_or("--exclude requires a directory basename")?;
                let path = std::path::Path::new(name);
                if path.file_name() != Some(name.as_os_str()) || name.is_empty() {
                    return Err("--exclude requires a directory basename".into());
                }
                excludes.push(name.clone());
            }
            Some("--format") => {
                if args.next().is_none_or(|value| value != "text") {
                    return Err("batch currently supports --format text only".into());
                }
            }
            Some("--explain") => cleanup.push(arg.clone()),
            Some("--days" | "--hours" | "-d" | "-h") if cleanup[0] == "wt" => {
                cleanup.push(arg.clone());
                cleanup.push(
                    args.next()
                        .ok_or("worktree age option requires a value")?
                        .clone(),
                );
            }
            Some(value) if value.starts_with('-') => {
                return Err(format!("unknown option: {value}"));
            }
            _ => roots.push(PathBuf::from(arg)),
        }
    }
    if mode.ok_or("specify --dry-run to preview or --execute to delete")? {
        cleanup.push("--dry-run".into());
    }
    // Validate every cleanup argument before any repository can be changed.
    crate::parse_options(cleanup.clone().into_iter())?;
    if roots.is_empty() {
        roots.push(".".into());
    }
    Ok(Options {
        discovery: DiscoveryOptions {
            roots,
            max_depth,
            excludes,
        },
        cleanup,
    })
}

// Keep the launch worktree protected even when discovery prefers its main tree.
fn launch_worktree() -> Result<Option<discovery::Repository>, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    for ancestor in cwd.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                let mut result = discovery::discover(&DiscoveryOptions {
                    roots: vec![ancestor.to_owned()],
                    max_depth: 0,
                    excludes: Vec::new(),
                });
                if !result.errors.is_empty() || result.repositories.len() != 1 {
                    return Err("cannot identify the current worktree safely".into());
                }
                return Ok(result.repositories.pop());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect current worktree: {error}")),
        }
    }
    Ok(None)
}

fn run(options: Options) -> Result<ExitCode, String> {
    // Reject ambient repository overrides instead of risking another repository.
    // Preserve authentication, global configuration, and transport settings.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        if std::env::var_os(name).is_some() {
            return Err(format!("unset {name} before running batch cleanup"));
        }
    }
    let launch = launch_worktree()?;
    let discovered = discovery::discover(&options.discovery);
    let discovery_errors = discovered.errors.len();
    let mut failed = discovery_errors > 0;
    let mut skipped = 0;
    let mut succeeded = 0;
    let mut unsuccessful = 0;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for error in discovered.errors {
        eprintln!("giclean batch: {:?}: {}", error.path, error.message);
    }
    for mut repository in discovered.repositories {
        if let Some(launch) = &launch
            && launch.common_git_dir == repository.common_git_dir
            && !launch.bare
            && !repository.bare
        {
            repository.path = launch.path.clone();
            repository.bare = false;
        }
        writeln!(out, "== Repository {:?} ==", repository.path).map_err(|e| e.to_string())?;
        if repository.bare {
            writeln!(out, "Skipped: bare repository").map_err(|e| e.to_string())?;
            skipped += 1;
            continue;
        }
        let fresh = discovery::discover(&DiscoveryOptions {
            roots: vec![repository.path.clone()],
            max_depth: 0,
            excludes: Vec::new(),
        });
        if !fresh.errors.is_empty()
            || fresh.repositories.len() != 1
            || fresh.repositories[0].common_git_dir != repository.common_git_dir
            || fresh.repositories[0].bare
        {
            failed = true;
            unsuccessful += 1;
            eprintln!(
                "giclean batch: {:?}: repository identity changed or could not be rechecked",
                repository.path
            );
            continue;
        }
        out.flush().map_err(|e| e.to_string())?;
        let status = Command::new(&executable)
            .args(&options.cleanup)
            .current_dir(&repository.path)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        match status {
            Ok(status) if status.success() => succeeded += 1,
            result => {
                failed = true;
                unsuccessful += 1;
                match result {
                    Ok(status) => eprintln!(
                        "giclean batch: {:?}: cleanup failed ({status})",
                        repository.path
                    ),
                    Err(error) => eprintln!(
                        "giclean batch: {:?}: could not run cleanup: {error}",
                        repository.path
                    ),
                }
            }
        }
    }
    writeln!(
        out,
        "Batch summary: {succeeded} succeeded, {unsuccessful} failed, {skipped} skipped, {discovery_errors} discovery errors"
    )
    .map_err(|e| e.to_string())?;
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
