use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod codex;
mod ignored;

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn old_time() -> SystemTime {
    SystemTime::now() - Duration::from_secs(10 * 86400)
}

fn set_time(path: &Path, time: SystemTime) {
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}

fn age_directory(path: &Path) {
    for child in fs::read_dir(path).unwrap() {
        let child = child.unwrap().path();
        if child.is_dir() {
            age_directory(&child);
        }
        set_time(&child, old_time());
    }
    set_time(path, old_time());
}

impl Repo {
    fn wt() -> Self {
        let mut repo = Self::new();
        repo.root = fs::canonicalize(&repo.root).unwrap();
        repo.work = fs::canonicalize(&repo.work).unwrap();
        repo.origin = fs::canonicalize(&repo.origin).unwrap();
        // All Git transport remains local; gh is a deterministic API boundary.
        repo.git(&[
            "config",
            &format!("url.{}.insteadOf", repo.origin.display()),
            "https://github.com/test/repo.git",
        ]);
        repo.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/test/repo.git",
        ]);
        fs::create_dir(repo.root.join("bin")).unwrap();
        fs::create_dir(repo.root.join("pr")).unwrap();
        executable(
            &repo.root.join("bin/gh"),
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$TEST_GH_ROOT/gh-calls\"\nif [ -f \"$TEST_GH_ROOT/gh-action\" ]; then /bin/sh \"$TEST_GH_ROOT/gh-action\"; fi\nif [ -f \"$TEST_GH_ROOT/gh-fail\" ]; then exit 1; fi\nfor arg do\n case \"$arg\" in repos/test/repo/commits/*/pulls\\?per_page=100) oid=${arg#repos/test/repo/commits/}; oid=${oid%/pulls*};; esac\ndone\nif [ -f \"$TEST_GH_ROOT/pr/$oid\" ]; then /bin/cat \"$TEST_GH_ROOT/pr/$oid\"; fi\n",
        );
        repo
    }

    fn wt_cli(&self) -> Command {
        let mut command = self.cli();
        let mut paths = vec![self.root.join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("TEST_GH_ROOT", &self.root);
        command
    }

    fn topic(&self, branch: &str, merge: Option<&str>) -> (String, String, PathBuf) {
        self.git(&["switch", "-c", branch]);
        fs::write(self.work.join(branch), format!("{branch}\n")).unwrap();
        self.git(&["add", "--", branch]);
        let date = format!(
            "@{} +0000",
            old_time().duration_since(UNIX_EPOCH).unwrap().as_secs()
        );
        checked(
            self.git_command()
                .args(["commit", "-m", branch])
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date),
        );
        let tip = self.git(&["rev-parse", "HEAD"]);
        self.git(&["switch", "main"]);
        match merge {
            Some("squash") => {
                self.git(&["merge", "--squash", branch]);
                self.git(&["commit", "-m", "squash merge"]);
            }
            Some("merge") => {
                self.git(&["merge", "--no-ff", "-m", "merge", branch]);
            }
            Some("ff") => {
                self.git(&["merge", "--ff-only", branch]);
            }
            None => {}
            _ => unreachable!(),
        }
        let merged = self.git(&["rev-parse", "HEAD"]);
        self.git(&["push", "origin", "main"]);
        self.add_worktree(branch, &format!("tree {branch}"));
        let tree = self.root.join(format!("tree {branch}"));
        self.age_tree(&tree);
        (tip, merged, tree)
    }

    fn age_tree(&self, tree: &Path) {
        let git_dir = text(
            &checked(
                self.git_command()
                    .current_dir(tree)
                    .args(["rev-parse", "--absolute-git-dir"]),
            )
            .stdout,
        )
        .to_owned();
        age_directory(tree);
        age_directory(Path::new(&git_dir));
    }

    fn pr(&self, tip: &str, branch: &str, merged: &str, time: SystemTime) {
        fs::write(
            self.root.join("pr").join(tip),
            format!(
                "{}\t{tip}\t{branch}\tmain\ttest/repo\t{merged}\n",
                time.duration_since(UNIX_EPOCH).unwrap().as_secs()
            ),
        )
        .unwrap();
    }

    fn git_wrapper(&self, body: &str) {
        let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .unwrap();
        executable(
            &self.root.join("bin/git"),
            &format!("#!/bin/sh\nset -eu\n{body}\nexec {} \"$@\"\n", quote(&real)),
        );
    }
}

