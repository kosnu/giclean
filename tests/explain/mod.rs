use super::*;

#[test]
fn explain_br_covers_candidates_and_protected_and_unmerged_branches() {
    let repo = Repo::new();
    repo.git(&["branch", "merged"]);
    repo.git(&["branch", "master"]);
    repo.git(&["branch", "used"]);
    repo.add_worktree("used", "used-tree");
    repo.git(&["switch", "-c", "unmerged"]);
    repo.git(&["commit", "--allow-empty", "-m", "not included"]);
    repo.git(&["switch", "main"]);
    let before = repo.git(&["show-ref"]);
    let output = checked(repo.cli().args(["br", "--explain", "--dry-run"]));
    let output = text(&output.stdout);
    assert!(output.contains("candidate: merged"), "{output}");
    for fragment in [
        "branch \"merged\": eligible (tip is an ancestor",
        "branch \"master\": kept (protected branch name)",
        "branch \"used\": kept (branch is checked out",
        "branch \"unmerged\": kept (neither ancestry",
    ] {
        assert!(output.contains(fragment), "missing {fragment}: {output}");
    }
    assert_eq!(repo.git(&["show-ref"]), before);
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    let output = checked(repo.cli().args(["br", "--explain"]));
    assert!(text(&output.stdout).contains("merged: deleted"));
    assert!(!repo.exists("merged"));
    assert!(repo.exists("master"));
    assert!(repo.exists("used"));
    assert!(repo.exists("unmerged"));
}

#[test]
fn explain_protected_only_needs_no_origin() {
    let repo = Repo::new();
    repo.git(&["remote", "remove", "origin"]);
    let output = checked(repo.cli().args(["br", "--dry-run", "--explain"]));
    assert!(text(&output.stdout).contains("explain: branch \"main\": kept"));
}

#[test]
fn explain_rejects_duplicate_or_unrelated_flags_without_git() {
    let repo = Repo::new();
    for args in [
        vec!["--explain"],
        vec!["br", "--explain", "--explain"],
        vec!["br", "--explain", "--unknown"],
        vec!["wt", "-d", "3", "--explain", "--explain"],
    ] {
        let output = repo.cli().env("PATH", "").args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn explain_br_identifies_exact_squash_inclusion() {
    let repo = Repo::new();
    repo.git(&["switch", "-c", "squash"]);
    fs::write(repo.work.join("feature"), "feature\n").unwrap();
    repo.git(&["add", "feature"]);
    repo.git(&["commit", "-m", "feature"]);
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--squash", "squash"]);
    repo.git(&["commit", "-m", "squashed"]);
    repo.git(&["push", "origin", "main"]);
    let output = checked(repo.cli().args(["br", "--dry-run", "--explain"]));
    assert!(text(&output.stdout).contains("all changed paths match at one point"));
    assert!(repo.exists("squash"));
}
