use super::*;

#[test]
fn explain_wt_shows_age_proof_and_keeps_dry_run_safe() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("squash"));
    repo.pr(&tip, "topic", &merged, old_time());
    let output = checked(
        repo.wt_cli()
            .args(["wt", "-d", "3", "--explain", "--dry-run"]),
    );
    let output = text(&output.stdout);
    for fragment in [
        "candidate:",
        "eligible (verified PR merge",
        "verified PR merge:",
        "latest local activity:",
        "eligible at or before:",
        "primary worktree",
    ] {
        assert!(output.contains(fragment), "missing {fragment}: {output}");
    }
    assert!(tree.exists());
    let output = checked(repo.wt_cli().args(["wt", "--explain", "-d", "3"]));
    assert!(text(&output.stdout).contains(": deleted"));
    assert!(!tree.exists());
    assert!(repo.exists("topic"));
}

#[test]
fn explain_wt_shows_local_changes_recent_activity_and_unknown_pr() {
    for (case, expected) in [
        ("dirty", "local changes or untracked/ignored files"),
        ("index", "index flags may hide local changes"),
        ("recent", "later worktree activity restarted"),
        ("merge", "verified PR merge is too recent"),
        ("unknown", "no matching PR merge time"),
        ("locked", "worktree is locked"),
    ] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("topic", Some("merge"));
        if case != "unknown" {
            repo.pr(
                &tip,
                "topic",
                &merged,
                if case == "merge" {
                    SystemTime::now()
                } else {
                    old_time()
                },
            );
        }
        match case {
            "dirty" => fs::write(tree.join("notes"), "local").unwrap(),
            "index" => {
                checked(repo.git_command().current_dir(&tree).args([
                    "update-index",
                    "--assume-unchanged",
                    "topic",
                ]));
            }
            "recent" => set_time(&tree.join("topic"), SystemTime::now()),
            "locked" => {
                repo.git(&["worktree", "lock", tree.to_str().unwrap()]);
            }
            _ => {}
        }
        let output = checked(repo.wt_cli().args(["wt", "-d", "3", "--explain"]));
        assert!(
            text(&output.stdout).contains(expected),
            "{case}: {}",
            text(&output.stdout)
        );
        assert!(tree.exists());
    }
}

#[test]
fn explain_wt_primary_only_requires_no_origin() {
    let repo = Repo::new();
    repo.git(&["remote", "remove", "origin"]);
    let output = checked(repo.cli().args(["wt", "--explain", "-d", "3"]));
    assert!(text(&output.stdout).contains("primary worktree"));
}

#[test]
fn explain_wt_reports_recheck_races_without_removing_changed_tree() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("a", Some("merge"));
    repo.pr(&tip, "a", &merged, old_time());
    let (other, other_merged, _) = repo.topic("z", Some("merge"));
    repo.pr(&other, "z", &other_merged, old_time());
    fs::write(
        repo.root.join("gh-action"),
        format!("case $(/bin/cat \"$TEST_GH_ROOT/gh-calls\") in *{other}*) git worktree lock {};; esac\n", quote(&tree)),
    ).unwrap();
    let output = checked(repo.wt_cli().args(["wt", "-h", "12", "--explain"]));
    let output = text(&output.stdout);
    assert!(
        output.contains("state changed during inspection or before deletion"),
        "{output}"
    );
    assert!(tree.exists());
    assert!(!output.contains(&format!("{}: deleted", tree.display())));
}

#[test]
fn explain_wt_query_failure_retains_error_exit_and_all_trees() {
    let repo = Repo::wt();
    let (_, _, tree) = repo.topic("topic", Some("merge"));
    fs::write(repo.root.join("gh-fail"), "").unwrap();
    let output = repo
        .wt_cli()
        .args(["wt", "-d", "3", "--explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stdout).contains("inspection failed"));
    assert!(text(&output.stderr).contains("could not read PR merge times"));
    assert!(tree.exists());
}
