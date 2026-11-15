//! Explicit, data-only defaults. Never discover configuration in a repository.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Config {
    paths: Vec<OsString>,
    excludes: Vec<OsString>,
    depth: Option<OsString>,
    age: Option<(OsString, OsString)>,
}

impl Config {
    fn parse(text: &str, directory: &Path) -> Result<Self, String> {
        let mut config = Self::default();
        for (line, source) in text.lines().enumerate() {
            let source = source.trim();
            if source.is_empty() || source.starts_with('#') {
                continue;
            }
            let invalid = || format!("invalid configuration at line {}", line + 1);
            let (key, value) = source.split_once('=').ok_or_else(invalid)?;
            let value = value.trim();
            if value.is_empty() || value.chars().any(char::is_control) {
                return Err(invalid());
            }
            match key.trim() {
                "path" => {
                    let path = Path::new(value);
                    config.paths.push(directory.join(path).into_os_string());
                }
                "exclude" => {
                    // Reuse the CLI validator; exclusions remain exact basenames.
                    crate::discovery::parse(&["--exclude".into(), value.into()])?;
                    config.excludes.push(value.into());
                }
                "max-depth" if config.depth.is_none() => {
                    crate::discovery::parse(&["--max-depth".into(), value.into()])?;
                    config.depth = Some(value.into());
                }
                "wt-days" | "wt-hours" if config.age.is_none() => {
                    let flag = if key.trim() == "wt-days" {
                        "--days"
                    } else {
                        "--hours"
                    };
                    crate::worktrees::parse_args(&[flag.into(), value.into()])?;
                    config.age = Some((flag.into(), value.into()));
                }
                _ => return Err(invalid()),
            }
        }
        Ok(config)
    }

    fn apply(self, mut args: Vec<OsString>) -> Result<Vec<OsString>, String> {
        let command = args.first().and_then(|s| s.to_str());
        let discovery = matches!(command, Some("repos" | "batch"));
        let wt = command == Some("wt")
            || (command == Some("batch") && args.get(1).is_some_and(|s| s == "wt"));
        let start = if command == Some("batch") { 2 } else { 1 };
        // Help must keep its exact existing shape, and must not turn into a run.
        if !discovery && !wt
            || args
                .get(start)
                .is_some_and(|s| s == "--help" || (!wt && s == "-h"))
            || (command == Some("batch") && args.len() <= start)
        {
            return Ok(args);
        }
        let mut depth = false;
        let mut excludes = false;
        let mut age = false;
        let mut paths = false;
        let mut i = start;
        while i < args.len() {
            let takes_value = matches!(
                args[i].to_str(),
                Some("--max-depth" | "--exclude" | "--format" | "--allow-ignored")
            ) || (wt
                && matches!(args[i].to_str(), Some("--days" | "--hours" | "-d" | "-h")));
            if takes_value && i + 1 == args.len() {
                return Err("CLI option requires a value".into());
            }
            match args[i].to_str() {
                Some("--") => {
                    paths |= i + 1 < args.len();
                    break;
                }
                Some("--max-depth") => {
                    depth = true;
                    i += 1;
                }
                Some("--exclude") => {
                    excludes = true;
                    i += 1;
                }
                Some("--days" | "--hours" | "-d" | "-h") if wt => {
                    age = true;
                    i += 1;
                }
                Some("--format" | "--allow-ignored") => {
                    i += 1;
                }
                Some(s) if s.starts_with('-') => {}
                _ => {
                    paths = true;
                }
            }
            i += 1;
        }
        let mut defaults = Vec::new();
        if discovery {
            if !depth && let Some(value) = self.depth {
                defaults.extend(["--max-depth".into(), value]);
            }
            if !excludes {
                for value in self.excludes {
                    defaults.extend(["--exclude".into(), value]);
                }
            }
        }
        if wt
            && !age
            && let Some((flag, value)) = self.age
        {
            defaults.extend([flag, value]);
        }
        // Place defaults before any `--`, and roots after the user's options.
        args.splice(start.min(args.len())..start.min(args.len()), defaults);
        if discovery && !paths && !self.paths.is_empty() {
            if !args.iter().any(|s| s == "--") {
                args.push("--".into());
            }
            args.extend(self.paths);
        }
        Ok(args)
    }
}