#[test]
fn wt_deletes_aged_merge_squash_and_fast_forward_worktrees_but_keeps_branches() {
    for mode in ["merge", "squash", "ff"] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("topic", Some(mode));
        repo.pr(&tip, "topic", &merged, old_time());
        let before = repo.git(&["show-ref"]);
        let output = checked(repo.wt_cli().args(["wt", "-d", "3", "--dry-run"]));
        assert_eq!(
            text(&output.stdout),
            format!("candidate: {}", tree.display()),
            "{mode}"
        );
        assert!(tree.exists());
        assert_eq!(repo.git(&["show-ref"]), before);
        let output = checked(repo.wt_cli().args(["wt", "-h", "12"]));
        assert_eq!(
            text(&output.stdout),
            format!("{}: deleted", tree.display()),
            "{mode}"
        );
        assert!(!tree.exists());
        assert!(repo.exists("topic"));
        assert!(!repo.work.join(".git/FETCH_HEAD").exists());
        assert!(
            !repo
                .git(&["worktree", "list", "--porcelain"])
                .contains(tree.to_str().unwrap())
        );
    }
}

#[test]
fn wt_uses_pr_merge_time_instead_of_old_commit_time() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("ff"));
    // A fast-forward can merge an old commit much later; the PR date governs.
    repo.pr(&tip, "topic", &merged, SystemTime::now());
    let output = checked(repo.wt_cli().args(["wt", "--days", "3"]));
    assert!(output.stdout.is_empty());
    assert!(tree.exists());
    let hours_ago = SystemTime::now() - Duration::from_secs(18 * 3600);
    repo.pr(&tip, "topic", &merged, hours_ago);
    assert!(
        checked(repo.wt_cli().args(["wt", "-d", "1"]))
            .stdout
            .is_empty()
    );
    assert!(
        !checked(repo.wt_cli().args(["wt", "--hours", "12", "--dry-run"]))
            .stdout
            .is_empty()
    );
}

#[test]
fn wt_later_activity_restarts_the_wait_even_when_worktree_is_clean() {
    for activity in ["file", "directory", "index", "reflog", "checkout", "future"] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("topic", Some("merge"));
        repo.pr(&tip, "topic", &merged, old_time());
        let git_dir = PathBuf::from(text(
            &checked(
                repo.git_command()
                    .current_dir(&tree)
                    .args(["rev-parse", "--absolute-git-dir"]),
            )
            .stdout,
        ));
        match activity {
            "file" => set_time(&tree.join("topic"), SystemTime::now()),
            "directory" => set_time(&tree, SystemTime::now()),
            "index" => set_time(&git_dir.join("index"), SystemTime::now()),
            "reflog" => set_time(&git_dir.join("logs/HEAD"), SystemTime::now()),
            "checkout" => {
                checked(
                    repo.git_command()
                        .current_dir(&tree)
                        .args(["checkout", "topic"]),
                );
            }
            "future" => set_time(
                &tree.join("topic"),
                SystemTime::now() + Duration::from_secs(86400),
            ),
            _ => unreachable!(),
        }
        let output = checked(repo.wt_cli().args(["wt", "-h", "12"]));
        assert!(
            output.stdout.is_empty(),
            "{activity}: {}",
            text(&output.stdout)
        );
        assert!(tree.exists(), "{activity}");
    }
}

#[test]
fn wt_protects_all_local_changes_and_in_progress_operations_even_if_old() {
    for change in [
        "modified",
        "staged",
        "untracked",
        "ignored",
        "merge",
        "rebase",
        "lock",
    ] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("topic", Some("squash"));
        repo.pr(&tip, "topic", &merged, old_time());
        let git_dir = PathBuf::from(text(
            &checked(
                repo.git_command()
                    .current_dir(&tree)
                    .args(["rev-parse", "--absolute-git-dir"]),
            )
            .stdout,
        ));
        match change {
            "modified" | "staged" => {
                fs::write(tree.join("topic"), "local change").unwrap();
                if change == "staged" {
                    checked(repo.git_command().current_dir(&tree).args(["add", "topic"]));
                }
            }
            "untracked" => fs::write(tree.join("notes"), "keep").unwrap(),
            "ignored" => {
                fs::write(repo.work.join(".git/info/exclude"), "ignored-file\n").unwrap();
                fs::write(tree.join("ignored-file"), "keep").unwrap();
            }
            "merge" => fs::write(git_dir.join("MERGE_HEAD"), &tip).unwrap(),
            "rebase" => fs::create_dir(git_dir.join("rebase-merge")).unwrap(),
            "lock" => fs::write(git_dir.join("index.lock"), "").unwrap(),
            _ => unreachable!(),
        }
        repo.age_tree(&tree);
        let output = checked(repo.wt_cli().args(["wt", "-d", "3"]));
        assert!(output.stdout.is_empty(), "{change}");
        assert!(tree.exists(), "{change}");
    }
}

