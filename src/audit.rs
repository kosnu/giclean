//! Opt-in audit records. Never serialize paths, names, arguments, or diagnostics.
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::decision::{Outcome, RunReport, Target};

pub struct Audit {
    file: File,
    detailed: bool,
}

pub fn prepare(args: Vec<OsString>) -> Result<(Vec<OsString>, Option<Audit>), String> {
    if args.first().is_none_or(|a| a != "--audit-log") {
        return Ok((args, None));
    }
    let mut iter = args.into_iter().skip(1);
    let path = iter
        .next()
        .filter(|p| !p.is_empty())
        .ok_or("--audit-log requires a new file path")?;
    let filtered: Vec<_> = iter.collect();
    let command = filtered.first().and_then(|a| a.to_str()).unwrap_or("");
    if !matches!(command, "br" | "wt" | "wt-prune" | "batch") {
        return Err("--audit-log supports br, wt, wt-prune and batch only".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("cannot create new audit log: {e}"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let dry_run = filtered.iter().any(|a| a == "--dry-run");
    writeln!(file, "{{\"schema_version\":1,\"event\":\"started\",\"unix_seconds\":{now},\"command\":\"{command}\",\"dry_run_requested\":{dry_run}}}")
        .and_then(|()| file.sync_all()).map_err(|e| format!("cannot initialize audit log: {e}"))?;
    let audit = Audit {
        file,
        detailed: matches!(command, "br" | "wt"),
    };
    Ok((filtered, Some(audit)))
}

impl Audit {
    pub fn finish(&mut self, success: bool, report: &RunReport) -> io::Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        write!(
            self.file,
            "{{\"schema_version\":1,\"event\":\"completed\",\"unix_seconds\":{now},\"success\":{success},\"target_results_available\":{},\"results\":[",
            self.detailed
        )?;
        for (index, result) in report.results.iter().enumerate() {
            if index > 0 {
                write!(self.file, ",")?;
            }
            let (kind, oid) = match &result.target {
                Target::Branch { oid, .. } => ("branch", oid),
                Target::Worktree { oid, .. } => ("worktree", oid),
            };
            let outcome = match result.outcome {
                Outcome::NotAttempted => "not_attempted",
                Outcome::Candidate => "candidate",
                Outcome::Skipped => "skipped",
                Outcome::Deleted => "deleted",
                Outcome::Failed(_) => "failed",
            };
            // Only validated object IDs, never arbitrary Git output, enter the log.
            let oid = if matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
            {
                oid.as_str()
            } else {
                ""
            };
            write!(
                self.file,
                "{{\"kind\":\"{kind}\",\"oid\":\"{oid}\",\"outcome\":\"{outcome}\"}}"
            )?;
        }
        writeln!(self.file, "]}}")?;
        self.file.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_is_prefix_only_and_does_not_consume_command_values() {
        let args: Vec<OsString> = ["wt", "--allow-ignored", "--audit-log"]
            .into_iter()
            .map(Into::into)
            .collect();
        let (filtered, audit) = prepare(args.clone()).unwrap();
        assert_eq!(filtered, args);
        assert!(audit.is_none());
        assert!(prepare(vec!["--audit-log".into()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn late_write_failure_is_returned_not_reported_as_success() {
        let mut audit = Audit {
            file: File::open("/dev/null").unwrap(),
            detailed: true,
        };
        assert!(audit.finish(true, &RunReport::default()).is_err());
    }
}
