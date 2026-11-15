//! Parse only the prefix before the subcommand, keeping flag-shaped values intact.
use std::ffi::OsString;

pub fn prepare(
    args: Vec<OsString>,
) -> Result<(Vec<OsString>, Option<crate::audit::Audit>), String> {
    let mut config = None;
    let mut audit = None;
    let mut iter = args.into_iter().peekable();
    while let Some(flag) = iter.peek().and_then(|a| a.to_str()) {
        let slot = match flag {
            "--config" => &mut config,
            "--audit-log" => &mut audit,
            _ => break,
        };
        let flag = iter.next().unwrap();
        if slot.is_some() {
            return Err(format!("specify {} only once", flag.to_string_lossy()));
        }
        *slot = Some(
            iter.next()
                .filter(|a| !a.is_empty())
                .ok_or("global option requires a file path")?,
        );
    }
    let mut args: Vec<_> = iter.collect();
    if let Some(path) = config {
        args.splice(0..0, [OsString::from("--config"), path]);
        args = crate::config::resolve(args)?;
    }
    if let Some(path) = audit {
        args.splice(0..0, [OsString::from("--audit-log"), path]);
    }
    // No mutation has run. A failed audit setup still aborts before dispatch.
    crate::audit::prepare(args)
}
