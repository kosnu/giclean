use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command, ExitCode, Stdio};

mod progress;
mod ssh;

use progress::Progress;

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
squash は変更した全パスの内容・権限が一致する場合に対象とします。

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

#[derive(Clone, Debug)]
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

fn git_text(args: &[&str], progress: &Progress) -> Result<String, String> {
    git_command_text(git(args), args[0], progress)
}

fn git_command_text(
    mut command: Command,
    operation: &str,
    progress: &Progress,
) -> Result<String, String> {
    let output = progress
        .output(&mut command)
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

type TreeChanges = BTreeMap<Vec<u8>, Vec<u8>>;

// Compare tree objects in one Git process. Each response must have the requested
// pair header, followed by raw NUL records; filenames are never decoded as text.
fn tree_changes(
    pairs: &BTreeSet<(String, String)>,
    progress: &Progress,
) -> Result<BTreeMap<(String, String), TreeChanges>, String> {
    let input: String = pairs
        .iter()
        .map(|(base, tip)| format!("{base} {tip}\n"))
        .collect();
    let mut command = git(&[
        "diff-tree",
        "--stdin",
        "--raw",
        "-r",
        "-z",
        "--no-abbrev",
        "--no-renames",
        "--no-relative",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
    ]);
    let output = progress
        .output_with_input(&mut command, input.as_bytes())
        .map_err(|e| format!("git diff-tree を実行できません: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff-tree が失敗しました ({})。",
            output.status
        ));
    }
    let error = "ツリーの変更を解釈できないため停止しました。";
    let mut remaining = output.stdout.as_slice();
    let mut results = BTreeMap::new();
    for pair in pairs {
        let header = format!("{} {}\n", pair.0, pair.1);
        remaining = remaining.strip_prefix(header.as_bytes()).ok_or(error)?;
        let mut changes = BTreeMap::new();
        while remaining.starts_with(b":") {
            let mut fields = remaining.splitn(3, |byte| *byte == 0);
            let change = fields.next().ok_or(error)?;
            let path = fields.next().filter(|path| !path.is_empty()).ok_or(error)?;
            remaining = fields.next().ok_or(error)?;
            if changes.insert(path.to_vec(), change.to_vec()).is_some() {
                return Err(error.into());
            }
        }
        results.insert(pair.clone(), changes);
    }
    if !remaining.is_empty() {
        return Err(error.into());
    }
    Ok(results)
}

fn squash_included(
    tips: &[&str],
    target: &str,
    progress: &Progress,
) -> Result<BTreeSet<String>, String> {
    // Bound argv size and the number of tree diffs retained at once. All reuse is
    // scoped to this run and immutable object IDs, never branch names or refs.
    const BATCH_SIZE: usize = 128;
    let error = "共通祖先を解釈できないため停止しました。";
    let mut bases = BTreeMap::new();
    for chunk in tips.chunks(BATCH_SIZE) {
        // A...B expands to B, A, and every merge-base prefixed by ^. Git performs
        // the same merge-base --all calculation, without one process per branch.
        let mut command = git(&["rev-parse", "--revs-only", "--end-of-options"]);
        command.args(chunk.iter().map(|tip| format!("{tip}...{target}")));
        let output = git_command_text(command, "rev-parse", progress)?;
        let mut lines = output.lines().peekable();
        for tip in chunk {
            if lines.next() != Some(target) || lines.next() != Some(*tip) {
                return Err(error.into());
            }
            let mut ancestors = Vec::new();
            while let Some(base) = lines.peek().and_then(|line| line.strip_prefix('^')) {
                if !valid_oid(base) {
                    return Err(error.into());
                }
                ancestors.push(base);
                lines.next();
            }
            // Unrelated histories and multiple best ancestors remain ineligible.
            if ancestors.len() == 1 {
                bases.insert((*tip).to_owned(), ancestors[0].to_owned());
            }
        }
        if lines.next().is_some() {
            return Err(error.into());
        }
    }
    if bases.is_empty() {
        return Ok(BTreeSet::new());
    }
    let commits: BTreeSet<_> = bases
        .iter()
        .flat_map(|(tip, base)| [tip.as_str(), base.as_str()])
        .chain(std::iter::once(target))
        .collect();
    let commits: Vec<_> = commits.into_iter().collect();
    let mut trees = BTreeMap::new();
    for chunk in commits.chunks(BATCH_SIZE) {
        let mut command = git(&["rev-parse", "--revs-only", "--end-of-options"]);
        command.args(chunk.iter().map(|commit| format!("{commit}^{{tree}}")));
        let output = git_command_text(command, "rev-parse", progress)?;
        let ids: Vec<_> = output.lines().collect();
        if ids.len() != chunk.len() || ids.iter().any(|oid| !valid_oid(oid)) {
            return Err("ツリーの ID を解釈できないため停止しました。".into());
        }
        trees.extend(
            chunk
                .iter()
                .zip(ids)
                .map(|(commit, tree)| (*commit, tree.to_owned())),
        );
    }
    let mut included = BTreeSet::new();
    let branches: Vec<_> = bases.iter().collect();
    for chunk in branches.chunks(BATCH_SIZE) {
        let pairs: BTreeSet<_> = chunk
            .iter()
            .filter(|(tip, base)| trees[tip.as_str()] != trees[base.as_str()])
            .flat_map(|(tip, base)| {
                [
                    (trees[base.as_str()].clone(), trees[tip.as_str()].clone()),
                    (trees[base.as_str()].clone(), trees[target].clone()),
                ]
            })
            .collect();
        if pairs.is_empty() {
            continue;
        }
        let diffs = tree_changes(&pairs, progress)?;
        for (tip, base) in chunk {
            if trees[tip.as_str()] == trees[base.as_str()] {
                continue;
            }
            let changes = &diffs[&(trees[base.as_str()].clone(), trees[tip.as_str()].clone())];
            let target_changes = &diffs[&(trees[base.as_str()].clone(), trees[target].clone())];
            if !changes.is_empty()
                && changes
                    .iter()
                    .all(|(path, change)| target_changes.get(path) == Some(change))
            {
                included.insert((*tip).clone());
            }
        }
    }
    Ok(included)
}

