use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "explain/mod.rs"]
mod explain_tests;

mod json_support;

#[path = "batch/mod.rs"]
mod batch_tests;
#[path = "discovery/mod.rs"]
mod discovery_tests;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
#[path = "worktrees/mod.rs"]
mod worktree_tests;

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
        "GIT_SSH_VARIANT",
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

    fn git_input(&self, args: &[&str], input: &[u8]) -> String {
        use std::io::Write;
        use std::process::Stdio;

        let mut command = self.git_command();
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_success(&output);
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
        isolated(&mut command)
            .current_dir(&self.work)
            .env("HOME", &self.root)
            .env_remove("CODEX_HOME");
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
fn dry_run_includes_squash_and_preserves_unmerged_current_protected_and_used_branches() {
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
    assert_eq!(text(&output.stdout), "candidate: merged\ncandidate: squash");
    assert_eq!(repo.git(&["show-ref"]), before);
    assert!(output.stderr.is_empty());
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
}

#[test]
fn squash_deletion_matches_complete_changes_with_other_default_branch_changes() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        for path in ["edited", "deleted", "renamed"] {
            fs::write(repo.work.join(path), "base\n").unwrap();
        }
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "base files"]);
        repo.git(&["switch", "-c", "squash"]);
        fs::write(repo.work.join("edited"), "changed\n").unwrap();
        repo.git(&["rm", "deleted"]);
        repo.git(&["mv", "renamed", "renamed\twith\nspaces"]);
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "first feature commit"]);
        fs::write(repo.work.join("binary"), [0, 255, 1, 2]).unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "second feature commit"]);
        repo.git(&["switch", "main"]);
        fs::write(repo.work.join("unrelated"), "another feature\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "independent change before squash"]);
        repo.git(&["merge", "--squash", "squash"]);
        repo.git(&["commit", "-m", "squash merge"]);
        fs::write(repo.work.join("unrelated"), "updated\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "independent change after squash"]);
        repo.git(&["push", "origin", "main"]);
        repo.git(&["config", "diff.relative", "true"]);
        repo.git(&["config", "diff.renames", "copies"]);
        repo.git(&["config", "branch.squash.description", "cleanup"]);
        fs::write(repo.work.join("edited"), "local uncommitted edit\n").unwrap();
        let subdir = repo.work.join("subdir");
        fs::create_dir(&subdir).unwrap();

        let output = repo.cli().arg("br").current_dir(subdir).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "squash: deleted", "{format}");
        assert!(!repo.exists("squash"));
        assert_eq!(
            fs::read(repo.work.join("edited")).unwrap(),
            b"local uncommitted edit\n"
        );
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        assert!(
            !repo
                .git_command()
                .args(["config", "--get", "branch.squash.description"])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
fn squash_deletes_after_later_edits_or_reverts_on_default_branch() {
    for format in ["sha1", "sha256"] {
        for case in ["later-edit", "reverted"] {
            let repo = Repo::with_format(format);
            repo.git(&["switch", "-c", "squash"]);
            for path in ["first", "second"] {
                fs::write(repo.work.join(path), "feature\n").unwrap();
            }
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "feature"]);
            repo.git(&["switch", "main"]);
            if case == "later-edit" {
                fs::write(repo.work.join("unrelated"), "independent change\n").unwrap();
                repo.git(&["add", "."]);
                repo.git(&["commit", "-m", "independent change before integration"]);
            }
            repo.git(&["merge", "--squash", "squash"]);
            repo.git(&["commit", "-m", "integration"]);
            if case == "reverted" {
                repo.git(&["revert", "--no-edit", "HEAD"]);
            } else {
                fs::write(repo.work.join("first"), "feature plus later edit\n").unwrap();
                repo.git(&["add", "."]);
                repo.git(&["commit", "-m", "later edit"]);
            }
            repo.git(&["push", "origin", "main"]);
            let before = repo.git(&["show-ref"]);
            let output = repo.run(&["br", "--dry-run"]);
            assert_success(&output);
            assert_eq!(
                text(&output.stdout),
                "candidate: squash",
                "{format}: {case}"
            );
            assert_eq!(repo.git(&["show-ref"]), before);
            let output = repo.run(&["br"]);
            assert_success(&output);
            assert_eq!(text(&output.stdout), "squash: deleted", "{format}: {case}");
            assert!(!repo.exists("squash"));
            assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        }
    }
}

#[test]
fn squash_preserves_partial_and_extra_tip_changes() {
    for case in ["partial", "extra-tip", "extra-same-path"] {
        let repo = Repo::new();
        repo.git(&["switch", "-c", "squash"]);
        for path in ["first", "second"] {
            fs::write(repo.work.join(path), "feature\n").unwrap();
        }
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "feature"]);
        repo.git(&["switch", "main"]);
        if case == "partial" {
            fs::write(repo.work.join("first"), "feature\n").unwrap();
            repo.git(&["add", "."]);
        } else {
            repo.git(&["merge", "--squash", "squash"]);
        }
        repo.git(&["commit", "-m", "integration"]);
        if case != "partial" {
            fs::write(repo.work.join("first"), "feature plus later edit\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "later edit"]);
            repo.git(&["switch", "squash"]);
            let path = if case == "extra-tip" {
                "extra"
            } else {
                "first"
            };
            fs::write(repo.work.join(path), "unmerged\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "unmerged follow-up"]);
            repo.git(&["switch", "main"]);
        }
        repo.git(&["push", "origin", "main"]);
        let before = repo.git(&["show-ref"]);
        let output = repo.run(&["br", "--dry-run"]);
        assert_success(&output);
        assert_eq!(text(&output.stdout), "", "{case}");
        assert_eq!(repo.git(&["show-ref"]), before);
        let output = repo.run(&["br"]);
        assert_success(&output);
        assert_eq!(text(&output.stdout), "", "{case}");
        assert!(repo.exists("squash"));
    }
}