#[test]
fn wt_preserves_worktrees_with_hidden_changes_without_modifying_index_flags() {
    for flags in [
        &["--assume-unchanged"][..],
        &["--skip-worktree"][..],
        &["--assume-unchanged", "--skip-worktree"][..],
    ] {
        for changed in [false, true] {
            let repo = Repo::wt();
            let (tip, merged, tree) = repo.topic("topic", Some("merge"));
            repo.pr(&tip, "topic", &merged, old_time());
            for flag in flags {
                checked(repo.git_command().current_dir(&tree).args([
                    "update-index",
                    flag,
                    "--",
                    "topic",
                ]));
            }
            let content = if changed {
                "hidden local data\n"
            } else {
                "topic\n"
            };
            fs::write(tree.join("topic"), content).unwrap();
            assert!(
                checked(repo.git_command().current_dir(&tree).args([
                    "status",
                    "--porcelain=v1",
                    "-z",
                ]))
                .stdout
                .is_empty()
            );
            repo.age_tree(&tree);
            let git_dir = PathBuf::from(text(
                &checked(
                    repo.git_command()
                        .current_dir(&tree)
                        .args(["rev-parse", "--absolute-git-dir"]),
                )
                .stdout,
            ));
            let before = fs::read(git_dir.join("index")).unwrap();
            let before_time = fs::metadata(git_dir.join("index"))
                .unwrap()
                .modified()
                .unwrap();
            for args in [&["wt", "-d", "3", "--dry-run"][..], &["wt", "-d", "3"][..]] {
                let output = checked(repo.wt_cli().args(args));
                assert!(output.stdout.is_empty(), "{flags:?}, changed={changed}");
                assert_eq!(fs::read_to_string(tree.join("topic")).unwrap(), content);
                assert_eq!(fs::read(git_dir.join("index")).unwrap(), before);
                assert_eq!(
                    fs::metadata(git_dir.join("index"))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    before_time
                );
            }
            assert!(!repo.root.join("gh-calls").exists());
        }
    }
}

#[test]
fn wt_rechecks_index_flags_even_when_hidden_changes_and_index_are_old() {
    for flags in [
        "--assume-unchanged",
        "--skip-worktree",
        "--assume-unchanged --skip-worktree",
    ] {
        for dry_run in [true, false] {
            let repo = Repo::wt();
            let (tip, merged, tree) = repo.topic("a", Some("merge"));
            repo.pr(&tip, "a", &merged, old_time());
            let (other, other_merged, other_tree) = repo.topic("z", Some("merge"));
            repo.pr(&other, "z", &other_merged, old_time());
            let git_dir = PathBuf::from(text(
                &checked(
                    repo.git_command()
                        .current_dir(&tree)
                        .args(["rev-parse", "--absolute-git-dir"]),
                )
                .stdout,
            ));
            let reference = repo.root.join("old-time");
            fs::write(&reference, "").unwrap();
            set_time(&reference, old_time());
            // The later PR lookup runs after a was admitted as a candidate.
            // Restore all modified mtimes so only the flag check can protect a.
            let action = format!(
                "git -C {} update-index {flags} -- a\nprintf 'hidden local data\\n' > {}\ntouch -r {} {} {} {}",
                quote(&tree),
                quote(&tree.join("a")),
                quote(&reference),
                quote(&tree.join("a")),
                quote(&git_dir.join("index")),
                quote(&git_dir),
            );
            fs::write(repo.root.join("gh-action"), format!(
                "set -eu\ncase $(/bin/cat \"$TEST_GH_ROOT/gh-calls\") in *{other}*) {action};; esac\n",
            )).unwrap();
            let mut command = repo.wt_cli();
            command.args(["wt", "-d", "3"]);
            if dry_run {
                command.arg("--dry-run");
            }
            let output = checked(&mut command);
            assert!(
                !text(&output.stdout).contains(tree.to_str().unwrap()),
                "{flags}, dry_run={dry_run}"
            );
            assert_eq!(
                fs::read_to_string(tree.join("a")).unwrap(),
                "hidden local data\n"
            );
            assert!(
                fs::metadata(git_dir.join("index"))
                    .unwrap()
                    .modified()
                    .unwrap()
                    < SystemTime::now() - Duration::from_secs(3 * 86400)
            );
            assert_eq!(other_tree.exists(), dry_run);
        }
    }
}

