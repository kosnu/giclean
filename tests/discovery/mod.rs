use super::*;

fn rows(output: &Output) -> Vec<String> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}
fn row(kind: &str, path: &Path) -> String {
    format!("{kind}: {:?}", path.canonicalize().unwrap())
}
fn init(repo: &Repo, path: &Path) {
    checked(
        repo.git_command()
            .args(["init", "--initial-branch=main"])
            .arg(path),
    );
}

#[test]
fn lists_normal_bare_and_nested_without_parent_false_positives() {
    let repo = Repo::new();
    let nested = repo.work.join("nested");
    init(&repo, &nested);
    let output = checked(repo.cli().arg("repos").arg(&repo.root));
    let found = rows(&output);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.contains(&row("repository", &repo.work)));
    assert!(found.contains(&row("repository", &nested)));
    assert!(found.contains(&row("bare", &repo.origin)));
    let subdir = repo.work.join("ordinary");
    fs::create_dir(&subdir).unwrap();
    assert!(rows(&checked(repo.cli().arg("repos").arg(subdir))).is_empty());
    assert!(
        rows(&checked(
            repo.cli().arg("repos").arg(repo.work.join(".git"))
        ))
        .is_empty()
    );
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
}

#[test]
fn depth_excludes_defaults_and_overlapping_roots() {
    let repo = Repo::new();
    let nested = repo.work.join("group").join("nested");
    init(&repo, &nested);
    assert_eq!(
        rows(&checked(repo.cli().args(["repos", "--max-depth", "0"]))),
        vec![row("repository", &repo.work)]
    );
    let output = checked(
        repo.cli()
            .args(["repos", "--max-depth", "1", "--exclude", "origin.git"])
            .arg(&repo.root),
    );
    assert_eq!(rows(&output), vec![row("repository", &repo.work)]);
    // repo is first visited with no remaining depth, then as a root with one level.
    let output = checked(
        repo.cli()
            .args(["repos", "--max-depth", "2"])
            .arg(&repo.root)
            .arg(&repo.work)
            .arg(&repo.work),
    );
    assert_eq!(rows(&output).len(), 3);
    assert!(rows(&output).contains(&row("repository", &nested)));
    let output = checked(
        repo.cli()
            .args(["repos", "--exclude", "repo", "--exclude", "origin.git"])
            .arg(&repo.root),
    );
    assert!(rows(&output).is_empty());
}

#[test]
fn linked_worktrees_are_deduplicated_and_main_worktree_is_preferred() {
    let repo = Repo::new();
    repo.git(&["branch", "linked"]);
    repo.add_worktree("linked", "aaa-linked");
    let linked = repo.root.join("aaa-linked");
    let output = checked(repo.cli().arg("repos").arg(&linked));
    assert_eq!(rows(&output), vec![row("repository", &linked)]);
    let output = checked(
        repo.cli()
            .arg("repos")
            .arg(&linked)
            .arg(&repo.work)
            .arg(&repo.root),
    );
    assert_eq!(rows(&output).len(), 2);
    assert!(rows(&output).contains(&row("repository", &repo.work)));
}

#[test]
fn failures_do_not_hide_healthy_repositories() {
    let repo = Repo::new();
    let broken = repo.root.join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(
        broken.join(".git"),
        "gitdir: /missing-giclean-test-git-dir\n",
    )
    .unwrap();
    let output = repo
        .cli()
        .arg("repos")
        .arg(repo.root.join("missing"))
        .arg(&repo.root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(rows(&output).len(), 2);
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("missing"));
    assert!(error.contains("broken"));
}

