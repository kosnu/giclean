use super::*;

fn fixture() -> (Repo, PathBuf) {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("ff"));
    repo.pr(&tip, "topic", &merged, old_time());
    fs::write(repo.work.join(".git/info/exclude"), "cache/\n").unwrap();
    fs::create_dir_all(tree.join("cache/generated")).unwrap();
    fs::write(tree.join("cache/generated/output"), "regenerable").unwrap();
    repo.age_tree(&tree);
    (repo, tree)
}

#[test]
fn explicit_ignored_paths_preview_then_delete_without_force() {
    let (repo, tree) = fixture();
    let output = checked(repo.wt_cli().args(["wt", "-d", "3"]));
    assert!(!text(&output.stdout).contains("deleted"));
    assert!(tree.exists());
    let args = ["wt", "-d", "3", "--allow-ignored", "cache/generated"];
    let output = checked(repo.wt_cli().args(args).arg("--dry-run"));
    assert!(text(&output.stdout).contains("candidate"));
    assert!(tree.exists());
    repo.git_wrapper("for arg do if [ \"$arg\" = --force ]; then exit 99; fi; done");
    let output = checked(repo.wt_cli().args(args));
    assert!(text(&output.stdout).contains("deleted"));
    assert!(!tree.exists());
    assert!(repo.exists("topic"));
}

#[test]
fn explicit_ignored_paths_preserve_other_safety_checks() {
    for case in [
        "sibling",
        "untracked",
        "dirty",
        "nested",
        "git-alias",
        "bare",
        "empty-sibling",
        "locked",
        "recent",
        "codex",
    ] {
        let (repo, tree) = fixture();
        match case {
            "sibling" => fs::write(tree.join("cache/personal"), "keep").unwrap(),
            "untracked" => fs::write(tree.join("personal"), "keep").unwrap(),
            "dirty" => fs::write(tree.join("topic"), "changed").unwrap(),
            "nested" => {
                fs::create_dir(tree.join("cache/generated/.git")).unwrap();
            }
            "git-alias" => {
                fs::create_dir(tree.join("cache/generated/.GiT")).unwrap();
            }
            "bare" => {
                fs::write(tree.join("cache/generated/HEAD"), "ref: refs/heads/main").unwrap();
                fs::create_dir(tree.join("cache/generated/objects")).unwrap();
                fs::create_dir(tree.join("cache/generated/refs")).unwrap();
            }
            "empty-sibling" => {
                fs::create_dir(tree.join("cache/personal")).unwrap();
            }
            "locked" => {
                repo.git(&["worktree", "lock", tree.to_str().unwrap()]);
            }
            "recent" => {}
            "codex" => {
                fs::create_dir(repo.root.join("codex-present")).unwrap();
            }
            _ => unreachable!(),
        }
        repo.age_tree(&tree);
        if case == "recent" {
            set_time(&tree.join("cache/generated/output"), SystemTime::now());
        }
        let mut command = repo.wt_cli();
        if case == "codex" {
            command.env("CODEX_HOME", repo.root.join("codex-present"));
        }
        let output = checked(command.args(["wt", "-h", "1", "--allow-ignored", "cache/generated"]));
        assert!(!text(&output.stdout).contains("deleted"), "{case}");
        assert!(tree.exists(), "{case}");
    }
}

#[test]
fn explicit_ignored_paths_recheck_new_unapproved_files() {
    let (repo, tree) = fixture();
    fs::write(
        repo.root.join("gh-action"),
        format!(
            "printf keep > {}/cache/personal\ntouch -t 202001010000 {}/cache/personal {}/cache",
            quote(&tree),
            quote(&tree),
            quote(&tree)
        ),
    )
    .unwrap();
    let output =
        checked(
            repo.wt_cli()
                .args(["wt", "-d", "3", "--allow-ignored", "cache/generated"]),
        );
    assert!(!text(&output.stdout).contains("deleted"));
    assert!(tree.join("cache/personal").exists());
}

#[test]
fn explicit_ignored_paths_reject_unsafe_arguments() {
    let repo = Repo::wt();
    for value in [
        "/tmp",
        ".",
        "..",
        "cache/../x",
        "./cache",
        ".git",
        "cache/.git",
    ] {
        assert!(
            !repo
                .wt_cli()
                .args(["wt", "-d", "3", "--allow-ignored", value])
                .output()
                .unwrap()
                .status
                .success(),
            "{value}"
        );
    }
}