#[test]
fn wt_index_flag_query_errors_and_malformed_records_stop_deletion() {
    for response in [
        None,
        Some(&b"Q topic\0"[..]),
        Some(&b"H topic"[..]),
        Some(&b"H \0"[..]),
        Some(&b"h topic\0Q other\0"[..]),
    ] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("topic", Some("merge"));
        repo.pr(&tip, "topic", &merged, old_time());
        let response_file = repo.root.join("index-response");
        if let Some(response) = response {
            fs::write(&response_file, response).unwrap();
        }
        let action = if response.is_some() {
            format!("/bin/cat {}; exit 0", quote(&response_file))
        } else {
            "echo 'test index query failure' >&2; exit 1".into()
        };
        repo.git_wrapper(&format!("case \"$*\" in *' ls-files '*) {action};; esac"));
        for args in [&["wt", "-d", "3", "--dry-run"][..], &["wt", "-d", "3"][..]] {
            let output = repo.wt_cli().args(args).output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{response:?}");
            assert!(output.stdout.is_empty());
            assert!(text(&output.stderr).contains("index flags"));
            assert!(tree.join("topic").exists());
        }
    }
}

#[test]
fn wt_index_flag_inspection_handles_literal_filenames() {
    use std::os::unix::ffi::OsStringExt;

    let repo = Repo::wt();
    let mut name = b"h name with spaces\nS another".to_vec();
    // macOS filesystems reject non-UTF-8 filenames; Linux can exercise those too.
    if cfg!(target_os = "linux") {
        name.push(0xff);
    }
    let name = OsString::from_vec(name);
    fs::write(repo.work.join(name), "tracked\n").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "-m", "literal filename"]);
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    assert!(
        !checked(repo.wt_cli().args(["wt", "-d", "3", "--dry-run"]))
            .stdout
            .is_empty()
    );
    checked(repo.wt_cli().args(["wt", "-d", "3"]));
    assert!(!tree.exists());
}

#[test]
fn wt_keeps_locked_current_detached_primary_and_protected_branch_worktrees() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    repo.git(&["worktree", "lock", tree.to_str().unwrap()]);
    assert!(
        checked(repo.wt_cli().args(["wt", "-d", "3"]))
            .stdout
            .is_empty()
    );
    repo.git(&["worktree", "unlock", tree.to_str().unwrap()]);
    repo.age_tree(&tree);
    fs::create_dir(tree.join("subdirectory")).unwrap();
    repo.age_tree(&tree);
    assert!(
        checked(
            repo.wt_cli()
                .current_dir(tree.join("subdirectory"))
                .args(["wt", "-h", "12"])
        )
        .stdout
        .is_empty()
    );
    repo.git(&["worktree", "lock", tree.to_str().unwrap()]);
    for branch in ["master", "production"] {
        repo.git(&["branch", branch, &tip]);
        repo.add_worktree(branch, branch);
        repo.pr(&tip, branch, &merged, old_time());
        repo.age_tree(&repo.root.join(branch));
    }
    checked(
        repo.git_command()
            .args(["worktree", "add", "--detach"])
            .arg(repo.root.join("detached"))
            .arg(&tip),
    );
    repo.age_tree(&repo.root.join("detached"));
    fs::write(repo.root.join("gh-fail"), "").unwrap();
    let output = checked(repo.wt_cli().args(["wt", "-d", "3"]));
    assert!(output.stdout.is_empty());
    assert!(tree.exists());
    assert!(repo.work.exists());
}

