use super::*;

fn batch(repos: &[&Repo], options: &[&str]) -> Output {
    let mut command = repos[0].cli();
    command.args(["batch", "br"]).args(options);
    for repo in repos {
        command.arg(&repo.work);
    }
    command.output().unwrap()
}

#[test]
fn preview_then_execute_across_repositories_keeps_protected_branches() {
    let a = Repo::new();
    let b = Repo::new();
    for repo in [&a, &b] {
        repo.git(&["branch", "merged"]);
        repo.git(&["branch", "production"]);
    }
    let preview = batch(&[&a, &b], &["--dry-run", "--explain"]);
    assert_success(&preview);
    assert_eq!(text(&preview.stdout).matches("== Repository").count(), 2);
    assert!(text(&preview.stdout).contains("2 succeeded, 0 failed"));
    for repo in [&a, &b] {
        assert!(repo.exists("merged"));
    }
    let execute = batch(&[&a, &b], &["--execute"]);
    assert_success(&execute);
    for repo in [&a, &b] {
        assert!(!repo.exists("merged"));
        assert!(repo.exists("main"));
        assert!(repo.exists("production"));
    }
}

#[test]
fn a_repository_failure_does_not_stop_other_repositories() {
    let a = Repo::new();
    let b = Repo::new();
    a.git(&["branch", "merged"]);
    a.git(&["remote", "remove", "origin"]);
    b.git(&["branch", "merged"]);
    let output = batch(&[&a, &b], &["--execute"]);
    assert!(!output.status.success());
    assert!(!b.exists("merged"));
    assert!(text(&output.stdout).contains("1 succeeded, 1 failed"));
}

#[test]
fn discovery_errors_are_reported_but_valid_repositories_still_run() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    let output = repo
        .cli()
        .args(["batch", "br", "--dry-run"])
        .arg(repo.root.join("missing"))
        .arg(&repo.work)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output.stdout).contains("1 succeeded"));
    assert!(repo.exists("merged"));
}

#[test]
fn bare_repositories_are_skipped_and_linked_worktrees_are_deduplicated() {
    let repo = Repo::new();
    repo.git(&["branch", "linked"]);
    repo.add_worktree("linked", "linked-tree");
    repo.git(&["branch", "merged"]);
    let output = repo
        .cli()
        .args(["batch", "br", "--execute"])
        .arg(&repo.root)
        .arg(&repo.work)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(text(&output.stdout).contains("1 succeeded, 0 failed, 1 skipped"));
    assert!(repo.exists("linked"));
    assert!(!repo.exists("merged"));
}

#[test]
fn execution_requires_explicit_mode_and_valid_arguments_before_mutation() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    for options in [
        vec![],
        vec!["--execute", "--dry-run"],
        vec!["--execute", "--bogus"],
        vec!["--execute", "--format", "json"],
        vec!["--execute", "--max-depth", "-1"],
    ] {
        let output = batch(&[&repo], &options);
        assert_eq!(output.status.code(), Some(2));
        assert!(repo.exists("merged"));
    }
}

#[test]
fn ambient_git_repository_override_is_rejected_before_mutation() {
    let repo = Repo::new();
    let other = Repo::new();
    repo.git(&["branch", "merged"]);
    other.git(&["branch", "merged"]);
    let output = repo
        .cli()
        .args(["batch", "br", "--execute"])
        .arg(&repo.work)
        .env("GIT_DIR", other.work.join(".git"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("unset GIT_DIR"));
    assert!(repo.exists("merged"));
    assert!(other.exists("merged"));
}

#[test]
fn depth_and_exclusions_apply_to_batch() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    let output = repo
        .cli()
        .args(["batch", "br", "--execute", "--max-depth", "0"])
        .arg(&repo.root)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(repo.exists("merged"));
    let output = repo
        .cli()
        .args(["batch", "br", "--execute", "--exclude", "repo"])
        .arg(&repo.root)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(repo.exists("merged"));
}

#[test]
fn option_terminator_preserves_flag_shaped_root_paths() {
    let repo = Repo::new();
    let output = repo
        .cli()
        .args(["batch", "br", "--execute", "--", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("--help"));
    assert!(!text(&output.stdout).contains("Usage:"));
}

#[test]
fn bare_root_stays_skipped_when_launched_from_its_linked_worktree() {
    let repo = Repo::new();
    let linked = repo.root.join("bare-linked");
    checked(
        repo.git_command()
            .arg("--git-dir")
            .arg(&repo.origin)
            .args(["worktree", "add"])
            .arg(&linked)
            .arg("main"),
    );
    let output = checked(
        repo.cli()
            .current_dir(&linked)
            .args(["batch", "br", "--execute"])
            .arg(&repo.origin),
    );
    assert!(text(&output.stdout).contains("0 succeeded, 0 failed, 1 skipped"));
}
