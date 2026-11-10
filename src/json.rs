//! Dependency-free JSON presentation. Never execute Git or infer a decision here.
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::decision::{Decision, Eligibility, ExecutionResult, Outcome, Target};

pub fn render(
    out: &mut impl Write,
    command: &str,
    dry_run: bool,
    decisions: &[Decision],
    executions: &[ExecutionResult],
    success: bool,
    error: Option<&str>,
) -> io::Result<()> {
    write!(out, "{{\"schema_version\":1,\"command\":")?;
    string(out, command)?;
    write!(
        out,
        ",\"dry_run\":{dry_run},\"success\":{success},\"decisions\":["
    )?;
    for (index, decision) in decisions.iter().enumerate() {
        if index > 0 {
            write!(out, ",")?;
        }
        write!(out, "{{\"target\":")?;
        target(out, &decision.target)?;
        write!(out, ",\"eligibility\":")?;
        string(
            out,
            match decision.eligibility() {
                Eligibility::Deletable => "deletable",
                Eligibility::Protected => "protected",
                Eligibility::Unknown => "unknown",
            },
        )?;
        write!(out, ",\"reason_code\":")?;
        string(out, decision.reason.as_str())?;
        write!(out, ",\"inspected_at\":")?;
        timestamp(out, Some(decision.inspected_at))?;
        write!(out, ",\"evidence\":{{\"merged_at\":")?;
        timestamp(out, decision.evidence.merged_at)?;
        write!(out, ",\"latest_activity\":")?;
        timestamp(out, decision.evidence.latest_activity)?;
        write!(out, ",\"cutoff\":")?;
        timestamp(out, decision.evidence.cutoff)?;
        write!(out, "}}")?;
        write!(out, ",\"detail\":")?;
        optional_string(out, decision.detail.as_deref())?;
        write!(out, "}}")?;
    }
    write!(out, "],\"executions\":[")?;
    for (index, result) in executions.iter().enumerate() {
        if index > 0 {
            write!(out, ",")?;
        }
        write!(out, "{{\"target\":")?;
        target(out, &result.target)?;
        write!(out, ",\"outcome\":")?;
        let (code, detail) = match &result.outcome {
            Outcome::Candidate => ("candidate", None),
            Outcome::Deleted => ("deleted", None),
            Outcome::Failed(detail) => ("failed", Some(detail.as_str())),
            Outcome::Skipped => ("skipped", None),
            Outcome::NotAttempted => ("not_attempted", None),
        };
        string(out, code)?;
        write!(out, ",\"reason_code\":")?;
        optional_string(
            out,
            if detail.is_some() {
                Some("deletion_failed")
            } else {
                None
            },
        )?;
        write!(out, ",\"detail\":")?;
        optional_string(out, detail)?;
        write!(out, "}}")?;
    }
    write!(out, "],\"error\":")?;
    match error {
        Some(message) => {
            write!(out, "{{\"code\":\"cleanup_failed\",\"message\":")?;
            string(out, message)?;
            write!(out, "}}")?;
        }
        None => write!(out, "null")?,
    }
    writeln!(out, "}}")
}

fn timestamp(out: &mut impl Write, value: Option<SystemTime>) -> io::Result<()> {
    match value.and_then(|value| value.duration_since(UNIX_EPOCH).ok()) {
        Some(time) => write!(
            out,
            "{{\"unix_seconds\":{},\"nanoseconds\":{}}}",
            time.as_secs(),
            time.subsec_nanos()
        ),
        None => write!(out, "null"),
    }
}

fn target(out: &mut impl Write, target: &Target) -> io::Result<()> {
    match target {
        Target::Branch { name, oid } => {
            write!(out, "{{\"kind\":\"branch\",\"name\":")?;
            string(out, name)?;
            write!(out, ",\"oid\":")?;
            string(out, oid)?;
        }
        Target::Worktree { path, branch, oid } => {
            write!(out, "{{\"kind\":\"worktree\",\"path\":")?;
            string(out, &path.to_string_lossy())?;
            write!(out, ",\"branch\":")?;
            optional_string(out, branch.as_deref())?;
            write!(out, ",\"oid\":")?;
            string(out, oid)?;
        }
    }
    write!(out, "}}")
}

fn optional_string(out: &mut impl Write, value: Option<&str>) -> io::Result<()> {
    match value {
        Some(value) => string(out, value),
        None => write!(out, "null"),
    }
}

fn string(out: &mut impl Write, value: &str) -> io::Result<()> {
    write!(out, "\"")?;
    for character in value.chars() {
        match character {
            '"' => write!(out, "\\\"")?,
            '\\' => write!(out, "\\\\")?,
            '\n' => write!(out, "\\n")?,
            '\r' => write!(out, "\\r")?,
            '\t' => write!(out, "\\t")?,
            '\u{0}'..='\u{1f}' => write!(out, "\\u{:04x}", character as u32)?,
            _ => write!(out, "{character}")?,
        }
    }
    write!(out, "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_all_controls_quotes_backslashes_and_preserves_unicode() {
        let mut out = Vec::new();
        string(&mut out, "\"\\\n\r\t\0\u{8}\u{c}\u{1f}日本語🦀").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"\\\"\\\\\\n\\r\\t\\u0000\\u0008\\u000c\\u001f日本語🦀\""
        );
    }
}