#[test]
fn wt_pr_metadata_alone_never_proves_inclusion() {
    for reason in [
        "unmerged",
        "extra-commit",
        "wrong-head",
        "wrong-branch",
        "wrong-base",
        "wrong-repo",
        "unknown-time",
        "unreachable-merge",
    ] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic(
            "topic",
            if reason == "unmerged" {
                None
            } else {
                Some("squash")
            },
        );
        repo.pr(&tip, "topic", &merged, old_time());
        let pr = repo.root.join("pr").join(&tip);
        match reason {
            "unmerged" => {} // Lie about an unrelated commit; Git must reject it.
            "extra-commit" => {
                fs::write(tree.join("topic"), "unmerged change").unwrap();
                checked(repo.git_command().current_dir(&tree).args(["add", "topic"]));
                checked(repo.git_command().current_dir(&tree).args([
                    "commit",
                    "--allow-empty",
                    "-m",
                    "later",
                ]));
                let later = text(
                    &checked(
                        repo.git_command()
                            .current_dir(&tree)
                            .args(["rev-parse", "HEAD"]),
                    )
                    .stdout,
                )
                .to_owned();
                repo.pr(&later, "topic", &merged, old_time());
                repo.age_tree(&tree);
            }
            "wrong-head" => {
                fs::write(
                    &pr,
                    fs::read_to_string(&pr).unwrap().replacen(&tip, &merged, 1),
                )
                .unwrap();
            }
            "wrong-branch" => {
                fs::write(
                    &pr,
                    fs::read_to_string(&pr)
                        .unwrap()
                        .replace("\ttopic\t", "\tother\t"),
                )
                .unwrap();
            }
            "wrong-base" => {
                fs::write(
                    &pr,
                    fs::read_to_string(&pr)
                        .unwrap()
                        .replace("\tmain\t", "\tother\t"),
                )
                .unwrap();
            }
            "wrong-repo" => {
                fs::write(
                    &pr,
                    fs::read_to_string(&pr)
                        .unwrap()
                        .replace("test/repo", "test/other"),
                )
                .unwrap();
            }
            "unknown-time" => {
                fs::write(&pr, "").unwrap();
            }
            "unreachable-merge" => repo.pr(&tip, "topic", &tip, old_time()),
            _ => unreachable!(),
        }
        let output = checked(repo.wt_cli().args(["wt", "-d", "3"]));
        assert!(
            output.stdout.is_empty(),
            "{reason}: {}",
            text(&output.stdout)
        );
        assert!(tree.exists(), "{reason}");
    }
}

#[test]
fn wt_query_failure_or_malformed_response_stops_before_any_deletion() {
    for response in ["failure", "invalid\n", "0\tbad\tresponse\n"] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("a", Some("merge"));
        repo.pr(&tip, "a", &merged, old_time());
        let (other, other_merged, other_tree) = repo.topic("z", Some("merge"));
        repo.pr(&other, "z", &other_merged, old_time());
        // The first candidate is valid; failure of a later lookup must retain it.
        fs::write(repo.root.join("pr").join(&other), response).unwrap();
        if response == "failure" {
            fs::write(repo.root.join("gh-fail"), "").unwrap();
        }
        let output = repo.wt_cli().args(["wt", "-d", "3"]).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(tree.exists());
        assert!(other_tree.exists());
    }
}

#[test]
fn wt_rechecks_activity_head_locks_and_local_files_after_remote_queries() {
    for change in ["activity", "head", "locked", "ignored"] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("a", Some("merge"));
        repo.pr(&tip, "a", &merged, old_time());
        let (other, other_merged, _) = repo.topic("z", Some("merge"));
        repo.pr(&other, "z", &other_merged, old_time());
        let action = match change {
            "activity" => format!("touch {}/a", quote(&tree)),
            "head" => format!(
                "git -C {} commit --allow-empty -m later >/dev/null",
                quote(&tree)
            ),
            "locked" => format!("git worktree lock {}", quote(&tree)),
            "ignored" => {
                fs::write(repo.work.join(".git/info/exclude"), "ignored-file\n").unwrap();
                format!("printf keep > {}/ignored-file", quote(&tree))
            }
            _ => unreachable!(),
        };
        fs::write(
            repo.root.join("gh-action"),
            format!("case $(/bin/cat \"$TEST_GH_ROOT/gh-calls\") in *{other}*) {action};; esac\n"),
        )
        .unwrap();
        let output = checked(repo.wt_cli().args(["wt", "-h", "12"]));
        assert!(tree.exists(), "{change}: {}", text(&output.stdout));
        assert!(!text(&output.stdout).contains(&format!("{}: deleted", tree.display())));
    }
}