#[test]
fn historical_squash_requires_all_paths_to_match_at_the_same_commit() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "squash"]);
    for path in ["first", "second"] {
        fs::write(repo.work.join(path), "feature\n").unwrap();
    }
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "feature"]);
    repo.git(&["switch", "main"]);
    fs::write(repo.work.join("first"), "feature\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "first path only"]);
    fs::write(repo.work.join("first"), "different\n").unwrap();
    fs::write(repo.work.join("second"), "feature\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "second path only"]);
    repo.git(&["push", "origin", "main"]);
    for args in [&["br", "--dry-run"][..], &["br"][..]] {
        let output = repo.run(args);
        assert_success(&output);
        assert!(output.stdout.is_empty());
        assert!(repo.exists("squash"));
    }
}

#[test]
fn historical_squash_ignores_side_branch_snapshots_discarded_by_a_merge() {
    let repo = Repo::new();
    let base = repo.git(&["rev-parse", "HEAD"]);
    let base_tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    repo.git(&["switch", "-c", "squash"]);
    fs::write(repo.work.join("feature"), "feature\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "feature"]);
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let side = repo.git(&["commit-tree", &tree, "-p", &base, "-m", "side integration"]);
    let target = repo.git(&[
        "commit-tree",
        &base_tree,
        "-p",
        &base,
        "-p",
        &side,
        "-m",
        "discard side changes",
    ]);
    repo.git(&["switch", "main"]);
    repo.git(&["reset", "--hard", &target]);
    repo.git(&["push", "origin", "main"]);
    for args in [&["br", "--dry-run"][..], &["br"][..]] {
        let output = repo.run(args);
        assert_success(&output);
        assert!(output.stdout.is_empty());
        assert!(repo.exists("squash"));
    }
}

#[test]
fn unrelated_histories_are_preserved() {
    let repo = Repo::new();
    repo.git(&["switch", "--orphan", "unrelated"]);
    repo.git(&["commit", "--allow-empty", "-m", "unrelated root"]);
    repo.git(&["switch", "main"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "");
    assert!(repo.exists("unrelated"));
}

#[test]
fn multiple_merge_bases_are_preserved_in_a_batch_with_an_eligible_branch() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "ambiguous"]);
    fs::write(repo.work.join("first"), "first side\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "first side"]);
    repo.git(&["switch", "main"]);
    fs::write(repo.work.join("second"), "second side\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "second side"]);
    repo.git(&["tag", "second-side"]);
    repo.git(&["merge", "--no-ff", "ambiguous", "-m", "merge first side"]);
    repo.git(&["switch", "ambiguous"]);
    repo.git(&["merge", "--no-ff", "second-side", "-m", "merge second side"]);
    repo.git(&["switch", "main"]);
    assert_eq!(
        repo.git(&["merge-base", "--all", "main", "ambiguous"])
            .lines()
            .count(),
        2
    );
    repo.git(&["switch", "-c", "squash"]);
    fs::write(repo.work.join("feature"), "squashed\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "feature"]);
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--squash", "squash"]);
    repo.git(&["commit", "-m", "squash merge"]);
    repo.git(&["push", "origin", "main"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "squash: deleted");
    assert!(repo.exists("ambiguous"));
    assert!(!repo.exists("squash"));
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
    assert_eq!(
        text(&output.stdout),
        "merged: deleted\n日本語/マージ済み: deleted"
    );
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
    assert_eq!(text(&output.stdout), "");
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
    assert_eq!(text(&output.stdout), "");
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
    assert_eq!(text(&output.stdout), "");
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
    assert_eq!(text(&output.stdout), "merged: deleted");
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
    assert!(text(&output.stderr).contains("remote HEAD"));
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
    assert_eq!(text(&output.stdout), "merged: deleted");
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
    assert_eq!(text(&output.stdout), "");
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
    assert_eq!(text(&output.stdout), "");
    assert!(repo.exists("unmerged"));
}

#[test]
fn supports_sha256_repositories() {
    let repo = Repo::with_format("sha256");
    repo.git(&["branch", "merged"]);
    let output = repo.run(&["br"]);
    assert_success(&output);
    assert_eq!(text(&output.stdout), "merged: deleted");
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
    assert!(help.starts_with("giclean — Git cleanup"));
    assert!(help.contains("Usage: giclean <command> [options]"));
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
        assert!(text(&output.stderr).contains("Usage:"));
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
    assert!(help.contains("Usage: giclean br [--dry-run]"));
    assert!(help.contains("origin's latest default branch"));
    assert!(help.contains("Deletes without confirmation"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn historical_search_preserves_a_match_when_the_base_is_a_second_parent() {
        let repo = Repo::with_format("sha256");
        let root = repo.git(&["rev-parse", "HEAD"]);
        let empty_tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
        let feature = repo.git_input(&["hash-object", "-w", "--stdin"], b"feature\n");
        let later = repo.git_input(&["hash-object", "-w", "--stdin"], b"later edit\n");
        let tree = repo.git_input(
            &["mktree"],
            format!("100644 blob {feature}\tfeature\n").as_bytes(),
        );
        let first = repo.git(&[
            "commit-tree",
            &tree,
            "-p",
            &root,
            "-m",
            "first-parent feature",
        ]);
        let base = repo.git(&["commit-tree", &empty_tree, "-p", &root, "-m", "side base"]);
        let tip = repo.git(&["commit-tree", &tree, "-p", &base, "-m", "topic"]);
        let mut parent = repo.git(&[
            "commit-tree",
            &tree,
            "-p",
            &first,
            "-p",
            &base,
            "-m",
            "merge side base",
        ]);
        // The only matching eligible snapshot is the merge itself. Its paths
        // equal its first parent's, so a path-limited walk would omit it.
        for i in 0..130 {
            let blob = repo.git_input(
                &["hash-object", "-w", "--stdin"],
                format!("{i}\n").as_bytes(),
            );
            let tree = repo.git_input(
                &["mktree"],
                format!("100644 blob {later}\tfeature\n100644 blob {blob}\tunrelated\n").as_bytes(),
            );
            parent = repo.git(&["commit-tree", &tree, "-p", &parent, "-m", "later change"]);
        }
        repo.git_input(
            &["update-ref", "--stdin"],
            format!("update refs/heads/main {parent}\ncreate refs/heads/topic {tip}\n").as_bytes(),
        );
        repo.git(&["reset", "--hard", "main"]);
        repo.git(&["push", "origin", "main"]);
        let before = repo.git(&["show-ref"]);
        let output = repo.run(&["br", "--dry-run"]);
        assert_success(&output);
        assert_eq!(text(&output.stdout), "candidate: topic");
        assert_eq!(repo.git(&["show-ref"]), before);
        let output = repo.run(&["br"]);
        assert_success(&output);
        assert_eq!(text(&output.stdout), "topic: deleted");
        assert!(!repo.exists("topic"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn historical_object_filter_handles_literal_bytes_subdirectories_and_query_errors() {
        let repo = Repo::new();
        let base = repo.git(&["rev-parse", "HEAD"]);
        // Construct invalid UTF-8 names in tree objects: macOS cannot create
        // these names in the filesystem, but Git must still compare their bytes.
        let feature = repo.git_input(&["hash-object", "-w", "--stdin"], b"feature\n");
        let later = repo.git_input(&["hash-object", "-w", "--stdin"], b"later edit\n");
        let unrelated = repo.git_input(&["hash-object", "-w", "--stdin"], b"initial\n");
        let name = b":(glob)*[x]\twith\n\xff";
        let nested_feature = repo.git_input(
            &["mktree", "-z"],
            &[format!("100644 blob {feature}\t").as_bytes(), name, b"\0"].concat(),
        );
        let nested = repo.git_input(
            &["mktree", "-z"],
            &[format!("100644 blob {later}\t").as_bytes(), name, b"\0"].concat(),
        );
        let tree = repo.git_input(
            &["mktree"],
            format!("040000 tree {nested_feature}\tnested\n").as_bytes(),
        );
        let tip = repo.git(&["commit-tree", &tree, "-p", &base, "-m", "feature"]);
        let tree = repo.git_input(
            &["mktree"],
            format!("100644 blob {unrelated}\tunrelated\n").as_bytes(),
        );
        let parent = repo.git(&[
            "commit-tree",
            &tree,
            "-p",
            &base,
            "-m",
            "independent change",
        ]);
        let tree = repo.git_input(
            &["mktree"],
            format!("040000 tree {nested_feature}\tnested\n100644 blob {unrelated}\tunrelated\n")
                .as_bytes(),
        );
        let parent = repo.git(&["commit-tree", &tree, "-p", &parent, "-m", "integration"]);
        let tree = repo.git_input(
            &["mktree"],
            format!("040000 tree {nested}\tnested\n100644 blob {unrelated}\tunrelated\n")
                .as_bytes(),
        );
        let mut parent = repo.git(&["commit-tree", &tree, "-p", &parent, "-m", "later edit"]);
        repo.git(&["switch", "--detach", &base]);
        repo.git(&["update-ref", "refs/heads/squash", &tip]);
        fs::create_dir(repo.work.join("nested")).unwrap();
        for i in 0..130 {
            let blob = repo.git_input(
                &["hash-object", "-w", "--stdin"],
                format!("{i}\n").as_bytes(),
            );
            let tree = repo.git_input(
                &["mktree"],
                format!("040000 tree {nested}\tnested\n100644 blob {blob}\tunrelated\n").as_bytes(),
            );
            parent = repo.git(&[
                "commit-tree",
                &tree,
                "-p",
                &parent,
                "-m",
                "unrelated change",
            ]);
        }
        repo.git(&["update-ref", "refs/heads/main", &parent]);
        repo.git(&["push", "origin", "main"]);
        let history_output = repo.root.join("history-output");
        let hook = format!(
            "if [ \"$1\" = rev-list ]; then\ncase \" $* \" in *' --objects '*)\ncase \"${{TEST_HISTORY_FAILURE:-}}\" in\nexit) exit 91 ;;\nmalformed) printf 'invalid\\n'; exit 0 ;;\nforeign) printf 'commit {}\\n{}\\n'; exit 0 ;;\nesac ;;\nesac\n\"$real_git\" --no-lazy-fetch \"$@\" > {}\ncat {}; exit 0\nfi",
            "a".repeat(40),
            "b".repeat(40),
            shell_quote(&history_output),
            shell_quote(&history_output),
        );
        let mut command = shim(&repo, &hook);
        command
            .arg("--dry-run")
            .current_dir(repo.work.join("nested"));
        for key in [
            "GIT_LITERAL_PATHSPECS",
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
        ] {
            let before = repo.git(&["show-ref"]);
            let output = command.env(key, "1").output().unwrap();
            command.env_remove(key);
            assert_success(&output);
            assert_eq!(text(&output.stdout), "candidate: squash", "{key}");
            assert_eq!(repo.git(&["show-ref"]), before);
            let history = fs::read_to_string(&history_output).unwrap();
            assert_eq!(history.lines().next(), Some(parent.as_str()));
        }
        let calls_path = repo.root.join("calls");
        let calls = fs::read_to_string(&calls_path).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "rev-list").count(), 8);
        repo.git(&["branch", "a-merged", &base]);
        for failure in ["exit", "malformed", "foreign"] {
            let before = repo.git(&["show-ref"]);
            let output = command
                .env("TEST_HISTORY_FAILURE", failure)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{failure}");
            assert!(output.stdout.is_empty());
            assert_eq!(repo.git(&["show-ref"]), before);
        }
        command.env_remove("TEST_HISTORY_FAILURE");
        repo.git(&["branch", "-d", "a-merged"]);
        // Absent objects rule out all of these added paths, without affecting
        // the eligible branch whose blob exists in the target's history.
        let blob = repo.git_input(&["hash-object", "-w", "--stdin"], b"unmerged\n");
        let rows: String = (0..1000)
            .map(|i| format!("100644 blob {blob}\tlong-unmerged-path-{i:04}\n"))
            .collect();
        let tree = repo.git_input(&["mktree"], rows.as_bytes());
        let tip = repo.git(&[
            "commit-tree",
            &tree,
            "-p",
            &base,
            "-m",
            "many unmerged paths",
        ]);
        repo.git(&["update-ref", "refs/heads/wide", &tip]);
        fs::write(&calls_path, "").unwrap();
        let output = command.output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "candidate: squash");
        let calls = fs::read_to_string(&calls_path).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "rev-list").count(), 2);
        let output = repo.run(&["br"]);
        assert_success(&output);
        assert_eq!(text(&output.stdout), "squash: deleted");
        assert!(repo.exists("wide"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn historical_object_filter_preserves_modes_deletions_and_missing_gitlink_objects() {
        let repo = Repo::with_format("sha256");
        let root = repo.git(&["rev-parse", "HEAD"]);
        let blob = repo.git_input(&["hash-object", "-w", "--stdin"], b"base\n");
        let absent = repo.git_input(&["hash-object", "-w", "--stdin"], b"unmerged\n");
        let gitlink = "a".repeat(64);
        let later_gitlink = "b".repeat(64);
        let rows = format!("100644 blob {blob}\tdeleted\n100644 blob {blob}\tscript\n");
        let tree = repo.git_input(&["mktree"], rows.as_bytes());
        let base = repo.git(&["commit-tree", &tree, "-p", &root, "-m", "base files"]);
        for (name, rows) in [
            ("deleted", format!("100644 blob {blob}\tscript\n")),
            (
                "mode",
                format!("100644 blob {blob}\tdeleted\n100755 blob {blob}\tscript\n"),
            ),
            (
                "gitlink",
                format!("{rows}160000 commit {gitlink}\tmodule\n"),
            ),
            (
                "wrong-path",
                format!("{rows}100644 blob {blob}\twrong-path\n"),
            ),
            (
                "unmerged",
                format!("{rows}100644 blob {absent}\tunmerged\n"),
            ),
        ] {
            let tree = repo.git_input(&["mktree"], rows.as_bytes());
            let tip = repo.git(&["commit-tree", &tree, "-p", &base, "-m", name]);
            repo.git(&["update-ref", &format!("refs/heads/{name}"), &tip]);
        }
        let tree = repo.git_input(
            &["mktree"],
            format!("{rows}100644 blob {blob}\tunrelated\n").as_bytes(),
        );
        let parent = repo.git(&["commit-tree", &tree, "-p", &base, "-m", "unrelated first"]);
        let tree = repo.git_input(
            &["mktree"],
            format!("160000 commit {gitlink}\tmodule\n100755 blob {blob}\tscript\n100644 blob {blob}\tunrelated\n").as_bytes(),
        );
        let mut parent = repo.git(&["commit-tree", &tree, "-p", &parent, "-m", "integration"]);
        for i in 0..130 {
            let history_blob = repo.git_input(
                &["hash-object", "-w", "--stdin"],
                format!("{i}\n").as_bytes(),
            );
            let tree = repo.git_input(
                &["mktree"],
                format!("{rows}160000 commit {later_gitlink}\tmodule\n100644 blob {history_blob}\tunrelated\n").as_bytes(),
            );
            parent = repo.git(&["commit-tree", &tree, "-p", &parent, "-m", "later changes"]);
        }
        repo.git(&["switch", "--detach", &root]);
        repo.git(&["update-ref", "refs/heads/main", &parent]);
        repo.git(&["push", "origin", "main"]);
        repo.git(&["config", "diff.ignoreSubmodules", "all"]);
        let reachable = repo.git(&["rev-list", "--objects", "--no-object-names", "main"]);
        assert!(
            !reachable
                .lines()
                .any(|oid| oid == gitlink || oid == later_gitlink || oid == absent)
        );
        let before = repo.git(&["show-ref"]);
        let output = shim(&repo, "").arg("--dry-run").output().unwrap();
        assert_success(&output);
        assert_eq!(
            text(&output.stdout),
            "candidate: deleted\ncandidate: gitlink\ncandidate: mode"
        );
        assert_eq!(repo.git(&["show-ref"]), before);
        let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "rev-list").count(), 2);
        let output = repo.run(&["br"]);
        assert_success(&output);
        assert_eq!(
            text(&output.stdout),
            "deleted: deleted\ngitlink: deleted\nmode: deleted"
        );
        assert!(repo.exists("wrong-path"));
        assert!(repo.exists("unmerged"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn squash_checks_modes_symlinks_and_gitlinks_and_keeps_protected_branches() {
        for mismatch in ["none", "mode", "symlink", "gitlink"] {
            let repo = Repo::new();
            let root = repo.git(&["rev-parse", "HEAD"]);
            fs::write(repo.work.join("script"), "same content\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "base file"]);
            let base = repo.git(&["rev-parse", "HEAD"]);
            repo.git(&["switch", "-c", "squash"]);
            fs::set_permissions(repo.work.join("script"), fs::Permissions::from_mode(0o755))
                .unwrap();
            std::os::unix::fs::symlink("script", repo.work.join("link")).unwrap();
            repo.git(&["add", "."]);
            repo.git(&[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{base},module"),
            ]);
            repo.git(&["commit", "-m", "feature modes link and gitlink"]);
            for name in ["squash-used", "master", "production"] {
                repo.git(&["branch", name]);
            }
            repo.git(&["switch", "main"]);
            repo.add_worktree("squash-used", "used");
            repo.git(&["merge", "--squash", "squash"]);
            match mismatch {
                "mode" => {
                    repo.git(&["update-index", "--chmod=-x", "script"]);
                }
                "symlink" => {
                    fs::remove_file(repo.work.join("link")).unwrap();
                    std::os::unix::fs::symlink("another-target", repo.work.join("link")).unwrap();
                    repo.git(&["add", "link"]);
                }
                "gitlink" => {
                    repo.git(&[
                        "update-index",
                        "--cacheinfo",
                        &format!("160000,{root},module"),
                    ]);
                }
                _ => {}
            }
            // A mismatch must exist at integration time: a later metadata edit
            // would now qualify through the earlier matching snapshot.
            repo.git(&["commit", "-m", "integration with selected metadata"]);
            repo.git(&["push", "origin", "main"]);
            // A user setting must not hide submodule differences.
            repo.git(&["config", "diff.ignoreSubmodules", "all"]);
            let output = repo.run(&["br"]);
            assert_success(&output);
            assert_eq!(
                text(&output.stdout),
                if mismatch == "none" {
                    "squash: deleted"
                } else {
                    ""
                },
                "{mismatch}"
            );
            assert_eq!(repo.exists("squash"), mismatch != "none");
            for name in ["squash-used", "master", "production", "main"] {
                assert!(repo.exists(name));
            }
        }
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }

    // Both supported CI systems provide script. Its PTY exercises is_terminal
    // and live output without adding a terminal library to the application.
    fn terminal_command(command: &Command) -> Command {
        let mut terminal = Command::new("script");
        if cfg!(target_os = "macos") {
            terminal
                .args(["-q", "/dev/null"])
                .arg(command.get_program());
            terminal.args(command.get_args());
        } else {
            let mut args = vec![shell_quote(Path::new(command.get_program()))];
            args.extend(command.get_args().map(|arg| shell_quote(Path::new(arg))));
            terminal.args(["-q", "-e", "-c", &args.join(" "), "/dev/null"]);
        }
        if let Some(directory) = command.get_current_dir() {
            terminal.current_dir(directory);
        }
        terminal.env("TERM", "xterm");
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                terminal.env(key, value);
            } else {
                terminal.env_remove(key);
            }
        }
        terminal
    }

    fn in_terminal(command: &Command) -> Output {
        use std::process::Stdio;

        let mut child = terminal_command(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Keep the terminal open until the command exits. Sending EOF here can
        // echo ^D into a nested PTY before Git has even started.
        let _input = child.stdin.take().unwrap();
        child.wait_with_output().unwrap()
    }

    #[test]
    fn terminal_progress_preserves_direct_tty_authentication_prompts() {
        use std::io::{Read, Write};
        use std::process::Stdio;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let request = repo.root.join("credential-request");
        fs::write(&request, "protocol=https\nhost=example.invalid\n\n").unwrap();
        let hook = format!(
            "if [ \"$1\" = ls-remote ]; then\nsleep 0.4\n\"$real_git\" -c credential.helper= credential fill < {} > /dev/null\nfi",
            shell_quote(&request),
        );
        let mut command = shim(&repo, &hook);
        command
            .arg("--dry-run")
            .env("GIT_TERMINAL_PROMPT", "1")
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS");
        let mut child = terminal_command(&command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(size) = output.read(&mut buffer) {
                if size == 0 || send.send(buffer[..size].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut transcript = Vec::new();
        let mut redraws_during_input = Vec::new();
        for (prompt, reply) in [
            ("Username for ", "repro-user\n"),
            ("Password for ", "repro-password\n"),
        ] {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !String::from_utf8_lossy(&transcript).contains(prompt) {
                let next = receive.recv_timeout(deadline.saturating_duration_since(Instant::now()));
                match next {
                    Ok(bytes) => transcript.extend(bytes),
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!(
                            "{prompt} was not shown: {error}; {}",
                            String::from_utf8_lossy(&transcript)
                        );
                    }
                }
            }
            // Keep the real Git prompt open long enough for multiple redraws.
            let start = transcript.len();
            let deadline = Instant::now() + Duration::from_millis(350);
            while let Ok(bytes) =
                receive.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                transcript.extend(bytes);
            }
            redraws_during_input.push(transcript[start..].contains(&b'\x1b'));
            input.write_all(reply.as_bytes()).unwrap_or_else(|error| {
                panic!(
                    "reply to {prompt} failed: {error}; {}",
                    String::from_utf8_lossy(&transcript)
                );
            });
            input.flush().unwrap();
        }
        let status = child.wait().unwrap();
        drop(input);
        reader.join().unwrap();
        for bytes in receive.try_iter() {
            transcript.extend(bytes);
        }
        let display = String::from_utf8_lossy(&transcript);
        assert!(status.success(), "{display}");
        assert_eq!(redraws_during_input, [false, false], "{display}");
        let before_prompt = display.split_once("Username for ").unwrap().0;
        assert!(
            before_prompt
                .matches("Checking origin's default branch and latest commit...")
                .count()
                >= 3,
            "{display}"
        );
        assert!(!display.contains("repro-password"), "{display}");
        assert!(display.contains("candidate: merged"), "{display}");
        assert!(repo.exists("merged"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn terminal_progress_updates_one_line_and_completes_dry_run() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let temporary = repo.root.join("terminal ' $(not-a-command)");
        fs::create_dir(&temporary).unwrap();
        let mut command = shim(&repo, "if [ \"$1\" = ls-remote ]; then sleep 0.4; fi");
        command.arg("--dry-run").env("TMPDIR", &temporary);
        let output = in_terminal(&command);
        assert_success(&output);
        let display = text(&output.stdout);
        assert!(display.contains("\r\x1b[2K⠋"), "{display}");
        let before_results = display.split_once("candidate: merged").unwrap().0;
        assert!(!before_results.contains('\n'), "{display}");
        assert!(before_results.contains("Checking origin's default branch and latest commit..."));
        assert!(
            before_results
                .matches("Checking origin's default branch and latest commit...")
                .count()
                >= 3,
            "{display}"
        );
        assert!(before_results.contains("Checking merged branches..."));
        assert!(!display.contains("秒"), "{display}");
        assert!(!display.contains('✔'), "{display}");
        assert!(display.ends_with("candidate: merged"), "{display}");
        assert!(repo.exists("merged"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        assert_eq!(fs::read_dir(&temporary).unwrap().count(), 0);
        let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "ls-remote").count(), 1);
    }

    #[test]
    fn terminal_progress_keeps_a_static_status_when_relay_preparation_fails() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let mut command = repo.cli();
        command
            .args(["br", "--dry-run"])
            .env("TMPDIR", repo.root.join("missing-directory"));
        let output = in_terminal(&command);
        assert_success(&output);
        let display = text(&output.stdout);
        assert!(
            display.contains("Checking origin's default branch and latest commit...\r\n"),
            "{display}"
        );
        assert!(display.ends_with("candidate: merged"), "{display}");
        assert!(repo.exists("merged"));
    }

    #[test]
    fn terminal_progress_reports_deleted_branches_after_clearing_the_spinner() {
        let repo = Repo::new();
        for branch in ["merged", "日本語/マージ済み"] {
            repo.git(&["branch", branch]);
        }
        let command = shim(&repo, "case \"$1\" in branch|-c) sleep 0.2 ;; esac");
        let output = in_terminal(&command);
        assert_success(&output);
        let display = text(&output.stdout);
        assert!(display.contains("⠙"), "{display}");
        assert!(!display.contains("秒"), "{display}");
        assert!(!display.contains('✔'), "{display}");
        let (progress, results) = display.split_once("merged: deleted").unwrap();
        assert!(!progress.contains('\n'), "{display}");
        assert!(progress.ends_with("\r\x1b[2K"), "{display}");
        assert_eq!(results, "\r\n日本語/マージ済み: deleted");
        assert!(!repo.exists("merged"));
        assert!(!repo.exists("日本語/マージ済み"));
        assert!(repo.exists("main"));
    }

    #[test]
    fn terminal_progress_stops_for_child_diagnostics_and_clears_on_failure() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let command = shim(
            &repo,
            "if [ \"$1\" = for-each-ref ]; then printf 'local diagnostic' >&2; sleep 0.3; exit 23; fi",
        );
        let output = in_terminal(&command);
        let display = text(&output.stdout);
        let after_diagnostic = display.split_once("local diagnostic").unwrap().1;
        assert!(after_diagnostic.starts_with("\r\ngiclean:"), "{display}");
        assert!(after_diagnostic.contains("git for-each-ref failed"));
        assert!(!after_diagnostic.contains('\x1b'), "{display}");
        assert!(!display.contains('✘'), "{display}");
        assert!(!display.contains(": deleted"), "{display}");
        assert!(repo.exists("merged"));
    }

    #[test]
    fn terminal_remote_diagnostics_survive_progress_resuming() {
        for diagnostic in ["remote diagnostic", "remote diagnostic\\n"] {
            let repo = Repo::new();
            repo.git(&["branch", "merged"]);
            let before = repo.git(&["show-ref"]);
            let hook =
                format!("if [ \"$1\" = ls-remote ]; then printf '{diagnostic}' >&2; sleep 0.3; fi");
            let mut command = shim(&repo, &hook);
            command.arg("--dry-run");
            let output = in_terminal(&command);
            assert_success(&output);
            // Nested terminal output can pass through ONLCR twice. Repeated
            // carriage returns have the same visible effect as a single one.
            let display = text(&output.stdout).replace("\r\r\n", "\r\n");
            let after_diagnostic = display.split_once("remote diagnostic").unwrap().1;
            assert!(
                after_diagnostic.starts_with("\r\n\r\x1b[2K⠋ Checking merged branches..."),
                "{display}"
            );
            assert!(display.ends_with("candidate: merged"), "{display}");
            assert_eq!(repo.git(&["show-ref"]), before);
            assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        }
    }

    #[test]
    fn terminal_remote_diagnostics_are_separated_from_errors() {
        for diagnostic in ["remote diagnostic", "remote diagnostic\\n"] {
            let repo = Repo::new();
            repo.git(&["branch", "merged"]);
            let before = repo.git(&["show-ref"]);
            let hook = format!(
                "if [ \"$1\" = ls-remote ]; then printf '{diagnostic}' >&2; sleep 0.3; exit 23; fi"
            );
            let output = in_terminal(&shim(&repo, &hook));
            let display = text(&output.stdout).replace("\r\r\n", "\r\n");
            let after_diagnostic = display.split_once("remote diagnostic").unwrap().1;
            assert_eq!(
                after_diagnostic, "\r\ngiclean: git ls-remote failed (exit status: 23)",
                "{display}"
            );
            assert_eq!(repo.git(&["show-ref"]), before);
            assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        }
    }

    #[test]
    fn dumb_terminals_only_show_results() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let mut command = repo.cli();
        command.args(["br", "--dry-run"]).env("TERM", "dumb");
        let output = in_terminal(&command);
        assert_success(&output);
        let display = text(&output.stdout);
        // BSD script can echo EOF as ^D followed by backspaces before output.
        assert!(!display.contains('\x1b'), "{display}");
        assert!(!display.contains("giclean:"), "{display}");
        assert!(display.ends_with("candidate: merged"), "{display}");
        assert_eq!(display.lines().count(), 1, "{display}");
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

    // These tests exercise the individual fallback's precise failure boundary.
    fn individual_shim(repo: &Repo, hook: &str) -> Command {
        let hooks = repo.root.join("user-hooks");
        fs::create_dir(&hooks).unwrap();
        let path = hooks.join("reference-transaction");
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        checked(
            repo.git_command()
                .args(["config", "core.hooksPath"])
                .arg(hooks),
        );
        shim(repo, hook)
    }

    #[test]
    fn bulk_deletion_preserves_protection_cleans_config_and_uses_one_delete() {
        for format in ["sha1", "sha256"] {
            let repo = Repo::with_format(format);
            for name in ["topic-a", "日本語/完了", "used", "master", "production"] {
                repo.git(&["branch", name]);
            }
            for name in ["topic-a", "日本語/完了"] {
                repo.git(&["config", &format!("branch.{name}.description"), "cleanup"]);
            }
            repo.add_worktree("used", "linked");
            let output = shim(&repo, "").output().unwrap();
            assert_success(&output);
            assert_eq!(
                text(&output.stdout),
                "topic-a: deleted\n日本語/完了: deleted"
            );
            for name in ["main", "used", "master", "production"] {
                assert!(repo.exists(name));
            }
            for name in ["topic-a", "日本語/完了"] {
                assert!(!repo.exists(name));
            }
            assert!(
                repo.git_command()
                    .args([
                        "config",
                        "--get-regexp",
                        r"^branch\.(topic-a|日本語/完了)\."
                    ])
                    .output()
                    .unwrap()
                    .stdout
                    .is_empty()
            );
            let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
            assert_eq!(calls.lines().filter(|c| *c == "-c").count(), 1);
            assert_eq!(calls.lines().filter(|c| *c == "for-each-ref").count(), 3);
            assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        }
    }

    #[test]
    fn bulk_rechecks_changed_tip_before_committing() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        repo.git(&["switch", "-c", "future"]);
        fs::write(repo.work.join("feature"), "unmerged\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "unmerged"]);
        let future = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["switch", "main"]);
        let hook =
            format!("if [ \"$1\" = -c ]; then \"$real_git\" update-ref refs/heads/z {future}; fi");
        let output = shim(&repo, &hook).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "a: deleted");
        assert_eq!(repo.git(&["rev-parse", "z"]), future);
        assert!(repo.exists("future"));
    }

    #[test]
    fn bulk_preserves_a_branch_that_becomes_symbolic_with_the_same_oid() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        let hook =
            "if [ \"$1\" = -c ]; then \"$real_git\" symbolic-ref refs/heads/z refs/heads/main; fi";
        let output = shim(&repo, hook).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "a: deleted");
        assert_eq!(
            repo.git(&["symbolic-ref", "refs/heads/z"]),
            "refs/heads/main"
        );
        assert!(repo.exists("main"));
    }

    #[test]
    fn bulk_setup_failure_and_large_sets_use_individual_deletion() {
        for count in [2, 129] {
            let repo = Repo::new();
            let oid = repo.git(&["rev-parse", "HEAD"]);
            let refs: String = (0..count)
                .map(|i| format!("create refs/heads/topic-{i:03} {oid}\n"))
                .collect();
            repo.git_input(&["update-ref", "--stdin"], refs.as_bytes());
            let invalid_tmp = repo.root.join("not-a-directory");
            fs::write(&invalid_tmp, "file").unwrap();
            let output = shim(&repo, "").env("TMPDIR", invalid_tmp).output().unwrap();
            assert_success(&output);
            assert_eq!(text(&output.stdout).lines().count(), count);
            let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
            assert!(!calls.lines().any(|c| c == "-c"));
            assert_eq!(
                repo.git(&["for-each-ref", "--format=%(refname)", "refs/heads/"]),
                "refs/heads/main"
            );
        }
    }

    #[test]
    fn bulk_dry_run_keeps_individual_validation_and_all_refs() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        let before = repo.git(&["show-ref"]);
        let output = shim(&repo, "if [ \"$1\" = -c ]; then exit 99; fi")
            .arg("--dry-run")
            .output()
            .unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "candidate: a\ncandidate: z");
        assert_eq!(repo.git(&["show-ref"]), before);
        let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|c| *c == "for-each-ref").count(), 4);
    }

    #[test]
    fn bulk_rechecks_worktree_created_after_git_initial_worktree_check() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        let linked = repo.root.join("late-linked");
        let marker = repo.root.join("mutation");
        let disabled_hooks = format!("core.hooksPath={}", repo.root.join("no-hooks").display());
        let hook = format!(
            "if [ \"$1\" = for-each-ref ] && [ -n \"${{GICLEAN_GUARD_DIRECTORY:-}}\" ] && [ ! -f {} ]; then\ntouch {}\n\"$real_git\" -c {} worktree add --no-checkout -q {} z\nfi",
            shell_quote(&marker),
            shell_quote(&marker),
            shell_quote(Path::new(&disabled_hooks)),
            shell_quote(&linked)
        );
        let output = shim(&repo, &hook).output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), "a: deleted");
        assert!(marker.exists());
        assert!(repo.exists("z"));
        assert!(!repo.exists("a"));
        assert!(
            repo.git(&["worktree", "list", "--porcelain"])
                .contains("branch refs/heads/z")
        );
    }

    #[test]
    fn bulk_guard_read_errors_stop_without_fallback_or_deletions() {
        for response in ["exit 91", "printf 'invalid\\n'; exit 0"] {
            let repo = Repo::new();
            for name in ["a", "z"] {
                repo.git(&["branch", name]);
            }
            let guards = repo.work.join("guards");
            fs::create_dir(&guards).unwrap();
            let sub = repo.work.join("sub");
            fs::create_dir(&sub).unwrap();
            let hook = format!(
                "if [ \"$1\" = for-each-ref ] && [ -n \"${{GICLEAN_GUARD_DIRECTORY:-}}\" ]; then {response}; fi"
            );
            let output = shim(&repo, &hook)
                .current_dir(sub)
                .env("TMPDIR", "../guards")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            for name in ["a", "z"] {
                assert!(repo.exists(name));
            }
            assert_eq!(fs::read_dir(guards).unwrap().count(), 0);
            let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
            assert!(!calls.lines().any(|c| c == "branch"));
        }
    }

    #[test]
    fn bulk_existing_relative_hook_veto_is_preserved_from_subdirectory() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        let hooks = repo.work.join("policy");
        fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("reference-transaction");
        fs::write(
            &hook,
            "#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\nprintf 'veto\\n' >&2\nexit 42\n",
        )
        .unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
        repo.git(&["config", "core.hooksPath", "policy"]);
        let sub = repo.work.join("sub");
        fs::create_dir(&sub).unwrap();
        let output = repo.cli().arg("br").current_dir(sub).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(text(&output.stdout).contains("veto"));
        for name in ["a", "z"] {
            assert!(repo.exists(name));
        }
    }

    #[test]
    fn bulk_reports_errors_after_refs_were_deleted() {
        let repo = Repo::new();
        for name in ["a", "z"] {
            repo.git(&["branch", name]);
        }
        let hook = "if [ \"$1\" = -c ]; then \"$real_git\" --no-lazy-fetch \"$@\"; printf 'cleanup failed\\n' >&2; exit 23; fi";
        let output = shim(&repo, hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            text(&output.stdout),
            "a: failed (cleanup failed)\nz: failed (cleanup failed)"
        );
        for name in ["a", "z"] {
            assert!(!repo.exists(name));
        }
    }

    #[test]
    fn bulk_skips_branches_removed_concurrently_before_the_batch() {
        for removed in ["z", "a z"] {
            let repo = Repo::new();
            for name in ["a", "z"] {
                repo.git(&["branch", name]);
            }
            let hook = format!(
                "if [ \"$1\" = -c ]; then for name in {removed}; do \"$real_git\" update-ref -d refs/heads/$name; done; fi"
            );
            let output = shim(&repo, &hook).output().unwrap();
            assert_success(&output);
            assert_eq!(
                text(&output.stdout),
                if removed == "z" { "a: deleted" } else { "" }
            );
            assert!(!repo.exists("a"));
            assert!(!repo.exists("z"));
        }
    }

    // The SSH boundary is simulated; the advertised refs come from a real local
    // upload-pack process, without network access or user SSH configuration.
    fn ssh_transport(repo: &Repo) -> Command {
        repo.git(&["branch", "merged"]);
        repo.git(&[
            "remote",
            "set-url",
            "origin",
            "git@example.invalid:repo.git",
        ]);
        let output = checked(Command::new("sh").args(["-c", "command -v git"]));
        let real_git = PathBuf::from(text(&output.stdout));
        let bin = repo.root.join("ssh-bin");
        fs::create_dir(&bin).unwrap();
        let path = bin.join("ssh");
        fs::write(&path, format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$1\" >> {}\nif [ \"$1\" = -V ]; then\nprintf '%s\\n' \"${{TEST_SSH_VERSION:-OpenSSH_9.9p2}}\" >&2\nexit 0\nfi\nif [ \"$1\" = -G ]; then\nexit 0\nfi\nprintf '%s\\n' \"$@\" > {}\nprintf '%s\\n' \"${{GIT_PROTOCOL:-}}\" > {}\nexec {} upload-pack {}\n",
            shell_quote(&repo.root.join("ssh-calls")),
            shell_quote(&repo.root.join("ssh-args")),
            shell_quote(&repo.root.join("git-protocol")),
            shell_quote(&real_git),
            shell_quote(&repo.origin),
        )).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = repo.cli();
        command
            .args(["br", "--dry-run"])
            .env("PATH", std::env::join_paths(paths).unwrap());
        command
    }

    #[test]
    fn ssh_default_protocol_and_explicit_protocols_return_the_same_candidates() {
        for format in ["sha1", "sha256"] {
            for setting in ["default", "repository", "environment"] {
                let repo = Repo::with_format(format);
                let mut command = ssh_transport(&repo);
                match setting {
                    "repository" => {
                        repo.git(&["config", "protocol.version", "2"]);
                    }
                    "environment" => {
                        command
                            .env("GIT_CONFIG_COUNT", "1")
                            .env("GIT_CONFIG_KEY_0", "protocol.version")
                            .env("GIT_CONFIG_VALUE_0", "2");
                    }
                    _ => {}
                }
                let output = checked(&mut command);
                assert_eq!(text(&output.stdout), "candidate: merged");
                let protocol = fs::read_to_string(repo.root.join("git-protocol")).unwrap();
                assert_eq!(
                    protocol.trim(),
                    if setting == "default" {
                        ""
                    } else {
                        "version=2"
                    },
                    "{format}: {setting}"
                );
                let args = fs::read_to_string(repo.root.join("ssh-args")).unwrap();
                assert!(args.contains("ControlMaster=auto"));
                assert!(args.contains("SendEnv=GIT_PROTOCOL"));
                assert!(repo.exists("merged"));
                assert!(!repo.work.join(".git/FETCH_HEAD").exists());
            }
        }
    }

    #[test]
    fn openssh_queries_use_the_same_private_control_path_across_invocations() {
        let repo = Repo::new();
        let mut command = ssh_transport(&repo);
        let output = checked(&mut command);
        assert_eq!(text(&output.stdout), "candidate: merged");
        let first = fs::read_to_string(repo.root.join("ssh-args")).unwrap();
        assert!(first.lines().any(|arg| arg == "ControlMaster=auto"));
        assert!(first.lines().any(|arg| arg == "ControlPersist=60"));
        assert!(first.lines().any(|arg| arg.starts_with("ControlPath=/tmp/giclean-ssh-") && arg.ends_with("-%C")));
        let output = checked(&mut command);
        assert_eq!(text(&output.stdout), "candidate: merged");
        assert_eq!(
            fs::read_to_string(repo.root.join("ssh-args")).unwrap(),
            first
        );
        assert!(repo.exists("merged"));
    }

    #[test]
    fn reused_ssh_transport_still_reads_the_latest_remote_head() {
        let repo = Repo::new();
        let mut command = ssh_transport(&repo);
        let first = checked(&mut command);
        assert_eq!(text(&first.stdout), "candidate: merged");
        let control = fs::read_to_string(repo.root.join("ssh-args")).unwrap();

        let other = repo.root.join("other");
        checked(
            repo.git_command()
                .arg("clone")
                .arg(&repo.origin)
                .arg(&other),
        );
        checked(repo.git_command().current_dir(&other).args([
            "commit",
            "--allow-empty",
            "-m",
            "new remote commit",
        ]));
        checked(
            repo.git_command()
                .current_dir(&other)
                .args(["push", "origin", "main"]),
        );

        let second = command.output().unwrap();
        assert_eq!(second.status.code(), Some(1));
        assert!(second.stdout.is_empty());
        assert!(text(&second.stderr).contains("git fetch origin"));
        assert_eq!(
            fs::read_to_string(repo.root.join("ssh-args")).unwrap(),
            control
        );
        assert!(repo.exists("merged"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn explicit_ssh_commands_and_variants_are_preserved() {
        for setting in [
            "GIT_SSH_COMMAND",
            "GIT_SSH",
            "GIT_SSH_VARIANT",
            "core.sshCommand",
            "ssh.variant",
        ] {
            let repo = Repo::new();
            let mut command = ssh_transport(&repo);
            match setting {
                "GIT_SSH_COMMAND" => {
                    command.env(setting, "ssh -o IdentityFile=/test/user-key");
                }
                "GIT_SSH" => {
                    command.env(setting, repo.root.join("ssh-bin/ssh"));
                }
                "GIT_SSH_VARIANT" => {
                    command.env(setting, "simple");
                }
                "core.sshCommand" => {
                    repo.git(&["config", setting, "ssh -o IdentityFile=/test/user-key"]);
                }
                "ssh.variant" => {
                    repo.git(&["config", setting, "simple"]);
                }
                _ => unreachable!(),
            }
            let output = checked(&mut command);
            assert_eq!(text(&output.stdout), "candidate: merged", "{setting}");
            let args = fs::read_to_string(repo.root.join("ssh-args")).unwrap();
            assert!(!args.contains("ControlMaster"), "{setting}: {args}");
            if matches!(setting, "GIT_SSH_COMMAND" | "core.sshCommand") {
                assert!(args.contains("IdentityFile=/test/user-key"), "{setting}");
            }
            assert!(
                !fs::read_to_string(repo.root.join("ssh-calls"))
                    .unwrap()
                    .lines()
                    .any(|arg| arg == "-V"),
                "{setting}"
            );
        }
    }

    #[test]
    fn unsupported_ssh_implementations_keep_the_native_transport() {
        let repo = Repo::new();
        let mut command = ssh_transport(&repo);
        command.env("TEST_SSH_VERSION", "Dropbear ssh");
        let output = checked(&mut command);
        assert_eq!(text(&output.stdout), "candidate: merged");
        let args = fs::read_to_string(repo.root.join("ssh-args")).unwrap();
        assert!(!args.contains("ControlMaster"));
    }

    #[test]
    fn local_remotes_do_not_probe_or_launch_ssh() {
        let repo = Repo::new();
        let mut command = ssh_transport(&repo);
        checked(
            repo.git_command()
                .args(["remote", "set-url", "origin"])
                .arg(&repo.origin),
        );
        let output = checked(&mut command);
        assert_eq!(text(&output.stdout), "candidate: merged");
        assert!(!repo.root.join("ssh-calls").exists());
    }

    #[test]
    fn queries_the_remote_once_without_plain_progress() {
        let repo = Repo::new();
        repo.git(&["branch", "merged"]);
        let output = shim(&repo, "").arg("--dry-run").output().unwrap();
        assert!(output.stderr.is_empty());
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
    fn identical_trees_settle_eligibility_without_diffing_files() {
        for format in ["sha1", "sha256"] {
            for target_matches in ["base", "tip"] {
                let repo = Repo::with_format(format);
                let base = repo.git(&["rev-parse", "HEAD"]);
                let base_tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
                let blob = repo.git_input(&["hash-object", "-w", "--stdin"], b"feature\n");
                let tree = repo.git_input(
                    &["mktree"],
                    format!("100755 blob {blob}\tfeature\n").as_bytes(),
                );
                let tip = repo.git(&["commit-tree", &tree, "-p", &base, "-m", "feature"]);
                let empty = repo.git(&[
                    "commit-tree",
                    &base_tree,
                    "-p",
                    &base,
                    "-m",
                    "no net changes",
                ]);
                let target_tree = if target_matches == "base" {
                    &base_tree
                } else {
                    &tree
                };
                let target =
                    repo.git(&["commit-tree", target_tree, "-p", &base, "-m", "integration"]);
                repo.git_input(
                    &["update-ref", "--stdin"],
                    format!(
                        "create refs/heads/topic {tip}\ncreate refs/heads/alias {tip}\ncreate refs/heads/empty {empty}\nupdate refs/heads/main {target}\n"
                    ).as_bytes(),
                );
                repo.git(&["reset", "--hard", "main"]);
                repo.git(&["push", "origin", "main"]);
                let before = repo.git(&["show-ref"]);
                let mut command = shim(&repo, "if [ \"$1\" = diff-tree ]; then exit 91; fi");
                let output = command.arg("--dry-run").output().unwrap();
                assert_success(&output);
                let expected = if target_matches == "tip" {
                    "candidate: alias\ncandidate: topic"
                } else {
                    ""
                };
                assert_eq!(text(&output.stdout), expected);
                assert_eq!(repo.git(&["show-ref"]), before);
                let output = repo.run(&["br"]);
                assert_success(&output);
                assert_eq!(repo.exists("topic"), target_matches == "base");
                assert_eq!(repo.exists("alias"), target_matches == "base");
                assert!(repo.exists("empty"));
                assert!(repo.exists("main"));
                assert!(!repo.work.join(".git/FETCH_HEAD").exists());
                let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
                assert!(!calls.lines().any(|call| call == "diff-tree"));
                assert_eq!(calls.lines().filter(|call| *call == "ls-remote").count(), 1);
            }
        }
    }

    #[test]
    fn squash_detection_errors_stop_before_any_deletion() {
        for condition in [
            "[ \"$1\" = rev-parse ] && [ \"${2:-}\" = --revs-only ]",
            "[ \"$1\" = rev-parse ] && case \"${4:-}\" in *'^{tree}') true ;; *) false ;; esac",
            "[ \"$1\" = diff-tree ]",
        ] {
            let repo = Repo::new();
            repo.git(&["branch", "a-merged"]);
            repo.git(&["switch", "-c", "squash"]);
            fs::write(repo.work.join("feature"), "feature\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "feature"]);
            repo.git(&["switch", "main"]);
            repo.git(&["merge", "--squash", "squash"]);
            fs::write(repo.work.join("unrelated"), "independent change\n").unwrap();
            repo.git(&["add", "unrelated"]);
            repo.git(&["commit", "-m", "squash merge"]);
            repo.git(&["push", "origin", "main"]);
            let hook = format!("if {condition}; then exit 91; fi");
            let output = shim(&repo, &hook).output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(repo.exists("a-merged"));
            assert!(repo.exists("squash"));
        }
    }

    #[test]
    fn historical_squash_errors_stop_before_any_deletion() {
        for failure in ["query", "query-response", "diff", "diff-response"] {
            let repo = Repo::new();
            repo.git(&["branch", "a-merged"]);
            repo.git(&["switch", "-c", "squash"]);
            fs::write(repo.work.join("feature"), "feature\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "feature"]);
            repo.git(&["switch", "main"]);
            repo.git(&["merge", "--squash", "squash"]);
            repo.git(&["commit", "-m", "integration"]);
            fs::write(repo.work.join("feature"), "later edit\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "later edit"]);
            repo.git(&["push", "origin", "main"]);
            let hook = match failure {
                "query" => "if [ \"$1\" = rev-list ]; then exit 91; fi",
                "query-response" => {
                    "if [ \"$1\" = rev-list ]; then printf 'invalid\\n'; exit 0; fi"
                }
                "diff" => {
                    "if [ \"$1\" = rev-list ]; then touch ../history-queried; fi\nif [ \"$1\" = diff-tree ] && [ -f ../history-queried ]; then exit 91; fi"
                }
                _ => {
                    "if [ \"$1\" = rev-list ]; then touch ../history-queried; fi\nif [ \"$1\" = diff-tree ] && [ -f ../history-queried ]; then cat > /dev/null; printf 'invalid\\n'; exit 0; fi"
                }
            };
            let before = repo.git(&["show-ref"]);
            let output = shim(&repo, hook).output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{failure}");
            assert!(output.stdout.is_empty(), "{failure}");
            assert_eq!(repo.git(&["show-ref"]), before, "{failure}");
            assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        }
    }

    #[test]
    fn squash_batches_large_diffs_and_shared_trees_without_changing_eligibility() {
        let repo = Repo::with_format("sha256");
        let base = repo.git(&["rev-parse", "HEAD"]);
        let empty_tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
        let blob = repo.git_input(&["hash-object", "-w", "--stdin"], b"feature\n");
        let other_blob = repo.git_input(&["hash-object", "-w", "--stdin"], b"unmerged\n");
        let paths: Vec<_> = (0..1000)
            .map(|i| {
                if i == 5 {
                    format!("line\n{empty_tree} {empty_tree}\tfile")
                } else {
                    format!("file-{i:04}")
                }
            })
            .collect();
        let rows: String = paths
            .iter()
            .map(|path| format!("100644 blob {blob}\t{path}\0"))
            .collect();
        let target_tree = repo.git_input(&["mktree", "-z"], rows.as_bytes());
        let target = repo.git(&[
            "commit-tree",
            &target_tree,
            "-p",
            &base,
            "-m",
            "squash integrations",
        ]);
        let mut refs = format!("update refs/heads/main {target}\n");
        let mut expected = vec!["candidate: alias-000".to_owned()];
        for (i, path) in paths.iter().take(130).enumerate() {
            let content = if i == 17 { &other_blob } else { &blob };
            let tree = repo.git_input(
                &["mktree", "-z"],
                format!("100644 blob {content}\t{path}\0").as_bytes(),
            );
            let tip = repo.git(&[
                "commit-tree",
                &tree,
                "-p",
                &base,
                "-m",
                &format!("topic {i}"),
            ]);
            refs.push_str(&format!("create refs/heads/topic-{i:03} {tip}\n"));
            if i == 0 {
                refs.push_str(&format!("create refs/heads/alias-000 {tip}\n"));
            }
            if i != 17 {
                expected.push(format!("candidate: topic-{i:03}"));
            }
        }
        let empty = repo.git(&[
            "commit-tree",
            &empty_tree,
            "-p",
            &base,
            "-m",
            "no net change",
        ]);
        let unrelated = repo.git(&["commit-tree", &target_tree, "-m", "unrelated root"]);
        refs.push_str(&format!(
            "create refs/heads/empty {empty}\ncreate refs/heads/unrelated {unrelated}\n"
        ));
        repo.git_input(&["update-ref", "--stdin"], refs.as_bytes());
        repo.git(&["reset", "--hard", "main"]);
        repo.git(&["push", "origin", "main"]);
        let before = repo.git(&["show-ref"]);
        let output = shim(&repo, "").arg("--dry-run").output().unwrap();
        assert_success(&output);
        assert_eq!(text(&output.stdout), expected.join("\n"));
        assert_eq!(repo.git(&["show-ref"]), before);
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        let calls = fs::read_to_string(repo.root.join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|call| *call == "rev-parse").count(), 5);
        assert_eq!(calls.lines().filter(|call| *call == "diff-tree").count(), 2);
        assert_eq!(calls.lines().filter(|call| *call == "ls-remote").count(), 1);
        assert!(
            !calls
                .lines()
                .any(|call| matches!(call, "merge-base" | "fetch" | "pull" | "push"))
        );
    }

    #[test]
    fn malformed_batch_responses_stop_before_any_deletion() {
        for condition in [
            "[ \"$1\" = rev-parse ] && [ \"${2:-}\" = --revs-only ]",
            "[ \"$1\" = rev-parse ] && case \"${4:-}\" in *'^{tree}') true ;; *) false ;; esac",
            "[ \"$1\" = diff-tree ]",
        ] {
            let repo = Repo::new();
            repo.git(&["branch", "a-merged"]);
            repo.git(&["switch", "-c", "squash"]);
            fs::write(repo.work.join("feature"), "feature\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "feature"]);
            repo.git(&["switch", "main"]);
            repo.git(&["merge", "--squash", "squash"]);
            fs::write(repo.work.join("unrelated"), "independent change\n").unwrap();
            repo.git(&["add", "unrelated"]);
            repo.git(&["commit", "-m", "squash merge"]);
            repo.git(&["push", "origin", "main"]);
            let hook = format!(
                "if {condition}; then\nif [ \"$1\" = diff-tree ]; then cat > /dev/null; fi\nprintf 'invalid\\n'; exit 0; fi"
            );
            let output = shim(&repo, &hook).output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(repo.exists("a-merged"));
            assert!(repo.exists("squash"));
        }
    }

    #[test]
    fn squash_rechecks_tips_and_worktree_use_before_deletion() {
        for change in ["tip", "worktree"] {
            let repo = Repo::new();
            repo.git(&["switch", "-c", "squash"]);
            fs::write(repo.work.join("feature"), "feature\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "feature"]);
            let old = repo.git(&["rev-parse", "HEAD"]);
            repo.git(&["switch", "main"]);
            repo.git(&["merge", "--squash", "squash"]);
            repo.git(&["commit", "-m", "squash merge"]);
            repo.git(&["push", "origin", "main"]);
            let mutation = if change == "tip" {
                format!("\"$real_git\" update-ref refs/heads/squash main {old}")
            } else {
                format!(
                    "\"$real_git\" worktree add -q {} squash",
                    shell_quote(&repo.root.join("late-worktree"))
                )
            };
            let hook = format!(
                "if [ \"$1\" = for-each-ref ] && [ \"${{4:-}}\" = refs/heads/squash ]; then\n{mutation}\nfi"
            );
            let output = shim(&repo, &hook).output().unwrap();
            assert_success(&output);
            assert_eq!(text(&output.stdout), "");
            assert!(repo.exists("squash"));
        }
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
        assert_eq!(text(&output.stdout), "merged: deleted");
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
        assert_eq!(text(&output.stdout), "");
        assert!(repo.exists("moving"));
        assert_eq!(
            repo.git(&["rev-parse", "moving"]),
            repo.git(&["rev-parse", "main"])
        );
    }

    #[test]
    fn deletion_failure_returns_one_and_reports_successful_branches() {
        let repo = Repo::new();
        repo.git(&["branch", "a-blocked"]);
        repo.git(&["branch", "z-merged"]);
        let hook = format!(
            "if [ \"$1\" = branch ] && [ \"${{5:-}}\" = a-blocked ]; then\n\"$real_git\" worktree add -q {} a-blocked\nfi",
            shell_quote(&repo.root.join("late-worktree")),
        );
        let output = individual_shim(&repo, &hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            text(&output.stdout),
            "a-blocked: failed (checked out in a worktree)\nz-merged: deleted"
        );
        assert!(repo.exists("a-blocked"));
        assert!(!repo.exists("z-merged"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn deletion_lock_errors_preserve_the_reason_and_continue() {
        let repo = Repo::new();
        repo.git(&["branch", "a-locked"]);
        repo.git(&["branch", "z-merged"]);
        fs::write(repo.work.join(".git/refs/heads/a-locked.lock"), "").unwrap();
        let output = repo.run(&["br"]);
        assert_eq!(output.status.code(), Some(1));
        let results = text(&output.stdout);
        let rows: Vec<_> = results.lines().collect();
        assert_eq!(rows.len(), 2, "{results}");
        assert!(rows[0].starts_with("a-locked: failed ("), "{results}");
        assert!(rows[0].contains("a-locked.lock"), "{results}");
        assert!(rows[0].contains("File exists"), "{results}");
        assert_eq!(rows[1], "z-merged: deleted");
        assert!(output.stderr.is_empty());
        assert!(repo.exists("a-locked"));
        assert!(!repo.exists("z-merged"));
    }

    #[test]
    fn deletion_failure_without_diagnostics_reports_the_exit_status() {
        let repo = Repo::new();
        repo.git(&["branch", "a-blocked"]);
        repo.git(&["branch", "z-merged"]);
        let hook = "if [ \"$1\" = branch ]; then [ \"$LC_ALL\" = C ] || exit 90; if [ \"${5:-}\" = a-blocked ]; then exit 23; fi; fi";
        let output = individual_shim(&repo, hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            text(&output.stdout),
            "a-blocked: failed (git branch failed with exit status: 23)\nz-merged: deleted"
        );
        assert!(output.stderr.is_empty());
        assert!(repo.exists("a-blocked"));
        assert!(!repo.exists("z-merged"));
    }

    #[test]
    fn terminal_deletion_failures_are_reported_after_clearing_the_spinner() {
        let repo = Repo::new();
        repo.git(&["branch", "a-blocked"]);
        repo.git(&["branch", "z-merged"]);
        let hook = format!(
            "if [ \"$1\" = branch ] && [ \"${{5:-}}\" = a-blocked ]; then\n\"$real_git\" worktree add -q {} a-blocked\nsleep 0.3\nfi",
            shell_quote(&repo.root.join("late-worktree")),
        );
        let output = in_terminal(&individual_shim(&repo, &hook));
        let display = text(&output.stdout);
        let (progress, results) = display.split_once("a-blocked: failed").unwrap();
        assert!(!progress.contains('\n'), "{display}");
        assert!(progress.ends_with("\r\x1b[2K"), "{display}");
        assert_eq!(results, " (checked out in a worktree)\r\nz-merged: deleted");
        assert!(repo.exists("a-blocked"));
        assert!(!repo.exists("z-merged"));
    }

    #[test]
    fn json_recheck_failure_keeps_prior_deletions_and_unattempted_targets() {
        let repo = Repo::new();
        for branch in ["a-merged", "b-unreadable", "z-merged"] {
            repo.git(&["branch", branch]);
        }
        let hook = "if [ \"$1\" = for-each-ref ] && [ \"${4:-}\" = refs/heads/b-unreadable ]; then\nexit 91\nfi";
        let output = individual_shim(&repo, hook)
            .args(["--format", "json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let document = json_support::parse(&output.stdout);
        assert_eq!(document.get("error").get("code").string(), "cleanup_failed");
        let results = document.get("executions").array();
        assert!(
            results
                .iter()
                .any(|r| r.get("target").get("name").string() == "a-merged"
                    && r.get("outcome").string() == "deleted")
        );
        assert!(
            results
                .iter()
                .any(|r| r.get("target").get("name").string() == "z-merged"
                    && r.get("outcome").string() == "not_attempted")
        );
        assert!(!repo.exists("a-merged"));
        assert!(repo.exists("b-unreadable"));
        assert!(repo.exists("z-merged"));
    }

    #[test]
    fn json_bulk_cleanup_failure_does_not_claim_unchanged_refs() {
        let repo = Repo::new();
        repo.git(&["branch", "a"]);
        repo.git(&["branch", "z"]);
        let hook = "if [ \"$1\" = -c ]; then \"$real_git\" --no-lazy-fetch \"$@\"; printf 'cleanup failed\\n' >&2; exit 23; fi";
        let output = shim(&repo, hook)
            .args(["--format", "json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let document = json_support::parse(&output.stdout);
        for name in ["a", "z"] {
            let result = document
                .get("executions")
                .array()
                .iter()
                .find(|r| r.get("target").get("name").string() == name)
                .unwrap();
            assert_eq!(result.get("outcome").string(), "failed");
            assert!(result.get("detail").string().contains("cleanup failed"));
            assert!(!repo.exists(name));
        }
    }

    #[test]
    fn recheck_failure_stops_remaining_deletions_and_reports_partial_progress() {
        let repo = Repo::new();
        for branch in ["a-merged", "b-unreadable", "z-merged"] {
            repo.git(&["branch", branch]);
        }
        let hook = "if [ \"$1\" = for-each-ref ] && [ \"${4:-}\" = refs/heads/b-unreadable ]; then\nexit 91\nfi";
        let output = individual_shim(&repo, hook).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(text(&output.stdout), "a-merged: deleted");
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

#[test]
fn json_br_dry_run_and_delete_preserve_names_and_protection() {
    let repo = Repo::new();
    let branch = "日本語-\"quoted\"";
    repo.git(&["branch", branch]);
    let preview = checked(repo.cli().args(["br", "--format", "json", "--dry-run"]));
    let document = json_support::parse(&preview.stdout);
    assert_eq!(
        document.get("schema_version"),
        &json_support::Json::Number(1)
    );
    assert_eq!(document.get("dry_run"), &json_support::Json::Bool(true));
    assert_eq!(document.get("success"), &json_support::Json::Bool(true));
    assert_eq!(document.get("error"), &json_support::Json::Null);
    let result = document
        .get("executions")
        .array()
        .iter()
        .find(|r| r.get("target").get("name").string() == branch)
        .unwrap();
    assert_eq!(result.get("outcome").string(), "candidate");
    assert!(
        document
            .get("decisions")
            .array()
            .iter()
            .any(|d| d.get("target").get("name").string() == "main"
                && d.get("eligibility").string() == "protected")
    );
    assert!(repo.exists(branch));
    let deleted = checked(repo.cli().args(["br", "--format", "json"]));
    let document = json_support::parse(&deleted.stdout);
    assert!(
        document
            .get("executions")
            .array()
            .iter()
            .any(|r| r.get("target").get("name").string() == branch
                && r.get("outcome").string() == "deleted")
    );
    assert!(!repo.exists(branch));
    assert!(repo.exists("main"));
}

#[test]
fn json_br_empty_and_runtime_error_are_complete_documents() {
    let repo = Repo::new();
    repo.git(&["remote", "remove", "origin"]);
    let output = checked(repo.cli().args(["br", "--format", "json"]));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("success"), &json_support::Json::Bool(true));
    repo.git(&["branch", "topic"]);
    let output = repo.run(&["br", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("success"), &json_support::Json::Bool(false));
    assert_eq!(document.get("error").get("code").string(), "cleanup_failed");
    assert!(repo.exists("topic"));
}

#[test]
fn json_invalid_arguments_never_print_help_or_partial_json_to_stdout() {
    let repo = Repo::new();
    for args in [
        vec!["br", "--format"],
        vec!["br", "--format", "xml"],
        vec!["br", "--format", "json", "--format", "json"],
        vec!["br", "--format", "json", "--help"],
        vec!["wt", "--format", "json", "--help"],
        vec!["wt", "--format", "json"],
        vec!["wt", "--days", "--format", "json", "3"],
        vec!["--format", "json"],
    ] {
        let output = repo.cli().env("PATH", "").args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let output = checked(repo.cli().args(["br", "--format", "text", "--dry-run"]));
    assert!(output.stdout.is_empty());
}

#[test]
fn json_br_partial_deletion_failure_keeps_successful_results() {
    let repo = Repo::new();
    repo.git(&["branch", "a-locked"]);
    repo.git(&["branch", "z-merged"]);
    fs::write(repo.work.join(".git/refs/heads/a-locked.lock"), "").unwrap();
    let output = repo.run(&["br", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("success"), &json_support::Json::Bool(false));
    let results = document.get("executions").array();
    assert!(
        results
            .iter()
            .any(|r| r.get("target").get("name").string() == "a-locked"
                && r.get("outcome").string() == "failed"
                && r.get("reason_code").string() == "deletion_failed")
    );
    assert!(
        results
            .iter()
            .any(|r| r.get("target").get("name").string() == "z-merged"
                && r.get("outcome").string() == "deleted")
    );
    assert!(repo.exists("a-locked"));
    assert!(!repo.exists("z-merged"));
}

#[test]
fn json_and_explain_together_keep_stdout_machine_readable() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    for options in [
        vec!["br", "--format", "json", "--explain", "--dry-run"],
        vec!["br", "--dry-run", "--explain", "--format", "json"],
    ] {
        let output = checked(repo.cli().args(&options));
        let document = json_support::parse(&output.stdout);
        assert_eq!(document.get("success"), &json_support::Json::Bool(true));
        assert!(!text(&output.stdout).contains("explain:"));
        assert!(repo.exists("topic"));
    }
    let output = checked(
        repo.cli()
            .args(["br", "--explain", "--format", "text", "--dry-run"]),
    );
    assert!(text(&output.stdout).contains("explain:"));
}
