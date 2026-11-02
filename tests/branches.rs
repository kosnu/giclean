use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Repo {
    root: PathBuf,
    work: PathBuf,
    origin: PathBuf,
}

fn isolated(command: &mut Command) -> &mut Command {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
        "GIT_TEMPLATE_DIR",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
    ] {
        command.env_remove(key);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Giclean Test")
        .env("GIT_AUTHOR_EMAIL", "giclean@example.invalid")
        .env("GIT_COMMITTER_NAME", "Giclean Test")
        .env("GIT_COMMITTER_EMAIL", "giclean@example.invalid")
        .env("LC_ALL", "C")
}

fn checked(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn text(output: &[u8]) -> &str {
    std::str::from_utf8(output).unwrap().trim()
}

impl Repo {
    fn new() -> Self {
        Self::with_format("sha1")
    }

    fn with_format(format: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("giclean-test-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let work = root.join("repo");
        let origin = root.join("origin.git");
        let repo = Self { root, work, origin };
        let object_format = format!("--object-format={format}");
        let mut init = Command::new("git");
        isolated(&mut init)
            .args(["init", "--bare", "--initial-branch=main", &object_format])
            .arg(&repo.origin);
        checked(&mut init);
        let mut init = Command::new("git");
        isolated(&mut init)
            .args(["init", "--initial-branch=main", &object_format])
            .arg(&repo.work);
        checked(&mut init);
        repo.git(&["config", "core.hooksPath", "no-test-hooks"]);
        repo.git(&["config", "commit.gpgSign", "false"]);
        repo.git(&["commit", "--allow-empty", "-m", "initial"]);
        let mut remote = repo.git_command();
        remote.args(["remote", "add", "origin"]).arg(&repo.origin);
        checked(&mut remote);
        repo.git(&["push", "origin", "main"]);
        repo
    }

    fn git_command(&self) -> Command {
        let mut command = Command::new("git");
        isolated(&mut command).current_dir(&self.work);
        command
    }

    fn git(&self, args: &[&str]) -> String {
        let output = checked(self.git_command().args(args));
        text(&output.stdout).to_owned()
    }

    fn exists(&self, branch: &str) -> bool {
        self.git_command()
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .status()
            .unwrap()
            .success()
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_giclean"));
        isolated(&mut command).current_dir(&self.work);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cli().args(args).output().unwrap()
    }

    fn add_worktree(&self, branch: &str, path: &str) {
        checked(
            self.git_command()
                .args(["worktree", "add"])
                .arg(self.root.join(path))
                .arg(branch),
        );
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn dry_run_preserves_squash_unmerged_current_protected_and_used_branches() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "merged"]);
    repo.git(&["commit", "--allow-empty", "-m", "feature"]);
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--no-ff", "merged", "-m", "merge feature"]);
    repo.git(&["branch", "used"]);
    repo.add_worktree("used", "worktree with spaces");
    for protected in ["master", "production"] {
        repo.git(&["branch", protected]);
    }
    repo.git(&["switch", "-c", "squash"]);
    fs::write(repo.work.join("squashed.txt"), "squash\n").unwrap();
    repo.git(&["add", "squashed.txt"]);
    repo.git(&["commit", "-m", "squashed feature"]);
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--squash", "squash"]);
    repo.git(&["commit", "-m", "squash merge"]);
    repo.git(&["push", "origin", "main"]);
    repo.git(&["branch", "current"]);
    repo.git(&["switch", "-c", "unmerged"]);
    repo.git(&["commit", "--allow-empty", "-m", "unmerged"]);
    repo.git(&["switch", "current"]);

    let before = repo.git(&["show-ref"]);
    let output = repo.run(&["br", "--dry-run"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "candidate: merged");
    assert_eq!(repo.git(&["show-ref"]), before);
    let progress = text(&output.stderr);
    assert!(progress.starts_with("giclean: ローカルブランチを確認しています..."));
    assert!(progress.contains("origin の既定ブランチと最新コミットを確認しています..."));
    assert!(progress.ends_with("giclean: 確認が完了しました。"));
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
}

#[test]
fn deletion_cleans_branch_configuration_and_preserves_other_refs() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    repo.git(&["branch", "日本語/マージ済み"]);
    repo.git(&["config", "branch.merged.description", "temporary branch"]);
    repo.git(&["switch", "-c", "unmerged"]);
    repo.git(&["commit", "--allow-empty", "-m", "unmerged"]);
    repo.git(&["switch", "main"]);

    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 2");
    assert!(!repo.exists("merged"));
    assert!(!repo.exists("日本語/マージ済み"));
    assert!(repo.exists("unmerged"));
    assert!(repo.exists("main"));
    assert!(
        !repo
            .git_command()
            .args(["config", "--get", "branch.merged.description"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn protected_or_used_branches_need_no_remote() {
    let repo = Repo::new();
    repo.git(&["branch", "used"]);
    repo.add_worktree("used", "used");
    repo.git(&["branch", "master"]);
    repo.git(&["branch", "production"]);
    repo.git(&["remote", "remove", "origin"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 0");
    assert!(!text(&output.stderr).contains("origin"));
    let output = repo.run(&["br", "--dry-run"]);
    assert_success(&output);
    assert!(output.stdout.is_empty());
}

#[test]
fn unborn_repository_needs_no_remote() {
    let repo = Repo::new();
    let unborn = repo.root.join("unborn");
    checked(
        repo.git_command()
            .args(["init", "--initial-branch=main"])
            .arg(&unborn),
    );
    let output = repo.cli().arg("br").current_dir(&unborn).output().unwrap();
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 0");
}

#[test]
fn missing_configured_origin_is_an_error_even_if_an_origin_directory_exists() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    repo.git(&["remote", "remove", "origin"]);
    fs::create_dir(repo.work.join("origin")).unwrap();
    let output = repo.run(&["br"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(repo.exists("merged"));
    assert!(output.stdout.is_empty());
}

#[test]
fn unreachable_remote_never_deletes_branches() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    checked(
        repo.git_command()
            .args(["remote", "set-url", "origin"])
            .arg(repo.root.join("missing.git")),
    );
    let output = repo.run(&["br"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(repo.exists("merged"));
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_remote_commit_stops_without_lazy_fetching_from_a_promisor_remote() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    let clone = repo.root.join("other");
    checked(
        repo.git_command()
            .arg("clone")
            .arg(&repo.origin)
            .arg(&clone),
    );
    checked(repo.git_command().current_dir(&clone).args([
        "commit",
        "--allow-empty",
        "-m",
        "new remote commit",
    ]));
    checked(
        repo.git_command()
            .current_dir(&clone)
            .args(["push", "origin", "main"]),
    );
    repo.git(&["config", "remote.origin.promisor", "true"]);
    repo.git(&["config", "remote.origin.partialclonefilter", "blob:none"]);
    let output = repo.run(&["br"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("git fetch origin"));
    assert!(repo.exists("merged"));
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
}

#[test]
fn uses_remote_head_instead_of_a_stale_remote_tracking_branch() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "only-local"]);
    repo.git(&["commit", "--allow-empty", "-m", "local only"]);
    let local_tip = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &local_tip]);
    repo.git(&["switch", "main"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 0");
    assert!(repo.exists("only-local"));
}

#[test]
fn protects_a_custom_default_branch_even_with_detached_head() {
    let repo = Repo::new();
    repo.git(&["branch", "trunk"]);
    repo.git(&["branch", "merged"]);
    repo.git(&["push", "origin", "trunk"]);
    checked(repo.git_command().arg("--git-dir").arg(&repo.origin).args([
        "symbolic-ref",
        "HEAD",
        "refs/heads/trunk",
    ]));
    repo.git(&["switch", "--detach"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 1");
    assert!(repo.exists("trunk"));
    assert!(repo.exists("main"));
    assert!(!repo.exists("merged"));
}

#[test]
fn detached_remote_head_is_rejected() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    let oid = repo.git(&["rev-parse", "HEAD"]);
    checked(repo.git_command().arg("--git-dir").arg(&repo.origin).args([
        "update-ref",
        "--no-deref",
        "HEAD",
        &oid,
    ]));
    let output = repo.run(&["br"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(repo.exists("merged"));
    assert!(text(&output.stderr).contains("リモートの HEAD"));
}

#[test]
fn empty_remote_is_rejected() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    checked(repo.git_command().arg("--git-dir").arg(&repo.origin).args([
        "update-ref",
        "-d",
        "refs/heads/main",
    ]));
    let output = repo.run(&["br"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(repo.exists("merged"));
}

#[test]
fn can_run_from_a_linked_worktree_and_protects_both_checkouts() {
    let repo = Repo::new();
    repo.git(&["branch", "used"]);
    repo.git(&["branch", "merged"]);
    repo.add_worktree("used", "linked");
    let output = repo
        .cli()
        .arg("br")
        .current_dir(repo.root.join("linked"))
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 1");
    assert!(repo.exists("used"));
    assert!(repo.exists("main"));
    assert!(!repo.exists("merged"));
}

#[test]
fn symbolic_local_branches_are_preserved() {
    let repo = Repo::new();
    repo.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 0");
    assert!(repo.exists("alias"));
    assert!(repo.exists("main"));
}

#[test]
fn replacement_objects_do_not_make_unmerged_commits_eligible() {
    let repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["switch", "-c", "unmerged"]);
    repo.git(&["commit", "--allow-empty", "-m", "unmerged"]);
    let tip = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["switch", "main"]);
    repo.git(&["replace", &tip, &base]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 0");
    assert!(repo.exists("unmerged"));
}

#[test]
fn supports_sha256_repositories() {
    let repo = Repo::with_format("sha256");
    repo.git(&["branch", "merged"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "deleted: 1");
    assert!(!repo.exists("merged"));
}

#[test]
fn rejects_bare_repositories_and_non_repositories() {
    let repo = Repo::new();
    for directory in [&repo.root, &repo.origin] {
        let output = repo
            .cli()
            .arg("br")
            .current_dir(directory)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn root_shows_cleanup_help_without_deleting_branches() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    let before = repo.git(&["show-ref"]);

    let output = repo.run(&[]);
    assert_success(&output);
    let help = text(&output.stdout);
    assert!(help.starts_with("giclean — Git のお掃除コマンド"));
    assert!(help.contains("使い方: giclean <command> [options]"));
    assert!(help.contains("  br "));
    assert!(output.stderr.is_empty());
    assert_eq!(repo.git(&["show-ref"]), before);
}

#[test]
fn invalid_arguments_return_two_before_running_git() {
    let repo = Repo::new();
    for args in [
        vec!["--force"],
        vec!["br", "--unknown"],
        vec!["br", "br"],
        vec!["br", "--dry-run", "extra"],
        vec!["--dry-run"],
        vec!["--dry-run", "br"],
        vec!["--dry-run", "--dry-run"],
        vec!["other"],
    ] {
        let output = repo.cli().args(&args).env("PATH", "").output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(text(&output.stderr).contains("使い方:"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn help_and_version_do_not_require_git_or_a_repository() {
    let repo = Repo::new();
    for args in [
        vec![],
        vec!["--help"],
        vec!["-h"],
        vec!["br", "--help"],
        vec!["br", "-h"],
        vec!["--version"],
        vec!["-V"],
    ] {
        let output = repo
            .cli()
            .current_dir(&repo.root)
            .args(&args)
            .env("PATH", "")
            .output()
            .unwrap();
        assert_success(&output);
        assert!(text(&output.stdout).starts_with("giclean"));
        assert!(output.stderr.is_empty());
    }

    let output = repo.run(&["br", "--help"]);
    assert_success(&output);
    let help = text(&output.stdout);
    assert!(help.contains("使い方: giclean br [--dry-run]"));
    assert!(help.contains("origin の最新の既定ブランチ"));
    assert!(help.contains("通常実行は確認なしで削除します"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }

    // Intercept only the child Git process. Every underlying Git operation is real,
    // and each test has its own PATH, hooks, log, and temporary repositories.
    fn shim(repo: &Repo, hook: &str) -> Command {
        let output = checked(Command::new("sh").args(["-c", "command -v git"]));
        let real_git = PathBuf::from(text(&output.stdout));
        let bin = repo.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let path = bin.join("git");
        fs::write(&path, format!(
            "#!/bin/sh\nset -eu\nreal_git={}\ncalls={}\n[ \"$1\" = --no-lazy-fetch ]\nshift\nprintf '%s\\n' \"$1\" >> \"$calls\"\n{hook}\nexec \"$real_git\" --no-lazy-fetch \"$@\"\n",
            shell_quote(&real_git), shell_quote(&repo.root.join("calls")),
        )).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        // Any accidental introduction of GitHub CLI use fails this test.
        let gh = bin.join("gh");
        fs::write(&gh, "#!/bin/sh\nexit 99\n").unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = repo.cli();
        command
            .arg("br")
            .env("PATH", std::env::join_paths(paths).unwrap());
        command
    }

    #[test]
    fn queries_the_remote_once_and_prints_progress_before_the_query() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let progress = repo.root.join("progress");
        let hook = format!(
            "if [ \"$1\" = ls-remote ]; then\ncase \"$(cat {})\" in\n*'giclean: origin の既定ブランチと最新コミットを確認しています...') ;;\n*) exit 90 ;;\nesac\nfi",
            shell_quote(&progress),
        );
        let output = shim(&repo, &hook)
            .arg("--dry-run")
            .stderr(fs::File::create(&progress).unwrap())
            .output()
            .unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "candidate: merged");
        let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "ls-remote").count(), 1);
        assert!(
            !calls
                .lines()
                .any(|call| matches!(call, "fetch" | "pull" | "push"))
        );
    }

    #[test]
    fn protects_branches_changed_created_or_checked_out_during_the_remote_query() {
        let repo = Repo::new();
        for branch in ["moving", "moving-merged", "checkout-later", "merged"] {
            repo.git(&["branch", branch]);
        }
        repo.git(&["commit", "--allow-empty", "-m", "new base"]);
        repo.git(&["push", "origin", "main"]);
        repo.git(&["switch", "-c", "unmerged"]);
        repo.git(&["commit", "--allow-empty", "-m", "unmerged"]);
        let tip = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["switch", "main"]);
        let hook = format!(
            "if [ \"$1\" = ls-remote ]; then\n\"$real_git\" update-ref refs/heads/moving {tip}\n\"$real_git\" update-ref refs/heads/moving-merged main\n\"$real_git\" branch created-later main\n\"$real_git\" worktree add -q {} checkout-later\nfi",
            shell_quote(&repo.root.join("checkout-later")),
        );
        let output = shim(&repo, &hook).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "deleted: 1");
        assert_eq!(repo.git(&["rev-parse", "moving"]), tip);
        assert_eq!(
            repo.git(&["rev-parse", "moving-merged"]),
            repo.git(&["rev-parse", "main"])
        );
        assert!(repo.exists("created-later"));
        assert!(repo.exists("checkout-later"));
        assert!(!repo.exists("merged"));
    }

    #[test]
    fn rechecks_branch_tips_after_the_merged_snapshot() {
        let repo = Repo::new();
        repo.git(&["branch", "moving"]);
        // A different, already merged tip still must not be deleted after it moves.
        let old = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["commit", "--allow-empty", "-m", "new base"]);
        repo.git(&["push", "origin", "main"]);
        let hook = format!(
            "if [ \"$1\" = for-each-ref ] && [ \"${{4:-}}\" = refs/heads/moving ]; then\n\"$real_git\" update-ref refs/heads/moving main {old}\nfi"
        );
        let output = shim(&repo, &hook).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "deleted: 0");
        assert!(repo.exists("moving"));
        assert_eq!(
            repo.git(&["rev-parse", "moving"]),
            repo.git(&["rev-parse", "main"])
        );
    }

    #[test]
    fn deletion_failure_returns_one_and_reports_the_successful_count() {
        let repo = Repo::new();
        repo.git(&["branch", "a-blocked"]);
        repo.git(&["branch", "z-merged"]);
        let hook = format!(
            "if [ \"$1\" = branch ] && [ \"${{5:-}}\" = a-blocked ]; then\n\"$real_git\" worktree add -q {} a-blocked\nfi",
            shell_quote(&repo.root.join("late-worktree")),
        );
        let output = shim(&repo, &hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(text(&output.stdout), "deleted: 1");
        assert!(repo.exists("a-blocked"));
        assert!(!repo.exists("z-merged"));
        assert!(text(&output.stderr).contains("1 件のブランチを削除できませんでした"));
    }

    #[test]
    fn recheck_failure_stops_remaining_deletions_and_reports_partial_progress() {
        let repo = Repo::new();
        for branch in ["a-merged", "b-unreadable", "z-merged"] {
            repo.git(&["branch", branch]);
        }
        let hook = "if [ \"$1\" = for-each-ref ] && [ \"${4:-}\" = refs/heads/b-unreadable ]; then\nexit 91\nfi";
        let output = shim(&repo, hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(text(&output.stdout), "deleted: 1");
        assert!(!repo.exists("a-merged"));
        assert!(repo.exists("b-unreadable"));
        assert!(repo.exists("z-merged"));
    }

    #[test]
    fn non_utf8_branch_names_stop_before_any_deletion() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        // APFS cannot create non-UTF-8 filenames. Packed refs can represent these
        // names on both macOS and Linux without creating such a loose-ref file.
        let oid = repo.git(&["rev-parse", "HEAD"]);
        let mut packed = format!("{oid} refs/heads/non-utf8-").into_bytes();
        packed.extend_from_slice(b"\xff\n");
        fs::write(repo.work.join(".git/packed-refs"), packed).unwrap();
        let output = repo.run(&["br"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(text(&output.stderr).contains("UTF-8"));
        assert!(repo.exists("merged"));
        assert!(
            repo.git_command()
                .args([
                    OsStr::new("show-ref"),
                    OsStr::new("--verify"),
                    OsStr::new("--quiet")
                ])
                .arg(OsString::from_vec(b"refs/heads/non-utf8-\xff".to_vec()))
                .status()
                .unwrap()
                .success()
        );
    }
}