pub fn resolve(mut args: Vec<OsString>) -> Result<Vec<OsString>, String> {
    if args.first().is_none_or(|arg| arg != "--config") {
        return Ok(args);
    }
    if args.len() < 3 || args[1].is_empty() {
        return Err("use --config PATH <command> [options]".into());
    }
    let path = PathBuf::from(&args[1]);
    // Canonicalization anchors relative config roots to the selected file, not cwd.
    let path = path
        .canonicalize()
        .map_err(|e| format!("cannot read configuration: {e}"))?;
    if !path.is_file() {
        return Err("configuration must be a regular file".into());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read configuration: {e}"))?;
    let config = Config::parse(&text, path.parent().ok_or("configuration has no parent")?)?;
    args.drain(..2);
    config.apply(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn apply(config: &str, args: &[&str]) -> Vec<OsString> {
        Config::parse(config, Path::new("/config"))
            .unwrap()
            .apply(args.iter().map(OsString::from).collect())
            .unwrap()
    }
    #[test]
    fn explicit_defaults_override_each_field() {
        let args = apply(
            "path=other\nmax-depth=9\nexclude=keep\nwt-days=5",
            &[
                "batch",
                "wt",
                "--dry-run",
                "--max-depth",
                "3",
                "--exclude",
                "skip",
                "--hours",
                "24",
                ".",
            ],
        );
        assert_eq!(
            args,
            [
                "batch",
                "wt",
                "--dry-run",
                "--max-depth",
                "3",
                "--exclude",
                "skip",
                "--hours",
                "24",
                "."
            ]
            .map(OsString::from)
        );
    }
    #[test]
    fn applies_only_relevant_defaults() {
        let config = "path=repo with spaces\nexclude=keep\nmax-depth=0\nwt-days=5";
        assert_eq!(
            apply(config, &["wt", "--dry-run"]),
            ["wt", "--days", "5", "--dry-run"].map(OsString::from)
        );
        for args in [
            &["br", "--dry-run"][..],
            &["wt-prune", "--dry-run"],
            &["wt", "--help"],
            &["batch", "wt", "--help"],
        ] {
            assert_eq!(
                apply(config, args),
                args.iter().map(OsString::from).collect::<Vec<_>>()
            );
        }
        assert_eq!(
            apply(config, &["repos"]),
            [
                "repos",
                "--max-depth",
                "0",
                "--exclude",
                "keep",
                "--",
                "/config/repo with spaces"
            ]
            .map(OsString::from)
        );
    }
    #[test]
    fn values_are_not_flags_and_separator_is_respected() {
        assert_eq!(
            apply("wt-days=5", &["wt", "--allow-ignored", "--hours"]),
            ["wt", "--days", "5", "--allow-ignored", "--hours"].map(OsString::from)
        );
        assert_eq!(
            apply("path=unused\nexclude=keep", &["repos", "--", "--exclude"]),
            ["repos", "--exclude", "keep", "--", "--exclude"].map(OsString::from)
        );
    }
    #[test]
    fn missing_cli_values_are_not_filled_by_defaults() {
        for flag in ["--exclude", "--max-depth", "--format"] {
            let config = Config::parse("path=repo", Path::new("/config")).unwrap();
            assert!(config.apply(vec!["repos".into(), flag.into()]).is_err());
        }
    }
    #[test]
    fn rejects_ambiguous_or_invalid_configuration() {
        for text in [
            "bad=true",
            "max-depth=1\nmax-depth=2",
            "wt-days=2\nwt-hours=2",
            "wt-days=0",
            "wt-hours=-1",
            "wt-days=18446744073709551615",
            "max-depth=x",
            "exclude=../keep",
            "path=",
            "execute=true",
            "allow-ignored=target",
            "path=x\0y",
        ] {
            assert!(Config::parse(text, Path::new("/config")).is_err(), "{text}");
        }
    }
}
