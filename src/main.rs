use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command, ExitCode, Stdio};

mod ssh;

const HELP: &str = "giclean — Git のお掃除コマンド

使い方: giclean <command> [options]

コマンド:
  br             マージ済みのローカルブランチを削除

オプション:
  -h, --help     ヘルプを表示
  -V, --version  バージョンを表示

各コマンドの使い方は giclean <command> --help で確認できます。
";

const BRANCHES_HELP: &str = "giclean br — マージ済みのローカルブランチを削除

使い方: giclean br [--dry-run]

origin の最新の既定ブランチにマージ済みのローカルブランチを削除します。

オプション:
  --dry-run   削除候補だけを表示
  -h, --help  ヘルプを表示

通常実行は確認なしで削除します。まず --dry-run で確認してください。
自動 fetch は行いません。GitHub CLI は不要です。
";

const BRANCH_FORMAT: &str = "--format=%(refname)%09%(objectname)%09%(HEAD)%09%(if)%(worktreepath)%(then)used%(else)free%(end)%09%(symref)";

enum Action {
    Branches { dry_run: bool },
    Help,
    BranchesHelp,
    Version,
}

#[derive(Debug)]
struct Branch {
    name: String,
    oid: String,
    in_use: bool,
    symbolic: bool,
}

impl Branch {
    fn protected(&self) -> bool {
        self.in_use
            || self.symbolic
            || matches!(self.name.as_str(), "main" | "master" | "production")
    }
}

struct RemoteHead {
    branch: String,
    oid: String,
}