fn branch_snapshot(
    merged_into: Option<&str>,
    name: Option<&str>,
    progress: &Progress,
) -> Result<BTreeMap<String, Branch>, String> {
    let mut args = vec!["for-each-ref", "--sort=refname", BRANCH_FORMAT];
    let merged_arg;
    if let Some(oid) = merged_into {
        merged_arg = format!("--merged={oid}");
        args.push(&merged_arg);
    }
    let reference = name.map_or_else(|| "refs/heads/".to_owned(), |n| format!("refs/heads/{n}"));
    args.push(&reference);
    let output = git_text(&args, progress)?;
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
    let progress = Progress::start(
        "ローカルブランチを確認中…",
        Some("ローカルブランチを確認しています..."),
    );
    if git_text(&["rev-parse", "--is-inside-work-tree"], &progress)?.trim() != "true" {
        return Err("作業ツリーのある Git リポジトリ内で実行してください。".into());
    }
    let initial: BTreeMap<_, _> = branch_snapshot(None, None, &progress)?
        .into_iter()
        .filter(|(_, branch)| !branch.protected())
        .collect();
    progress.finish("ローカルブランチを確認", None);
    if initial.is_empty() {
        eprintln!("giclean: 対象ブランチはありません。");
        if !dry_run {
            writeln!(out, "deleted: 0").map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    // Require a configured remote, rather than accidentally treating "origin" as a path.
    let progress = Progress::start(
        "origin の既定ブランチと最新コミットを確認中…",
        Some("origin の既定ブランチと最新コミットを確認しています..."),
    );
    let origin = git_text(&["remote", "get-url", "origin"], &progress)?;
    progress.pause_for_input();
    let mut query = git(&["ls-remote", "--symref", "--exit-code", "origin", "HEAD"]);
    ssh::configure(&mut query, origin.trim());
    let remote = parse_remote_head(&git_command_text(query, "ls-remote", &progress)?)?;
    git_text(
        &["check-ref-format", &format!("refs/heads/{}", remote.branch)],
        &progress,
    )?;
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
    progress.finish("origin の既定ブランチと最新コミットを確認", None);

    let progress = Progress::start(
        "マージ済みブランチを判定中…",
        Some("マージ済みブランチを判定しています..."),
    );
    let mut merged = branch_snapshot(Some(&remote.oid), None, &progress)?;
    let tips: BTreeSet<_> = initial
        .iter()
        .filter(|(name, _)| *name != &remote.branch && !merged.contains_key(*name))
        .map(|(_, branch)| branch.oid.as_str())
        .collect();
    let included = squash_included(
        &tips.into_iter().collect::<Vec<_>>(),
        &remote.oid,
        &progress,
    )?;
    for (name, branch) in &initial {
        if name != &remote.branch && !merged.contains_key(name) && included.contains(&branch.oid) {
            merged.insert(name.clone(), branch.clone());
        }
    }
    progress.finish("マージ済みブランチを判定", None);
    let progress = Progress::start(
        if dry_run {
            "削除候補を確認中…"
        } else {
            "ブランチを削除中… 0件"
        },
        None,
    );
    let mut deleted = 0;
    let mut candidates = 0;
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
        let latest = match branch_snapshot(None, Some(&name), &progress) {
            Ok(latest) => latest,
            Err(error) => {
                if !dry_run {
                    progress.suspend();
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
            progress.suspend();
            writeln!(out, "candidate: {name}").map_err(|e| e.to_string())?;
            candidates += 1;
        } else {
            match progress.status(&mut git(&["branch", "-D", "--quiet", "--", &name])) {
                Ok(status) if status.success() => {
                    deleted += 1;
                    progress.update(format!("ブランチを削除中… {deleted}件"));
                }
                Ok(status) => {
                    failures += 1;
                    progress.suspend();
                    eprintln!("giclean: {name} の削除に失敗しました ({status})。");
                }
                Err(error) => {
                    failures += 1;
                    progress.suspend();
                    eprintln!("giclean: {name} を削除できません: {error}");
                }
            }
        }
    }
    if failures == 0 {
        progress.finish(
            &if dry_run {
                format!("削除候補 {candidates}件の確認が完了")
            } else {
                format!("{deleted}件のブランチを削除しました")
            },
            dry_run.then_some("確認が完了しました。"),
        );
    } else {
        drop(progress);
    }
    if !dry_run {
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