#[test]
fn wt_reports_partial_removal_failure_and_continues() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("a", Some("merge"));
    repo.pr(&tip, "a", &merged, old_time());
    let (other, other_merged, other_tree) = repo.topic("z", Some("merge"));
    repo.pr(&other, "z", &other_merged, old_time());
    repo.git_wrapper("case \"$*\" in *'worktree remove -- '*'tree a') echo 'test removal failure' >&2; exit 1;; esac");
    let output = repo.wt_cli().args(["wt", "-d", "3"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stdout).contains("test removal failure"));
    assert!(text(&output.stdout).contains(&format!("{}: deleted", other_tree.display())));
    assert!(tree.exists());
    assert!(!other_tree.exists());
    assert!(repo.exists("a") && repo.exists("z"));
}

#[test]
fn wt_help_and_invalid_arguments_do_not_require_git_or_gh() {
    let repo = Repo::new();
    let output = checked(repo.cli().env("PATH", "").args(["wt", "--help"]));
    assert!(text(&output.stdout).contains("-h means hours"));
    for args in [
        vec!["wt"],
        vec!["wt", "-d"],
        vec!["wt", "-h"],
        vec!["wt", "-d", "0"],
        vec!["wt", "-h", "-1"],
        vec!["wt", "-h", "1.5"],
        vec!["wt", "-h", "+1"],
        vec!["wt", "-d", "18446744073709551615"],
        vec!["wt", "-d", "1", "-h", "12"],
        vec!["wt", "-d", "1", "--dry-run", "--dry-run"],
        vec!["wt", "--help", "-h", "12"],
    ] {
        let output = repo.cli().env("PATH", "").args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn wt_without_linked_worktrees_needs_no_origin_even_on_an_unborn_branch() {
    let repo = Repo::new();
    repo.git(&["remote", "remove", "origin"]);
    assert!(
        checked(repo.cli().args(["wt", "-d", "3"]))
            .stdout
            .is_empty()
    );
    repo.git(&["switch", "--orphan", "unborn"]);
    assert!(
        checked(repo.cli().args(["wt", "-h", "12"]))
            .stdout
            .is_empty()
    );
}

#[test]
fn wt_uses_the_latest_matching_pr_time_and_ignores_unrelated_newer_prs() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    let old = fs::read_to_string(repo.root.join("pr").join(&tip)).unwrap();
    repo.pr(&tip, "topic", &merged, SystemTime::now());
    let recent = fs::read_to_string(repo.root.join("pr").join(&tip)).unwrap();
    for data in [format!("{old}{recent}"), format!("{recent}{old}")] {
        fs::write(repo.root.join("pr").join(&tip), data).unwrap();
        assert!(
            checked(repo.wt_cli().args(["wt", "-d", "3"]))
                .stdout
                .is_empty()
        );
        assert!(tree.exists());
    }
    fs::write(
        repo.root.join("pr").join(&tip),
        format!("{old}{}", recent.replace("\tmain\t", "\tother\t")),
    )
    .unwrap();
    assert!(
        !checked(repo.wt_cli().args(["wt", "-d", "3", "--dry-run"]))
            .stdout
            .is_empty()
    );
}

#[test]
fn wt_missing_latest_remote_commit_fails_without_fetch_or_deletion() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    // Create a commit only in the bare origin, leaving this clone behind.
    let tree_oid = repo.git(&["rev-parse", "main^{tree}"]);
    let bare = format!("--git-dir={}", repo.origin.display());
    let new = text(
        &checked(repo.git_command().args([
            &bare,
            "commit-tree",
            &tree_oid,
            "-p",
            &merged,
            "-m",
            "remote only",
        ]))
        .stdout,
    )
    .to_owned();
    repo.git(&[&bare, "update-ref", "refs/heads/main", &new]);
    let output = repo.wt_cli().args(["wt", "-d", "3"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("git fetch origin"));
    assert!(output.stdout.is_empty());
    assert!(tree.exists());
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
    assert!(!repo.root.join("gh-calls").exists());
}

mod explain;

#[test]
fn json_wt_escapes_paths_and_reports_preview_and_deletion() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    let unusual = repo.root.join("日本語 \"quote\" \\ slash\n\tpath");
    checked(
        repo.git_command()
            .args(["worktree", "move"])
            .arg(&tree)
            .arg(&unusual),
    );
    repo.age_tree(&unusual);
    for dry_run in [true, false] {
        let mut command = repo.wt_cli();
        command.args(["wt", "-d", "3", "--format", "json"]);
        if dry_run {
            command.arg("--dry-run");
        }
        let output = checked(&mut command);
        let document = json_support::parse(&output.stdout);
        assert_eq!(document.get("command").string(), "wt");
        assert_eq!(document.get("success"), &json_support::Json::Bool(true));
        let result = document
            .get("executions")
            .array()
            .iter()
            .find(|r| r.get("target").get("path").string() == unusual.to_str().unwrap())
            .unwrap();
        assert_eq!(
            result.get("outcome").string(),
            if dry_run { "candidate" } else { "deleted" }
        );
        assert_eq!(unusual.exists(), dry_run);
    }
    assert!(repo.exists("topic"));
}

