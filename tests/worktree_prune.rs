use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "giclean-prune-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let r = Self(root);
        r.git(&["init", "-b", "main"]);
        r.git(&["config", "gc.auto", "0"]);
        r.git(&["config", "gc.worktreePruneExpire", "never"]);
        r.git(&["config", "user.name", "Test"]);
        r.git(&["config", "user.email", "test@example.invalid"]);
        r.git(&["commit", "--allow-empty", "-m", "initial"]);
        r
    }
    fn git(&self, args: &[&str]) {
        let o = Command::new("git")
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    fn stale(&self, name: &str, unique: bool) {
        self.git(&["worktree", "add", "--detach", name]);
        if unique {
            self.git(&["-C", name, "commit", "--allow-empty", "-m", "unique"]);
        }
        // Only synthetic fixtures are removed. Real worktrees are never touched.
        fs::remove_dir_all(self.0.join(name)).unwrap();
        age(&self.0.join(".git/worktrees").join(name));
    }
    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }
    fn run_with_env(&self, args: &[&str], env: &[(&str, &Path)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_giclean"))
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .envs(env.iter().copied())
            .arg("wt-prune")
            .args(args)
            .output()
            .unwrap()
    }
    fn exists(&self, name: &str) -> bool {
        self.0.join(".git/worktrees").join(name).exists()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn age(path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            age(&entry.unwrap().path());
        }
    }
    File::open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(946684800)))
        .unwrap();
}
#[test]
fn dry_run_then_execute_removes_only_registration() {
    let r = Repo::new();
    r.stale("stale", false);
    fs::create_dir(r.0.join("stale")).unwrap();
    fs::write(r.0.join("stale/keep.txt"), "valuable files").unwrap();
    let o = r.run(&["-d", "3", "--dry-run"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(r.exists("stale"));
    let o = r.run(&["-d", "3", "--execute"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!r.exists("stale"));
    assert_eq!(
        fs::read_to_string(r.0.join("stale/keep.txt")).unwrap(),
        "valuable files"
    );
}
#[test]
fn one_unique_commit_blocks_entire_prune() {
    let r = Repo::new();
    r.stale("safe", false);
    r.stale("unique", true);
    let o = r.run(&["-d", "1", "--execute"]);
    assert!(!o.status.success());
    assert!(r.exists("safe"));
    assert!(r.exists("unique"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unique registered"));
}
#[test]
fn recent_registration_blocks_entire_prune() {
    let r = Repo::new();
    r.stale("safe", false);
    r.stale("recent", false);
    File::open(r.0.join(".git/worktrees/recent/HEAD"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(std::time::SystemTime::now()))
        .unwrap();
    assert!(!r.run(&["-d", "2", "--execute"]).status.success());
    assert!(r.exists("safe"));
    assert!(r.exists("recent"));
}
#[test]
fn live_and_locked_registrations_survive() {
    let r = Repo::new();
    r.stale("stale", false);
    r.git(&["worktree", "add", "--detach", "live"]);
    r.git(&["worktree", "add", "--detach", "locked"]);
    r.git(&["worktree", "lock", "locked"]);
    fs::remove_dir_all(r.0.join("locked")).unwrap();
    let o = r.run(&["-h", "1", "--execute"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(r.exists("live"));
    assert!(r.exists("locked"));
    assert!(!r.exists("stale"));
}
#[test]
fn mode_and_age_are_required() {
    let r = Repo::new();
    for args in [
        vec![],
        vec!["--execute"],
        vec!["-d", "1"],
        vec!["-d", "0", "--execute"],
        vec!["-d", "1", "--execute", "--dry-run"],
    ] {
        assert!(!r.run(&args).status.success());
    }
}
#[test]
fn unique_reflog_commit_is_retained_even_after_reset() {
    let r = Repo::new();
    r.git(&["worktree", "add", "--detach", "stale"]);
    r.git(&["-C", "stale", "commit", "--allow-empty", "-m", "unique"]);
    r.git(&["-C", "stale", "reset", "--hard", "main"]);
    fs::remove_dir_all(r.0.join("stale")).unwrap();
    age(&r.0.join(".git/worktrees/stale"));
    assert!(!r.run(&["-d", "1", "--execute"]).status.success());
    assert!(r.exists("stale"));
}

#[test]
fn staged_contents_block_prune() {
    let r = Repo::new();
    r.git(&["worktree", "add", "--detach", "stale"]);
    fs::write(r.0.join("stale/pending"), "uncommitted").unwrap();
    r.git(&["-C", "stale", "add", "pending"]);
    fs::remove_dir_all(r.0.join("stale")).unwrap();
    age(&r.0.join(".git/worktrees/stale"));
    let o = r.run(&["-h", "1", "--execute"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("index"));
    assert!(r.exists("stale"));
}

#[test]
fn malformed_registration_blocks_other_candidates() {
    let r = Repo::new();
    r.stale("safe", false);
    fs::create_dir(r.0.join(".git/worktrees/broken")).unwrap();
    assert!(!r.run(&["-h", "1", "--execute"]).status.success());
    assert!(r.exists("safe"));
    assert!(r.exists("broken"));
}

#[test]
fn caller_selected_age_is_not_fixed() {
    let r = Repo::new();
    r.stale("stale", false);
    let p = r.0.join(".git/worktrees/stale/HEAD");
    File::open(p)
        .unwrap()
        .set_times(
            FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(7200)),
        )
        .unwrap();
    assert!(!r.run(&["-h", "3", "--dry-run"]).status.success());
    assert!(r.run(&["-h", "1", "--dry-run"]).status.success());
    assert!(r.exists("stale"));
}

#[test]
fn replacement_objects_cannot_hide_unique_history() {
    let r = Repo::new();
    r.stale("unique", true);
    let oid = fs::read_to_string(r.0.join(".git/worktrees/unique/HEAD")).unwrap();
    r.git(&["replace", oid.trim(), "main"]);
    let o = r.run(&["-h", "1", "--execute"]);
    assert!(!o.status.success());
    assert!(r.exists("unique"));
}

#[test]
fn grafted_history_is_retained() {
    let r = Repo::new();
    r.stale("safe", false);
    fs::write(r.0.join(".git/info/grafts"), "").unwrap();
    assert!(!r.run(&["-h", "1", "--execute"]).status.success());
    assert!(r.exists("safe"));
}

#[test]
fn alternate_clean_index_cannot_hide_registered_staged_changes() {
    let r = Repo::new();
    r.git(&["worktree", "add", "--detach", "stale"]);
    fs::write(r.0.join("stale/pending"), "only staged copy").unwrap();
    r.git(&["-C", "stale", "add", "pending"]);
    let index = r.0.join(".git/worktrees/stale/index");
    let original = fs::read(&index).unwrap();
    fs::remove_dir_all(r.0.join("stale")).unwrap();
    age(&r.0.join(".git/worktrees/stale"));
    // Create a clean alternate index even though the initial commit is empty.
    r.git(&["read-tree", "HEAD"]);
    for mode in ["--dry-run", "--execute"] {
        let out = r.run_with_env(
            &["-h", "1", mode],
            &[("GIT_INDEX_FILE", &r.0.join(".git/index"))],
        );
        assert!(
            !out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("index"));
        assert_eq!(fs::read(&index).unwrap(), original);
    }
}

#[test]
fn dirty_alternate_index_does_not_replace_clean_registration_index() {
    let r = Repo::new();
    r.stale("stale", false);
    fs::write(r.0.join("pending"), "main staged file").unwrap();
    r.git(&["add", "pending"]);
    let alternate = r.0.join(".git/index");
    let original = fs::read(&alternate).unwrap();
    let out = r.run_with_env(&["-h", "1", "--execute"], &[("GIT_INDEX_FILE", &alternate)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!r.exists("stale"));
    assert_eq!(fs::read(alternate).unwrap(), original);
}

#[test]
fn missing_or_corrupt_registration_index_is_not_substituted() {
    for missing in [true, false] {
        let r = Repo::new();
        r.stale("stale", false);
        r.git(&["read-tree", "HEAD"]);
        let index = r.0.join(".git/worktrees/stale/index");
        if missing {
            fs::remove_file(&index).unwrap();
        } else {
            fs::write(&index, "corrupt index").unwrap();
        }
        age(&r.0.join(".git/worktrees/stale"));
        let out = r.run_with_env(
            &["-h", "1", "--execute"],
            &[("GIT_INDEX_FILE", &r.0.join(".git/index"))],
        );
        assert!(!out.status.success());
        assert!(r.exists("stale"));
        assert!(String::from_utf8_lossy(&out.stderr).contains("index"));
    }
}

#[test]
fn inherited_repo_selection_does_not_redirect_registration_inspection() {
    let selected = Repo::new();
    selected.stale("stale", false);
    let cwd = Repo::new();
    cwd.stale("keep", true);
    let out = cwd.run_with_env(
        &["-h", "1", "--execute"],
        &[
            ("GIT_DIR", &selected.0.join(".git")),
            ("GIT_COMMON_DIR", &selected.0.join(".git")),
            ("GIT_WORK_TREE", &selected.0),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!selected.exists("stale"));
    assert!(cwd.exists("keep"));
}
