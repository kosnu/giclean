//! Read-only filesystem discovery. Repository identity is Git's canonical common directory.
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const HELP: &str = "giclean repos — Discover Git repositories

Usage: giclean repos [--max-depth N] [--exclude NAME] [--] [PATH...]

Lists repositories without fetching or deleting anything.
PATH defaults to the current directory. Roots have depth 0 (default limit: 3).
--exclude skips an exact directory basename, including roots; repeat as needed.
Symlink directory entries are skipped; explicit symlink roots are errors.
Root ancestors use normal path resolution. Git metadata is not traversed.
Linked worktrees sharing a common Git directory are listed once.
Bare repositories are labeled bare; they cannot be cleaned with br/wt.
Errors are reported separately; discovery continues and exits unsuccessfully.
";

#[derive(Debug)]
pub struct DiscoveryOptions {
    pub roots: Vec<PathBuf>,
    pub max_depth: usize,
    pub excludes: Vec<OsString>,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from(".")],
            max_depth: 3,
            excludes: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Repository {
    pub path: PathBuf,
    pub common_git_dir: PathBuf,
    pub bare: bool,
    main_worktree: bool,
}

#[derive(Debug)]
pub struct DiscoveryError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub repositories: Vec<Repository>,
    pub errors: Vec<DiscoveryError>,
}

pub fn parse(args: &[OsString]) -> Result<DiscoveryOptions, String> {
    let mut options = DiscoveryOptions::default();
    options.roots.clear();
    let mut args = args.iter();
    let mut paths_only = false;
    let mut has_depth = false;
    while let Some(arg) = args.next() {
        if paths_only {
            options.roots.push(arg.into());
        } else if arg == "--" {
            paths_only = true;
        } else if arg == "--max-depth" {
            if has_depth {
                return Err("specify --max-depth only once".into());
            }
            let value = args
                .next()
                .and_then(|s| s.to_str())
                .ok_or("--max-depth requires a nonnegative integer")?;
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err("--max-depth requires a nonnegative integer".into());
            }
            options.max_depth = value.parse().map_err(|_| "--max-depth is too large")?;
            has_depth = true;
        } else if arg == "--exclude" {
            let value = args
                .next()
                .ok_or("--exclude requires a directory basename")?;
            if value.is_empty()
                || Path::new(value).components().count() != 1
                || !matches!(
                    Path::new(value).components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                return Err("--exclude requires a directory basename (not a path or glob)".into());
            }
            options.excludes.push(value.clone());
        } else if arg.to_string_lossy().starts_with('-') {
            return Err(format!("unknown repos option: {}", arg.to_string_lossy()));
        } else {
            options.roots.push(arg.into());
        }
    }
    if options.roots.is_empty() {
        options.roots.push(PathBuf::from("."));
    }
    Ok(options)
}

fn git_path(path: &Path, argument: &str) -> Result<Vec<u8>, String> {
    let output = crate::git::command()
        .current_dir(path)
        .args([
            "--no-lazy-fetch",
            "rev-parse",
            "--path-format=absolute",
            argument,
        ])
        // Discovery must inspect this path, not an inherited repository override.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("cannot run git rev-parse: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse {argument} failed ({})",
            output.status
        ));
    }
    let mut bytes = output.stdout;
    if bytes.pop() != Some(b'\n') {
        return Err("invalid git rev-parse output".into());
    }
    Ok(bytes)
}

fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| "Git path is not valid UTF-8".into())
    }
}

fn inspect(path: &Path) -> Result<Option<Repository>, String> {
    let common_git_dir = path_from_bytes(git_path(path, "--git-common-dir")?)?
        .canonicalize()
        .map_err(|e| format!("cannot resolve common Git directory: {e}"))?;
    let bare = match git_path(path, "--is-bare-repository")?.as_slice() {
        b"true" => true,
        b"false" => false,
        _ => return Err("invalid bare repository response".into()),
    };
    let git_dir = path_from_bytes(git_path(path, "--absolute-git-dir")?)?
        .canonicalize()
        .map_err(|e| format!("cannot resolve Git directory: {e}"))?;
    if !bare && git_dir == path {
        // A valid --separate-git-dir directory is metadata, not a cleanup
        // target. Do not recurse into it or require a worktree top-level.
        return Ok(None);
    }
    if !bare {
        let top = path_from_bytes(git_path(path, "--show-toplevel")?)?
            .canonicalize()
            .map_err(|e| format!("cannot resolve worktree root: {e}"))?;
        if top != path {
            return Err("Git repository root does not match discovered path".into());
        }
    }
    Ok(Some(Repository {
        main_worktree: !bare && git_dir == common_git_dir,
        path: path.to_owned(),
        common_git_dir,
        bare,
    }))
}