#[cfg(unix)]
#[test]
fn symlink_cycles_are_not_followed_and_explicit_symlinks_fail() {
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    symlink(&repo.root, repo.root.join("cycle")).unwrap();
    symlink(&repo.work, repo.root.join("alias")).unwrap();
    let output = checked(
        repo.cli()
            .args(["repos", "--max-depth", "100"])
            .arg(&repo.root),
    );
    assert_eq!(rows(&output).len(), 2);
    let output = repo
        .cli()
        .arg("repos")
        .arg(repo.root.join("alias"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(rows(&output).is_empty());
}

#[test]
fn inherited_git_repository_selection_cannot_override_discovery() {
    let repo = Repo::new();
    let other = Repo::new();
    let output = checked(
        repo.cli()
            .arg("repos")
            .arg(&repo.root)
            .env("GIT_DIR", other.work.join(".git"))
            .env("GIT_COMMON_DIR", other.work.join(".git"))
            .env("GIT_WORK_TREE", &other.work),
    );
    let found = rows(&output);
    assert_eq!(found.len(), 2);
    assert!(found.contains(&row("repository", &repo.work)));
    assert!(found.contains(&row("bare", &repo.origin)));
}

#[cfg(unix)]
#[test]
fn unusual_path_bytes_are_preserved_and_output_is_escaped() {
    let repo = Repo::new();
    let strange = repo.root.join("line\nJapanese-日本語");
    init(&repo, &strange);
    let output = checked(repo.cli().arg("repos").arg(&strange));
    assert_eq!(rows(&output), vec![row("repository", &strange)]);
}

#[test]
fn repos_option_validation_and_dash_prefixed_roots() {
    let repo = Repo::new();
    for args in [
        vec!["--max-depth"],
        vec!["--max-depth", "-1"],
        vec!["--max-depth", "+1"],
        vec!["--max-depth", "1", "--max-depth", "2"],
        vec!["--exclude"],
        vec!["--exclude", "a/b"],
        vec!["--exclude", ".."],
        vec!["--wat"],
        vec!["--format", "json"],
    ] {
        let output = repo.cli().arg("repos").args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    let path = repo.work.join("--repo");
    init(&repo, &path);
    let output = checked(repo.cli().args(["repos", "--", "--repo"]));
    assert_eq!(rows(&output), vec![row("repository", &path)]);
    assert!(
        String::from_utf8_lossy(&checked(repo.cli().args(["repos", "--help"])).stdout)
            .contains("--max-depth")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_path_bytes_are_preserved() {
    use std::os::unix::ffi::OsStringExt;
    let repo = Repo::new();
    let strange = repo.root.join(OsString::from_vec(b"invalid-\xff".to_vec()));
    init(&repo, &strange);
    let output = checked(repo.cli().arg("repos").arg(&strange));
    assert_eq!(rows(&output), vec![row("repository", &strange)]);
}

#[cfg(unix)]
#[test]
fn explicit_symlink_roots_with_trailing_slashes_are_rejected() {
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    let alias = repo.root.join("alias");
    symlink(&repo.work, &alias).unwrap();
    for suffix in ["/", "///", "/.", "/./", "/././"] {
        let mut path = alias.clone().into_os_string();
        path.push(suffix);
        let output = repo.cli().arg("repos").arg(path).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(rows(&output).is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("symbolic links are not followed")
        );
        let mut regular = repo.work.clone().into_os_string();
        regular.push(suffix);
        let output = checked(repo.cli().arg("repos").arg(regular));
        assert_eq!(rows(&output), vec![row("repository", &repo.work)]);
    }
    // Do not collapse parent components through an ancestor symlink: the OS
    // resolves alias/.. to the real worktree's parent, not the link's parent.
    let container = repo.root.join("container");
    fs::create_dir(&container).unwrap();
    symlink(&repo.work, container.join("ancestor")).unwrap();
    let output = checked(
        repo.cli()
            .arg("repos")
            .arg(container.join("ancestor/../repo/")),
    );
    assert_eq!(rows(&output), vec![row("repository", &repo.work)]);
}

#[test]
fn separate_git_directory_is_skipped_and_main_worktree_wins() {
    let repo = Repo::new();
    let root = repo.root.join("separate");
    fs::create_dir(&root).unwrap();
    let main = root.join("main");
    let metadata = root.join("metadata");
    checked(
        repo.git_command()
            .args(["init", "--initial-branch=main", "--separate-git-dir"])
            .arg(&metadata)
            .arg(&main),
    );
    checked(repo.git_command().current_dir(&main).args([
        "-c",
        "commit.gpgSign=false",
        "commit",
        "--allow-empty",
        "-m",
        "initial",
    ]));
    let linked = root.join("aaa-linked");
    checked(
        repo.git_command()
            .current_dir(&main)
            .args(["worktree", "add", "-b", "linked"])
            .arg(&linked),
    );
    // If metadata is traversed this nested repository would leak into results.
    init(&repo, &metadata.join("must-not-descend"));
    assert!(rows(&checked(repo.cli().arg("repos").arg(&metadata))).is_empty());
    assert_eq!(
        rows(&checked(repo.cli().arg("repos").arg(&root))),
        vec![row("repository", &main)]
    );
    assert_eq!(
        rows(&checked(repo.cli().arg("repos").arg(&linked).arg(&main))),
        vec![row("repository", &main)]
    );
    assert_eq!(
        rows(&checked(repo.cli().arg("repos").arg(&linked))),
        vec![row("repository", &linked)]
    );
}