fn main() -> ExitCode {
    let action = match parse_args(env::args_os().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("giclean: {error}\n使い方: giclean <command> [options]");
            return ExitCode::from(2);
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let result = match action {
        Action::Help => write!(out, "{HELP}").map_err(|e| e.to_string()),
        Action::BranchesHelp => write!(out, "{BRANCHES_HELP}").map_err(|e| e.to_string()),
        Action::Version => {
            writeln!(out, "giclean {}", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())
        }
        Action::Branches { dry_run } => clean_branches(dry_run, &mut out),
    };
    match result.and_then(|()| out.flush().map_err(|e| e.to_string())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("giclean: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Action, String> {
    let args: Vec<_> = args.collect();
    match args.as_slice() {
        [] => Ok(Action::Help),
        [arg] if arg == "--help" || arg == "-h" => Ok(Action::Help),
        [arg] if arg == "--version" || arg == "-V" => Ok(Action::Version),
        [command] if command == "br" => Ok(Action::Branches { dry_run: false }),
        [command, option] if command == "br" && option == "--dry-run" => {
            Ok(Action::Branches { dry_run: true })
        }
        [command, option] if command == "br" && (option == "--help" || option == "-h") => {
            Ok(Action::BranchesHelp)
        }
        _ => Err(format!(
            "不正な引数: {}",
            args.iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )),
    }
}

fn git(args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        // Unsupported Git versions fail before doing any work, instead of silently
        // ignoring the no-lazy-fetch environment variable.
        .arg("--no-lazy-fetch")
        .args(args)
        // Ancestry must reflect the real commits, not local replacement objects.
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stderr(Stdio::inherit());
    command
}

fn git_text(args: &[&str]) -> Result<String, String> {
    git_command_text(git(args), args[0])
}

fn git_command_text(mut command: Command, operation: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|e| format!("git {operation} を実行できません: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {operation} が失敗しました ({})。",
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "Git の出力に UTF-8 以外の文字が含まれるため停止しました。".into())
}

fn branch_snapshot(
    merged_into: Option<&str>,
    name: Option<&str>,
) -> Result<BTreeMap<String, Branch>, String> {
    let mut args = vec!["for-each-ref", "--sort=refname", BRANCH_FORMAT];
    let merged_arg;
    if let Some(oid) = merged_into {
        merged_arg = format!("--merged={oid}");
        args.push(&merged_arg);
    }
    let reference = name.map_or_else(|| "refs/heads/".to_owned(), |n| format!("refs/heads/{n}"));
    args.push(&reference);
    let output = git_text(&args)?;
    let mut branches = BTreeMap::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 5
            || !fields[0].starts_with("refs/heads/")
            || !valid_oid(fields[1])
            || !matches!(fields[2], "*" | " ")
            || !matches!(fields[3], "used" | "free")
        {
            return Err("ローカルブランチの状態を解釈できないため停止しました。".into());
        }
        let branch = Branch {
            name: fields[0]["refs/heads/".len()..].to_owned(),
            oid: fields[1].to_owned(),
            in_use: fields[2] == "*" || fields[3] == "used",
            symbolic: !fields[4].is_empty(),
        };
        branches.insert(branch.name.clone(), branch);
    }
    Ok(branches)
}

fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid.bytes().all(|b| b.is_ascii_hexdigit())
        && oid.bytes().any(|b| b != b'0')
}

fn parse_remote_head(output: &str) -> Result<RemoteHead, String> {
    let mut branch = None;
    let mut oid = None;
    for line in output.lines() {
        let Some((value, reference)) = line.split_once('\t') else {
            return Err("origin の参照情報を解釈できないため停止しました。".into());
        };
        if reference != "HEAD" {
            continue;
        }
        if let Some(name) = value.strip_prefix("ref: refs/heads/") {
            if name.is_empty() || branch.replace(name.to_owned()).is_some() {
                return Err("origin の既定ブランチを一意に取得できません。".into());
            }
        } else if valid_oid(value) {
            if oid.replace(value.to_owned()).is_some() {
                return Err("origin の最新コミットを一意に取得できません。".into());
            }
        } else {
            return Err("origin の HEAD が通常のブランチを指していません。".into());
        }
    }
    match (branch, oid) {
        (Some(branch), Some(oid)) => Ok(RemoteHead { branch, oid }),
        _ => Err("origin の既定ブランチと最新コミットを取得できません。リモートの HEAD を確認してください。".into()),
    }
}

fn clean_branches(dry_run: bool, out: &mut impl Write) -> Result<(), String> {
    eprintln!("giclean: ローカルブランチを確認しています...");
    if git_text(&["rev-parse", "--is-inside-work-tree"])?.trim() != "true" {
        return Err("作業ツリーのある Git リポジトリ内で実行してください。".into());
    }
    let initial: BTreeMap<_, _> = branch_snapshot(None, None)?
        .into_iter()
        .filter(|(_, branch)| !branch.protected())
        .collect();
    if initial.is_empty() {
        eprintln!("giclean: 対象ブランチはありません。");
        if !dry_run {
            writeln!(out, "deleted: 0").map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    // Require a configured remote, rather than accidentally treating "origin" as a path.
    let origin = git_text(&["remote", "get-url", "origin"])?;
    eprintln!("giclean: origin の既定ブランチと最新コミットを確認しています...");
    let mut query = git(&["ls-remote", "--symref", "--exit-code", "origin", "HEAD"]);
    ssh::configure(&mut query, origin.trim());
    let remote = parse_remote_head(&git_command_text(query, "ls-remote")?)?;
    git_text(&["check-ref-format", &format!("refs/heads/{}", remote.branch)])?;
    let commit = git(&["cat-file", "-t", &remote.oid])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("基準コミットを確認できません: {e}"))?;
    if !commit.status.success() || commit.stdout != b"commit\n" {
        return Err(format!(
            "origin/{} の最新コミット ({}) がローカルにありません。git fetch origin を実行してから再実行してください。",
            remote.branch, remote.oid
        ));
    }

    eprintln!("giclean: マージ済みブランチを判定しています...");
    let merged = branch_snapshot(Some(&remote.oid), None)?;
    let mut deleted = 0;
    let mut failures = 0;
    for (name, branch) in merged {
        if branch.protected()
            || name == remote.branch
            || initial.get(&name).is_none_or(|old| old.oid != branch.oid)
        {
            continue;
        }
        // Recheck each branch immediately before acting, including changes made after
        // the remote query. Git also refuses to delete branches checked out in worktrees.
        let latest = match branch_snapshot(None, Some(&name)) {
            Ok(latest) => latest,
            Err(error) => {
                if !dry_run {
                    writeln!(out, "deleted: {deleted}").map_err(|e| e.to_string())?;
                }
                return Err(error);
            }
        };
        if latest
            .get(&name)
            .is_none_or(|current| current.protected() || current.oid != branch.oid)
        {
            continue;
        }
        if dry_run {
            writeln!(out, "candidate: {name}").map_err(|e| e.to_string())?;
        } else {
            match git(&["branch", "-D", "--quiet", "--", &name]).status() {
                Ok(status) if status.success() => deleted += 1,
                Ok(status) => {
                    failures += 1;
                    eprintln!("giclean: {name} の削除に失敗しました ({status})。");
                }
                Err(error) => {
                    failures += 1;
                    eprintln!("giclean: {name} を削除できません: {error}");
                }
            }
        }
    }
    if dry_run {
        eprintln!("giclean: 確認が完了しました。");
    } else {
        writeln!(out, "deleted: {deleted}").map_err(|e| e.to_string())?;
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!("{failures} 件のブランチを削除できませんでした。"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_head_requires_both_a_symbolic_branch_and_a_valid_oid() {
        let oid = "a".repeat(40);
        let parsed =
            parse_remote_head(&format!("ref: refs/heads/trunk\tHEAD\n{oid}\tHEAD\n")).unwrap();
        assert_eq!(parsed.branch, "trunk");
        assert_eq!(parsed.oid, oid);
        for invalid in [
            String::new(),
            format!("{oid}\tHEAD\n"),
            "ref: refs/heads/main\tHEAD\n".into(),
            format!("ref: refs/tags/v1\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\n{oid}\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\nref: refs/heads/other\tHEAD\n{oid}\tHEAD\n"),
            format!("ref: refs/heads/main\tHEAD\n{}\tHEAD\n", "0".repeat(40)),
            "ref: refs/heads/main\tHEAD\ninvalid\tHEAD\n".into(),
        ] {
            assert!(parse_remote_head(&invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn remote_head_ignores_refs_whose_names_only_end_in_head() {
        let oid = "a".repeat(64);
        let parsed = parse_remote_head(&format!(
            "ref: refs/heads/main\tHEAD\n{oid}\tHEAD\n{}\trefs/heads/HEAD\n",
            "b".repeat(64)
        ))
        .unwrap();
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.oid, oid);
    }
}
