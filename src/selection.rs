//! Explicit terminal selection; reuse the normal dry-run and execution safeguards.
use crate::{Action, Outcome, OutputFormat, RunReport, Target};
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

pub fn hook(args: &[OsString], report: &mut RunReport) -> Option<ExitCode> {
    if interactive_positions(args).is_empty() {
        return None;
    }
    let result = run(args, report);
    report.finish(&result);
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("giclean: {error}");
            ExitCode::FAILURE
        }
    })
}

// A path/value spelled --interactive is not an option. Do not reinterpret other
// commands, whose arguments may also be literal paths.
fn interactive_positions(args: &[OsString]) -> Vec<usize> {
    if !args.first().is_some_and(|arg| arg == "br" || arg == "wt") {
        return vec![];
    }
    let worktrees = args[0] == "wt";
    let mut positions = vec![];
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].to_str();
        if arg == Some("--interactive") {
            positions.push(index);
        }
        let takes_value = arg == Some("--format")
            || (worktrees
                && matches!(
                    arg,
                    Some("-d" | "--days" | "-h" | "--hours" | "--allow-ignored")
                ));
        index += if takes_value { 2 } else { 1 };
    }
    positions
}

fn run(args: &[OsString], report: &mut RunReport) -> Result<ExitCode, String> {
    let positions = interactive_positions(args);
    if positions.len() != 1 {
        return Err("specify --interactive only once".into());
    }
    let (action, format) = crate::parse_options(
        args.iter()
            .enumerate()
            .filter(|(i, _)| !positions.contains(i))
            .map(|(_, arg)| arg.clone()),
    )?;
    if format != OutputFormat::Text
        || !matches!(
            action,
            Action::Branches { dry_run: false, .. } | Action::Worktrees { dry_run: false, .. }
        )
    {
        return Err("--interactive requires br or wt without --dry-run or --format json".into());
    }
    // Fail before inspection or any mutation when launched by a pipe/script.
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("--interactive requires terminal stdin and stdout".into());
    }
    let mut plan = RunReport::default();
    let status = execute(&action, true, None, &mut io::sink(), &mut plan)?;
    if status != ExitCode::SUCCESS {
        return Err("candidate inspection failed; nothing selected".into());
    }
    let candidates: Vec<_> = plan
        .results
        .iter()
        .filter(|result| result.outcome == Outcome::Candidate)
        .map(|result| result.target.clone())
        .collect();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if candidates.is_empty() {
        writeln!(out, "No deletion candidates.").map_err(|e| e.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    let selected =
        choose(&candidates, &mut io::stdin().lock(), &mut out).map_err(|e| e.to_string())?;
    if selected.is_empty() {
        writeln!(out, "Cancelled; nothing deleted.").map_err(|e| e.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    // Fresh inspection plus the existing immediate checks; identities include OIDs.
    let result = execute(&action, false, Some(&selected), &mut out, report);
    report.finish(&result);
    if matches!(
        action,
        Action::Branches { explain: true, .. } | Action::Worktrees { explain: true, .. }
    ) {
        crate::explain::write(report, &mut out).map_err(|e| e.to_string())?;
    }
    result
}

fn execute(
    action: &Action,
    dry_run: bool,
    selected: Option<&[Target]>,
    out: &mut impl Write,
    report: &mut RunReport,
) -> Result<ExitCode, String> {
    match action {
        Action::Branches { .. } => crate::execute_branches(dry_run, selected, out, report),
        Action::Worktrees {
            age,
            include_detached,
            allow_ignored,
            ..
        } => crate::worktrees::execute(
            *age,
            dry_run,
            *include_detached,
            allow_ignored,
            selected,
            out,
            report,
        ),
        _ => unreachable!("validated interactive action"),
    }
}

fn choose(
    targets: &[Target],
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> io::Result<Vec<Target>> {
    for (index, target) in targets.iter().enumerate() {
        // Debug escaping prevents filenames from injecting terminal controls/new lines.
        let name = match target {
            Target::Branch { name, .. } => format!("{name:?}"),
            Target::Worktree { path, .. } => format!("{:?}", path.as_os_str()),
        };
        writeln!(out, "{}. {name}", index + 1)?;
    }
    write!(
        out,
        "Select numbers separated by spaces (Enter/q cancels): "
    )?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 || line.trim().is_empty() || line.trim() == "q" {
        return Ok(vec![]);
    }
    let mut selected = Vec::new();
    for word in line.split_whitespace() {
        let index = word
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|n| *n < targets.len())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid selection; nothing deleted",
                )
            })?;
        if !selected.contains(&targets[index]) {
            selected.push(targets[index].clone());
        }
    }
    write!(
        out,
        "Delete {} selected candidate(s)? Type yes: ",
        selected.len()
    )?;
    out.flush()?;
    line.clear();
    if input.read_line(&mut line)? == 0 || line.trim() != "yes" {
        return Ok(vec![]);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn targets() -> Vec<Target> {
        ["a", "b"]
            .iter()
            .map(|name| Target::Branch {
                name: name.to_string(),
                oid: "abc".into(),
            })
            .collect()
    }
    #[test]
    fn option_values_are_not_interactive_flags() {
        for option in [
            "--format",
            "-d",
            "--days",
            "-h",
            "--hours",
            "--allow-ignored",
        ] {
            let args: Vec<OsString> = ["wt", option, "--interactive"]
                .into_iter()
                .map(Into::into)
                .collect();
            assert!(interactive_positions(&args).is_empty(), "{option}");
            let mut args = args;
            args.push("--interactive".into());
            assert_eq!(interactive_positions(&args), vec![3]);
        }
        let args: Vec<OsString> = ["repos", "--interactive"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert!(interactive_positions(&args).is_empty());
    }
    #[test]
    fn selects_only_requested_and_deduplicates() {
        let t = targets();
        assert_eq!(
            choose(&t, &mut &b"2 2\nyes\n"[..], &mut vec![]).unwrap(),
            vec![t[1].clone()]
        );
    }
    #[test]
    fn eof_empty_and_decline_cancel() {
        for input in ["", "\n", "q\n", "1\n", "1\nno\n"] {
            assert!(
                choose(&targets(), &mut input.as_bytes(), &mut vec![])
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn invalid_selection_fails_closed() {
        for input in ["0\nyes\n", "3\nyes\n", "1 nope\nyes\n"] {
            assert!(choose(&targets(), &mut input.as_bytes(), &mut vec![]).is_err());
        }
    }
}