// A trailing slash makes lstat follow a directory symlink on Unix. Remove only
// trailing separators and `/.` for this check, without
// normalizing `..`, ancestor links,
// or an all-separator filesystem root.
fn metadata_path(path: &Path) -> &Path {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        let mut end = bytes.len();
        loop {
            end = bytes[..end]
                .iter()
                .rposition(|byte| *byte != b'/')
                .map_or(end, |index| index + 1);
            if bytes[..end].ends_with(b"/.") {
                end -= 1;
            } else {
                break;
            }
        }
        Path::new(OsStr::from_bytes(&bytes[..end]))
    }
    #[cfg(not(unix))]
    {
        path
    }
}

pub fn discover(options: &DiscoveryOptions) -> Discovery {
    let mut result = Discovery::default();
    let mut repositories: BTreeMap<PathBuf, Repository> = BTreeMap::new();
    // Track remaining depth rather than only visited identity: overlapping roots can
    // legitimately request a deeper traversal from an already encountered directory.
    let mut visited: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut errors_seen = HashSet::new();
    let mut pending: Vec<_> = options.roots.iter().rev().map(|p| (p.clone(), 0)).collect();
    while let Some((path, depth)) = pending.pop() {
        let visit = (|| -> Result<(), String> {
            let metadata = fs::symlink_metadata(metadata_path(&path)).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("symbolic links are not followed".into());
            }
            if !metadata.is_dir() {
                return Err("not a directory".into());
            }
            let path = path.canonicalize().map_err(|e| e.to_string())?;
            if path
                .file_name()
                .is_some_and(|n| n == ".git" || options.excludes.iter().any(|e| n == e))
            {
                return Ok(());
            }
            let remaining = options.max_depth.saturating_sub(depth);
            if visited.get(&path).is_some_and(|seen| *seen >= remaining) {
                return Ok(());
            }
            visited.insert(path.clone(), remaining);
            let marker = path.join(".git");
            let git_marker = fs::symlink_metadata(&marker).is_ok();
            let bare_marker = path.join("HEAD").is_file()
                && path.join("objects").is_dir()
                && path.join("refs").is_dir();
            if git_marker || bare_marker {
                match inspect(&path) {
                    Ok(None) => return Ok(()),
                    Ok(Some(repo)) => {
                        let is_bare = repo.bare;
                        let rank =
                            |repo: &Repository| (repo.bare, !repo.main_worktree, repo.path.clone());
                        if repositories
                            .get(&repo.common_git_dir)
                            .is_none_or(|old| rank(&repo) < rank(old))
                        {
                            repositories.insert(repo.common_git_dir.clone(), repo);
                        }
                        // Never recurse through a bare repository's object storage.
                        if is_bare {
                            return Ok(());
                        }
                    }
                    Err(message) => {
                        if errors_seen.insert(path.clone()) {
                            result.errors.push(DiscoveryError {
                                path: path.clone(),
                                message,
                            });
                        }
                        // An invalid Git directory might be metadata; do not descend.
                        return Ok(());
                    }
                }
            }
            if depth == options.max_depth {
                return Ok(());
            }
            let mut children = Vec::new();
            for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_dir() {
                    children.push(entry.path());
                }
            }
            children.sort();
            pending.extend(children.into_iter().rev().map(|p| (p, depth + 1)));
            Ok(())
        })();
        if let Err(message) = visit
            && errors_seen.insert(path.clone())
        {
            result.errors.push(DiscoveryError { path, message });
        }
    }
    result.repositories = repositories.into_values().collect();
    result.repositories.sort_by(|a, b| a.path.cmp(&b.path));
    result
}

pub fn run(options: &DiscoveryOptions, out: &mut impl Write) -> Result<ExitCode, String> {
    let result = discover(options);
    for repo in &result.repositories {
        writeln!(
            out,
            "{}: {:?}",
            if repo.bare { "bare" } else { "repository" },
            repo.path
        )
        .map_err(|e| e.to_string())?;
    }
    for error in &result.errors {
        eprintln!("giclean: {:?}: {}", error.path, error.message);
    }
    Ok(if result.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn metadata_path_only_removes_trailing_separators() {
        for (input, expected) in [
            ("/", "/"),
            ("//", "//"),
            ("", ""),
            ("alias/", "alias"),
            ("alias///", "alias"),
            ("a/../alias//", "a/../alias"),
            ("a//./alias//", "a//./alias"),
            ("alias/./", "alias"),
            ("alias/././", "alias"),
            ("/./", "/"),
            ("./", "."),
            ("alias/.././", "alias/.."),
        ] {
            assert_eq!(
                metadata_path(Path::new(input)).as_os_str(),
                OsString::from(expected)
            );
        }
    }
}
