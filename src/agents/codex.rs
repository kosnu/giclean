//! Conservative Codex protection without opening session or credential contents.
//!
//! This first detector has no complete live observation source. An existing
//! Codex home therefore vetoes every worktree as unknown, irrespective of its
//! age, contents, or session format. Only an absent default home can be clear
//! within this local home-discovery scope. Explicit invalid settings fail closed.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::{AgentDetector, AgentState, DetectionFailure};

pub struct CodexDetector;

impl AgentDetector for CodexDetector {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn detect(&self, worktree: &Path) -> Result<AgentState, DetectionFailure> {
        if !worktree.is_absolute() {
            return Err(DetectionFailure::PathUncertain);
        }
        // Resolve configuration again on each observation. Never run Codex or
        // start a new app server: its empty loaded list would prove nothing.
        inspect_home(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"))
    }
}

fn inspect_home(
    explicit: Option<OsString>,
    user_home: Option<OsString>,
) -> Result<AgentState, DetectionFailure> {
    let configured = explicit.is_some();
    let path = if let Some(value) = explicit {
        let path = std::path::PathBuf::from(value);
        if !path.is_absolute() {
            return Err(DetectionFailure::PathUncertain);
        }
        path
    } else {
        let home = user_home.ok_or(DetectionFailure::Unavailable)?;
        let home = Path::new(&home);
        if !home.is_absolute() {
            return Err(DetectionFailure::PathUncertain);
        }
        // A broken/inaccessible HOME must not make .codex look uninstalled.
        if !fs::metadata(home)
            .map_err(|_| DetectionFailure::ReadFailed)?
            .is_dir()
        {
            return Err(DetectionFailure::InvalidData);
        }
        home.join(".codex")
    };
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DetectionFailure::PathUncertain),
        Ok(metadata) if !metadata.is_dir() => Err(DetectionFailure::InvalidData),
        Ok(_) => Err(DetectionFailure::IncompleteCoverage),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !configured => {
            Ok(AgentState::NoReference)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(DetectionFailure::Unavailable)
        }
        Err(_) => Err(DetectionFailure::ReadFailed),
    }
}
