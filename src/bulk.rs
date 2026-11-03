use std::collections::BTreeMap;
use std::process::{ExitCode, Output};

use crate::{Branch, Progress};

pub enum Outcome {
    Individual,
    Deleted,
    Retry(Output),
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::env;
    use std::ffi::OsString;
    use std::fs::{self, DirBuilder};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    const DIRECTORY: &str = "GICLEAN_GUARD_DIRECTORY";
    const PROGRAM: &str = "GICLEAN_GUARD_GIT";
    const FORMAT: &str = "--format=%(refname)%09%(objectname)%09%(if)%(HEAD)%(then)head%(else)other%(end)%09%(if)%(worktreepath)%(then)used%(else)free%(end)%09%(if)%(symref)%(then)symbolic%(else)direct%(end)";

    struct GuardDirectory(PathBuf);

    impl Drop for GuardDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn directory() -> io::Result<GuardDirectory> {
        let root = env::temp_dir();
        let root = if root.is_absolute() {
            root
        } else {
            env::current_dir()?.join(root)
        };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..16 {
            let path = root.join(format!(
                "giclean-guard-{}-{timestamp}-{attempt}",
                std::process::id()
            ));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(GuardDirectory(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot create private guard directory",
        ))
    }

    // Git changes PATH before invoking hooks. Resolve the parent's selection
    // before handing it to the hook, including user-provided Git wrappers.
    fn git_program() -> Option<PathBuf> {
        let command = crate::git::command();
        let selected = Path::new(command.get_program());
        if selected.is_absolute() {
            return Some(selected.to_owned());
        }
        let cwd = env::current_dir().ok()?;
        env::split_paths(&env::var_os("PATH")?)
            .map(|directory| cwd.join(directory).join(selected))
            .find(|path| {
                path.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }

    pub fn delete(
        branches: &BTreeMap<String, Branch>,
        progress: &Progress,
    ) -> Result<Outcome, String> {
        // Keep argv and hook input bounded. Larger sets use the existing path.
        if branches.len() < 2
            || branches.len() > 128
            || branches.keys().map(|name| name.len() + 1).sum::<usize>() > 16 * 1024
        {
            return Ok(Outcome::Individual);
        }
        let original = crate::git_text(
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "hooks/reference-transaction",
            ],
            progress,
        )?;
        match fs::metadata(original.trim()) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => {
                return Ok(Outcome::Individual);
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Ok(Outcome::Individual);
            }
            _ => {}
        }
        let Some(program) = git_program() else {
            return Ok(Outcome::Individual);
        };
        let Ok(executable) = env::current_exe() else {
            return Ok(Outcome::Individual);
        };
        let Ok(directory) = directory() else {
            return Ok(Outcome::Individual);
        };
        let expected: String = branches
            .iter()
            .map(|(name, branch)| format!("refs/heads/{name}\t{}\n", branch.oid))
            .collect();
        fs::write(directory.0.join("expected"), expected).map_err(|e| e.to_string())?;
        fs::write(directory.0.join("status"), "pending").map_err(|e| e.to_string())?;
        symlink(executable, directory.0.join("reference-transaction"))
            .map_err(|e| e.to_string())?;
        let mut hooks = OsString::from("core.hooksPath=");
        hooks.push(&directory.0);
        let output = crate::git(&[])
            .arg("-c")
            .arg(hooks)
            .args(["branch", "-D", "--quiet", "--"])
            .args(branches.keys())
            .env(DIRECTORY, &directory.0)
            .env(PROGRAM, program)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("could not run guarded git branch: {e}"))?;
        let status = fs::read_to_string(directory.0.join("status")).map_err(|e| e.to_string())?;
        match status.as_str() {
            "ok" if output.status.success() => Ok(Outcome::Deleted),
            "ok" => Ok(Outcome::Retry(output)),
            "changed" => Ok(Outcome::Individual),
            "pending" if !output.status.success() => {
                // A ref-lock failure can precede prepared. A hook launch failure
                // must stop cleanup rather than bypass a broken safety check.
                let diagnostics = String::from_utf8_lossy(&output.stderr);
                if diagnostics.contains("cannot lock ref ")
                    || diagnostics.contains("packed-refs.lock")
                    || (diagnostics.contains("error: branch '")
                        && diagnostics.contains("' not found"))
                {
                    Ok(Outcome::Individual)
                } else {
                    Err("could not run branch deletion guard".into())
                }
            }
            _ => Err("branch deletion guard could not verify local state".into()),
        }
    }

    fn validate(directory: &Path) -> Result<bool, String> {
        let expected = fs::read_to_string(directory.join("expected")).map_err(|e| e.to_string())?;
        let output = Command::new(env::var_os(PROGRAM).ok_or("missing Git program")?)
            .args([
                "--no-lazy-fetch",
                "for-each-ref",
                "--sort=refname",
                FORMAT,
                "refs/heads/",
            ])
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("cannot read branch state".into());
        }
        let text = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
        let mut current = BTreeMap::new();
        for row in text.lines() {
            let fields: Vec<_> = row.split('\t').collect();
            if fields.len() != 5
                || !fields[0].starts_with("refs/heads/")
                || !crate::valid_oid(fields[1])
                || !matches!(fields[2], "head" | "other")
                || !matches!(fields[3], "used" | "free")
                || !matches!(fields[4], "symbolic" | "direct")
            {
                return Err("invalid branch state".into());
            }
            if current.insert(fields[0], fields).is_some() {
                return Err("duplicate branch state".into());
            }
        }
        if expected.is_empty() {
            return Err("empty guard expectation".into());
        }
        for row in expected.lines() {
            let (name, oid) = row.split_once('\t').ok_or("invalid guard expectation")?;
            if !crate::valid_oid(oid) {
                return Err("invalid guard OID".into());
            }
            if current
                .get(name)
                .is_none_or(|fields| fields.as_slice() != [name, oid, "other", "free", "direct"])
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn hook() -> Option<ExitCode> {
        let directory = env::var_os(DIRECTORY)?;
        let mut args = env::args_os();
        if Path::new(&args.next()?).file_name()? != "reference-transaction" {
            return None;
        }
        let phase = args.next()?;
        if args.next().is_some()
            || !["preparing", "prepared", "committed", "aborted"]
                .iter()
                .any(|p| phase == *p)
        {
            return None;
        }
        let directory = PathBuf::from(directory);
        let run = || -> Result<(), String> {
            if phase == "prepared" {
                fs::write(directory.join("status"), "checking").map_err(|e| e.to_string())?;
            }
            // Drain Git's input even in phases that need no validation.
            io::copy(&mut io::stdin().lock(), &mut io::sink()).map_err(|e| e.to_string())?;
            if phase != "prepared" {
                return Ok(());
            }
            match validate(&directory) {
                Ok(true) => fs::write(directory.join("status"), "ok").map_err(|e| e.to_string()),
                Ok(false) => {
                    fs::write(directory.join("status"), "changed").map_err(|e| e.to_string())?;
                    Err("local branch state changed".into())
                }
                Err(error) => {
                    let _ = fs::write(directory.join("status"), "error");
                    Err(error)
                }
            }
        };
        Some(if run().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    }
}

#[cfg(unix)]
pub use unix::{delete, hook};

#[cfg(not(unix))]
pub fn hook() -> Option<ExitCode> {
    None
}

#[cfg(not(unix))]
pub fn delete(_: &BTreeMap<String, Branch>, _: &Progress) -> Result<Outcome, String> {
    Ok(Outcome::Individual)
}
