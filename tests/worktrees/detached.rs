use super::*;

fn detach(repo: &Repo, tree: &Path) {
    checked(
        repo.git_command()
            .current_dir(tree)
            .args(["switch", "--detach"]),
    );
    repo.age_tree(tree);
}

#[test]
fn detached_opt_in_requires_exact_merge_proof_and_preserves_refs() {
    for mode in ["merge", "squash", "ff"] {
        for at_merge in [false, true] {
            let repo = Repo::wt();
            let (tip, merged, tree) = repo.topic("topic", Some(mode));
            detach(&repo, &tree);
            let head = if at_merge { &merged } else { &tip };
            checked(
                repo.git_command()
                    .current_dir(&tree)
                    .args(["checkout", "--detach", head]),
            );
            repo.age_tree(&tree);
            // Commit association may identify the merge commit rather than PR head.
            repo.pr(&tip, "topic", &merged, old_time());
            if at_merge && tip != merged {
                fs::copy(
                    repo.root.join("pr").join(&tip),
                    repo.root.join("pr").join(&merged),
                )
                .unwrap();
            }
            let refs = repo.git(&["show-ref"]);
            assert!(
                checked(repo.wt_cli().args(["wt", "-d", "3"]))
                    .stdout
                    .is_empty()
            );
            let preview =
                checked(
                    repo.wt_cli()
                        .args(["wt", "-d", "3", "--include-detached", "--dry-run"]),
                );
            assert!(
                text(&preview.stdout).contains("candidate:"),
                "{mode}/{at_merge}"
            );
            assert!(tree.exists());
            // The caller's chosen cutoff, not a fixed five-day threshold.
            assert!(
                checked(repo.wt_cli().args(["wt", "-d", "11", "--include-detached"]))
                    .stdout
                    .is_empty()
            );
            checked(repo.wt_cli().args(["wt", "-h", "12", "--include-detached"]));
            assert!(!tree.exists());
            assert_eq!(repo.git(&["show-ref"]), refs);
            assert!(repo.work.exists());
        }
    }
}

#[test]
fn detached_opt_in_keeps_uncertain_and_protected_worktrees() {
    for protection in [
        "no-pr",
        "unmerged",
        "different-content",
        "wrong-head",
        "wrong-base",
        "wrong-repo",
        "recent-merge",
        "unreachable-merge",
        "locked",
        "current",
        "dirty",
        "untracked",
        "ignored",
        "recent",
        "codex",
    ] {
        let repo = Repo::wt();
        let mode = if matches!(protection, "unmerged" | "different-content") {
            None
        } else {
            Some("merge")
        };
        let (tip, mut merged, tree) = repo.topic("topic", mode);
        if protection == "different-content" {
            fs::write(repo.work.join("topic"), "different\n").unwrap();
            repo.git(&["add", "topic"]);
            repo.git(&["commit", "-m", "similar but not identical"]);
            repo.git(&["push", "origin", "main"]);
            merged = repo.git(&["rev-parse", "HEAD"]);
        }
        detach(&repo, &tree);
        if protection != "no-pr" {
            repo.pr(&tip, "topic", &merged, old_time());
        }
        if matches!(
            protection,
            "wrong-base" | "wrong-repo" | "recent-merge" | "unreachable-merge"
        ) {
            let file = repo.root.join("pr").join(&tip);
            let data = fs::read_to_string(&file).unwrap();
            let data = match protection {
                "wrong-base" => data.replace("\tmain\t", "\tother\t"),
                "wrong-repo" => data.replace("test/repo", "other/repo"),
                "recent-merge" => format!(
                    "{}\t{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    data.split_once('\t').unwrap().1
                ),
                "unreachable-merge" => {
                    repo.git(&["push", "--force", "origin", &format!("{merged}~1:main")]);
                    data
                }
                _ => unreachable!(),
            };
            fs::write(file, data).unwrap();
        }
        match protection {
            "wrong-head" => {
                repo.pr(&tip, "topic", &merged, old_time());
                let file = repo.root.join("pr").join(&tip);
                let data = fs::read_to_string(&file)
                    .unwrap()
                    .replacen(&tip, &"a".repeat(40), 1);
                fs::write(file, data).unwrap();
            }
            "locked" => {
                repo.git(&["worktree", "lock", tree.to_str().unwrap()]);
            }
            "dirty" => fs::write(tree.join("topic"), "local edit").unwrap(),
            "untracked" => fs::write(tree.join("local"), "local data").unwrap(),
            "ignored" => {
                fs::write(repo.work.join(".git/info/exclude"), "ignored\n").unwrap();
                fs::write(tree.join("ignored"), "local data").unwrap();
            }
            "codex" => {
                fs::create_dir(repo.root.join("codex-home")).unwrap();
            }
            _ => {}
        }
        repo.age_tree(&tree);
        if protection == "recent" {
            set_time(&tree, SystemTime::now());
        }
        let mut command = repo.wt_cli();
        if protection == "current" {
            command.current_dir(&tree);
        }
        if protection == "codex" {
            command.env("CODEX_HOME", repo.root.join("codex-home"));
        }
        let output = checked(command.args(["wt", "-d", "3", "--include-detached"]));
        assert!(
            output.stdout.is_empty(),
            "{protection}: {}",
            text(&output.stdout)
        );
        assert!(tree.exists(), "{protection}");
    }
}

#[test]
fn detached_rechecks_a_lock_added_during_merge_lookup() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    detach(&repo, &tree);
    repo.pr(&tip, "topic", &merged, old_time());
    fs::write(
        repo.root.join("gh-action"),
        format!(
            "git -C {} worktree lock {}\n",
            quote(&repo.work),
            quote(&tree)
        ),
    )
    .unwrap();
    let output = checked(repo.wt_cli().args(["wt", "-d", "3", "--include-detached"]));
    assert!(output.stdout.is_empty());
    assert!(tree.exists());
}

#[test]
fn batch_accepts_detached_opt_in_without_deleting_in_preview() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    detach(&repo, &tree);
    repo.pr(&tip, "topic", &merged, old_time());
    let output = checked(
        repo.wt_cli()
            .args(["batch", "wt", "-d", "3", "--include-detached", "--dry-run"])
            .arg(&repo.work),
    );
    assert!(text(&output.stdout).contains("candidate:"));
    assert!(tree.exists());
}