#[test]
fn json_wt_partial_failure_and_query_error_preserve_valid_stdout() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("a", Some("merge"));
    repo.pr(&tip, "a", &merged, old_time());
    let (tip, merged, other) = repo.topic("z", Some("merge"));
    repo.pr(&tip, "z", &merged, old_time());
    repo.git_wrapper("case \"$*\" in *'worktree remove -- '*'tree a') echo 'test removal failure' >&2; exit 1;; esac");
    let output = repo
        .wt_cli()
        .args(["wt", "-d", "3", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("success"), &json_support::Json::Bool(false));
    assert!(
        document
            .get("executions")
            .array()
            .iter()
            .any(|r| r.get("outcome").string() == "failed")
    );
    assert!(
        document
            .get("executions")
            .array()
            .iter()
            .any(|r| r.get("outcome").string() == "deleted")
    );
    assert!(tree.exists());
    assert!(!other.exists());
    fs::write(repo.root.join("gh-fail"), "").unwrap();
    let output = repo
        .wt_cli()
        .args(["wt", "-d", "3", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("error").get("code").string(), "cleanup_failed");
    assert!(tree.exists());
}

#[test]
fn json_and_explain_worktree_output_stays_json() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    let output = checked(repo.wt_cli().args([
        "wt",
        "--format",
        "json",
        "-d",
        "3",
        "--explain",
        "--dry-run",
    ]));
    let document = json_support::parse(&output.stdout);
    assert_eq!(document.get("success"), &json_support::Json::Bool(true));
    assert!(!text(&output.stdout).contains("explain:"));
    assert!(tree.exists());
}

#[test]
fn batch_wt_preserves_preview_and_dirty_worktree_protection() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("batch-topic", Some("merge"));
    repo.pr(&tip, "batch-topic", &merged, old_time());
    let preview = checked(
        repo.wt_cli()
            .args(["batch", "wt", "--days", "3", "--dry-run"])
            .arg(&repo.work),
    );
    assert!(text(&preview.stdout).contains("batch-topic"));
    assert!(tree.exists());
    fs::write(tree.join("untracked"), "keep me").unwrap();
    checked(
        repo.wt_cli()
            .args(["batch", "wt", "--days", "3", "--execute"])
            .arg(&repo.work),
    );
    assert!(tree.exists());
    fs::remove_file(tree.join("untracked")).unwrap();
    repo.age_tree(&tree);
    checked(
        repo.wt_cli()
            .args(["batch", "wt", "--hours", "72", "--execute"])
            .arg(&repo.work),
    );
    assert!(!tree.exists());
    assert!(repo.exists("batch-topic"));
}

#[test]
fn batch_refuses_cleanup_when_repository_identity_recheck_fails() {
    let repo = Repo::wt();
    repo.git(&["branch", "merged"]);
    repo.git_wrapper(&format!(
        "case \"$*\" in *--git-common-dir*)\n if [ -f {marker} ]; then exit 1; fi\n touch {marker}\n;; esac",
        marker = quote(&repo.root.join("inspected-once")),
    ));
    let output = repo
        .wt_cli()
        .current_dir(&repo.root)
        .args(["batch", "br", "--execute"])
        .arg(&repo.work)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("repository identity changed or could not be rechecked"));
    assert!(repo.exists("merged"));
}

#[test]
fn batch_keeps_launch_worktree_even_when_discovery_prefers_main() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("batch-current", Some("merge"));
    repo.pr(&tip, "batch-current", &merged, old_time());
    fs::create_dir(tree.join("subdirectory")).unwrap();
    repo.age_tree(&tree);
    let output = checked(
        repo.wt_cli()
            .current_dir(tree.join("subdirectory"))
            .args(["batch", "wt", "--days", "3", "--execute", "--explain"])
            .arg(&repo.work)
            .arg(&tree),
    );
    assert!(tree.exists());
    assert!(text(&output.stdout).contains("current worktree"));
}
