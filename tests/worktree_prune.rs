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
            .args(["wt", "prune"])
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

fn cli(repo: &Repo, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_giclean"))
        .current_dir(&repo.0)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap()
}

const WARNING: &str = "giclean: wt-prune is deprecated; use `giclean wt prune` instead.\n";

#[test]
fn alias_and_canonical_have_equal_results_except_deprecation_warning() {
    let r = Repo::new();
    r.stale("safe", false);
    for args in [
        vec!["-d", "3", "--dry-run"],
        vec!["-d", "3"],
        vec!["-d", "0", "--execute"],
        vec!["-d", "3", "--execute", "--dry-run"],
        vec!["--help"],
    ] {
        let old = cli(&r, &[&["wt-prune"][..], &args].concat());
        let new = cli(&r, &[&["wt", "prune"][..], &args].concat());
        assert_eq!(old.status.code(), new.status.code());
        assert_eq!(old.stdout, new.stdout);
        assert_eq!(old.stderr, [WARNING.as_bytes(), &new.stderr].concat());
        assert!(r.exists("safe"));
    }
    // Failure retains every registration for either spelling.
    r.stale("unique", true);
    let old = cli(&r, &["wt-prune", "-d", "3", "--execute"]);
    let new = r.run(&["-d", "3", "--execute"]);
    assert_eq!(old.status.code(), new.status.code());
    assert_eq!(old.stdout, new.stdout);
    assert_eq!(old.stderr, [WARNING.as_bytes(), &new.stderr].concat());
    assert!(r.exists("safe") && r.exists("unique"));
}

#[test]
fn alias_execute_still_only_removes_registration() {
    let r = Repo::new();
    r.stale("safe", false);
    r.git(&["branch", "keep"]);
    let old = cli(&r, &["wt-prune", "-d", "3", "--execute"]);
    assert!(old.status.success());
    assert_eq!(old.stderr, WARNING.as_bytes());
    assert!(!r.exists("safe"));
    r.git(&["rev-parse", "--verify", "keep"]);
    r.stale("safe", false);
    let new = r.run(&["-d", "3", "--execute"]);
    assert_eq!(old.status.code(), new.status.code());
    assert_eq!(old.stdout, new.stdout);
    assert!(new.stderr.is_empty());
    assert!(!r.exists("safe"));
    r.git(&["rev-parse", "--verify", "keep"]);
}

#[test]
fn config_does_not_supply_prune_age_and_audit_keeps_command_identity() {
    let r = Repo::new();
    r.stale("safe", false);
    // A flag-shaped path must stay a path, not be treated as a command or flag.
    fs::write(r.0.join("wt-prune"), "wt-days=3\n").unwrap();
    for prefix in [
        vec!["--config", "wt-prune", "--audit-log", "log-a"],
        vec!["--audit-log", "log-b", "--config", "wt-prune"],
    ] {
        let args = [&prefix[..], &["wt", "prune", "--execute"]].concat();
        let out = cli(&r, &args);
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("deprecated"));
        assert!(r.exists("safe"));
    }
    for name in ["log-a", "log-b"] {
        let text = fs::read_to_string(r.0.join(name)).unwrap();
        assert!(text.contains("\"command\":\"wt-prune\""));
        assert!(text.contains("\"target_results_available\":false"));
    }
    let out = cli(
        &r,
        &[
            "--config",
            "wt-prune",
            "--audit-log",
            "--config",
            "wt",
            "prune",
            "-d",
            "3",
            "--dry-run",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(
        fs::read_to_string(r.0.join("--config"))
            .unwrap()
            .contains("\"command\":\"wt-prune\"")
    );
    assert!(r.exists("safe"));
}

#[test]
fn help_discovers_subcommand_without_changing_other_command_positions() {
    let r = Repo::new();
    let help = cli(&r, &["wt", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("giclean wt prune --help"));
    assert!(help.stderr.is_empty());
    for args in [
        vec!["br", "prune", "--dry-run"],
        vec!["wt", "-d", "3", "prune", "--dry-run"],
    ] {
        let out = cli(&r, &args);
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("deprecated"));
    }
}

#[test]
fn batch_still_treats_prune_as_a_root_path_not_a_subcommand() {
    let r = Repo::new();
    r.stale("safe", false);
    fs::create_dir(r.0.join("prune")).unwrap();
    let out = cli(&r, &["batch", "wt", "prune", "-d", "3", "--execute"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(r.exists("safe"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("registration"));
    assert!(out.stderr.is_empty());
}

fn configured_worktree(r: &Repo, name: &str) {
    fs::create_dir_all(r.0.join("src")).unwrap();
    fs::create_dir_all(r.0.join("docs")).unwrap();
    fs::write(r.0.join("src/kept"), "tracked").unwrap();
    fs::write(r.0.join("docs/hidden"), "outside sparse checkout").unwrap();
    r.git(&["add", "src", "docs"]);
    r.git(&["commit", "-m", "tracked files"]);
    r.git(&["config", "extensions.worktreeConfig", "true"]);
    r.git(&["worktree", "add", "--detach", name]);
    r.git(&["-C", name, "config", "--worktree", "core.bare", "false"]);
    r.git(&[
        "-C",
        name,
        "sparse-checkout",
        "set",
        "--cone",
        "--sparse-index",
        "src",
    ]);
}

#[test]
fn git_generated_sparse_config_and_patterns_can_be_pruned() {
    let r = Repo::new();
    configured_worktree(&r, "stale");
    let admin = r.0.join(".git/worktrees/stale");
    assert!(admin.join("config.worktree").is_file());
    assert!(admin.join("info/sparse-checkout").is_file());
    fs::remove_dir_all(r.0.join("stale")).unwrap();
    age(&admin);
    let before = fs::read(admin.join("config.worktree")).unwrap();
    let preview = r.run(&["-d", "5", "--dry-run"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&preview.stdout),
        "Would prune registration stale\n"
    );
    assert_eq!(fs::read(admin.join("config.worktree")).unwrap(), before);
    let execute = r.run(&["-d", "5", "--execute"]);
    assert!(
        execute.status.success(),
        "{}",
        String::from_utf8_lossy(&execute.stderr)
    );
    assert!(!r.exists("stale"));
}

#[test]
fn config_does_not_bypass_staged_or_unique_commit_protection() {
    for unique in [false, true] {
        let r = Repo::new();
        r.stale("safe", false);
        configured_worktree(&r, "keep");
        if unique {
            r.git(&["-C", "keep", "commit", "--allow-empty", "-m", "unique"]);
        } else {
            fs::create_dir_all(r.0.join("keep/src")).unwrap();
            fs::write(r.0.join("keep/src/pending"), "staged only").unwrap();
            r.git(&["-C", "keep", "add", "src/pending"]);
        }
        fs::remove_dir_all(r.0.join("keep")).unwrap();
        age(&r.0.join(".git/worktrees/keep"));
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains(if unique {
                "unique registered"
            } else {
                "staged or unverifiable index"
            }),
            "{error}"
        );
        assert!(r.exists("safe") && r.exists("keep"));
    }
}

#[test]
fn unsupported_or_invalid_config_retains_all_registrations() {
    for config in [
        "[core]\nworktree = ../elsewhere\n",
        "[core]\nbare = true\n",
        "[core]\nsparseCheckout = invalid\n",
        "[include]\npath = ordinary-extra-config\n",
        "[user]\nname = Test\n",
        "[invalid\n",
    ] {
        let r = Repo::new();
        r.stale("safe", false);
        r.stale("keep", false);
        let admin = r.0.join(".git/worktrees/keep");
        fs::write(admin.join("config.worktree"), config).unwrap();
        age(&admin);
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(!out.status.success(), "{config}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("config.worktree"),
            "{config}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(r.exists("safe") && r.exists("keep"));
    }
}

#[test]
fn recent_config_and_unknown_info_files_are_retained() {
    for unknown_info in [false, true] {
        let r = Repo::new();
        configured_worktree(&r, "keep");
        fs::remove_dir_all(r.0.join("keep")).unwrap();
        let admin = r.0.join(".git/worktrees/keep");
        age(&admin);
        if unknown_info {
            fs::write(admin.join("info/unknown"), "keep").unwrap();
            age(&admin);
        } else {
            fs::write(admin.join("config.worktree"), "[core]\nbare = false\n").unwrap();
        }
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(!out.status.success());
        assert!(r.exists("keep"));
    }
}

#[cfg(unix)]
#[test]
fn symlink_config_or_sparse_patterns_are_retained() {
    use std::os::unix::fs::symlink;
    for name in ["config.worktree", "info/sparse-checkout", "FETCH_HEAD"] {
        let r = Repo::new();
        r.stale("keep", false);
        let admin = r.0.join(".git/worktrees/keep");
        fs::create_dir_all(admin.join("info")).unwrap();
        let target = r.0.join("ordinary-file");
        fs::write(&target, "").unwrap();
        symlink(&target, admin.join(name)).unwrap();
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("symlink"));
        assert!(r.exists("keep"));
        assert!(target.exists());
    }
}

#[test]
fn split_index_metadata_remains_retained() {
    let r = Repo::new();
    configured_worktree(&r, "keep");
    r.git(&["-C", "keep", "sparse-checkout", "disable"]);
    r.git(&["-C", "keep", "update-index", "--split-index"]);
    let admin = r.0.join(".git/worktrees/keep");
    assert!(fs::read_dir(&admin).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")
    }));
    fs::remove_dir_all(r.0.join("keep")).unwrap();
    age(&admin);
    let out = r.run(&["-d", "5", "--execute"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("sharedindex."));
    assert!(r.exists("keep"));
}

#[test]
fn codex_environment_pointer_is_inert_and_case_insensitive() {
    for key in [
        "codex.localenvironmentconfigpath",
        "CoDeX.LocalEnvironmentConfigPath",
    ] {
        let r = Repo::new();
        configured_worktree(&r, "stale");
        let target = r.0.join("environment files/local settings.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        // Not Git config: success demonstrates the pointer is not an include.
        fs::write(&target, "environment metadata, not Git configuration\n").unwrap();
        r.git(&[
            "-C",
            "stale",
            "config",
            "--worktree",
            key,
            target.to_str().unwrap(),
        ]);
        fs::remove_dir_all(r.0.join("stale")).unwrap();
        let admin = r.0.join(".git/worktrees/stale");
        age(&admin);
        let config = fs::read(admin.join("config.worktree")).unwrap();
        let out = r.run(&["-d", "5", "--dry-run"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(fs::read(admin.join("config.worktree")).unwrap(), config);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "Would prune registration stale\n"
        );
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!r.exists("stale"));
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "environment metadata, not Git configuration\n"
        );
    }
}

#[test]
fn codex_pointer_does_not_allow_other_keys_or_unique_history() {
    for extra in [
        "codex.other",
        "core.worktree",
        "include.path",
        "unique-history",
    ] {
        let r = Repo::new();
        r.stale("safe", false);
        r.git(&["config", "extensions.worktreeConfig", "true"]);
        r.git(&["worktree", "add", "--detach", "keep"]);
        r.git(&[
            "-C",
            "keep",
            "config",
            "--worktree",
            "codex.localEnvironmentConfigPath",
            "nonexistent config with spaces.toml",
        ]);
        if extra == "unique-history" {
            r.git(&["-C", "keep", "commit", "--allow-empty", "-m", "unique"]);
        } else {
            r.git(&["-C", "keep", "config", "--worktree", extra, "unverified"]);
        }
        fs::remove_dir_all(r.0.join("keep")).unwrap();
        age(&r.0.join(".git/worktrees/keep"));
        let out = r.run(&["-d", "5", "--execute"]);
        assert!(!out.status.success(), "{extra}");
        assert!(r.exists("safe") && r.exists("keep"));
    }
}

#[test]
fn fetched_commit_is_verified_even_when_not_for_merge() {
    for unique in [false, true] {
        for not_for_merge in [false, true] {
            let r = Repo::new();
            let donor = Repo::new();
            donor.git(&["commit", "--allow-empty", "-m", "unique fetched commit"]);
            r.stale("safe", false);
            r.git(&["worktree", "add", "--detach", "keep"]);
            let remote = if unique { &donor.0 } else { &r.0 };
            r.git(&[
                "-C",
                "keep",
                "fetch",
                "--no-tags",
                remote.to_str().unwrap(),
                "main",
            ]);
            let admin = r.0.join(".git/worktrees/keep");
            let fetch_head = admin.join("FETCH_HEAD");
            let original = fs::read_to_string(&fetch_head).unwrap();
            let record = if not_for_merge {
                original.replacen("\t\t", "\tnot-for-merge\t", 1)
            } else {
                original
            };
            // Duplicate fetch records are ordinary and must not lose protection.
            fs::write(&fetch_head, format!("{record}{record}")).unwrap();
            fs::remove_dir_all(r.0.join("keep")).unwrap();
            age(&admin);
            let out = r.run(&["-d", "5", "--execute"]);
            assert_eq!(
                out.status.success(),
                !unique,
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(r.exists("keep"), unique);
            assert_eq!(r.exists("safe"), unique);
            if unique {
                assert!(String::from_utf8_lossy(&out.stderr).contains("HEAD/reflog/FETCH_HEAD"));
            }
        }
    }
}

#[test]
fn empty_fetch_head_is_safe_but_malformed_or_noncommit_records_are_retained() {
    for kind in [
        "empty",
        "malformed",
        "bad-marker",
        "missing-description",
        "tag",
        "tree",
    ] {
        let r = Repo::new();
        r.stale("keep", false);
        let admin = r.0.join(".git/worktrees/keep");
        let head = fs::read_to_string(admin.join("HEAD")).unwrap();
        let oid = head.trim();
        let contents = match kind {
            "empty" => String::new(),
            "malformed" => format!("{oid} no tabs\n"),
            "bad-marker" => format!("{oid}\tother\tbranch main\n"),
            "missing-description" => format!("{oid}\t\t\n"),
            "tag" | "tree" => {
                r.git(&["tag", "-a", "fetched-tag", "-m", "annotated", "HEAD"]);
                let revision = if kind == "tag" {
                    "fetched-tag"
                } else {
                    "HEAD^{tree}"
                };
                let output = Command::new("git")
                    .current_dir(&r.0)
                    .args(["rev-parse", revision])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                format!(
                    "{}\t\tfetched object\n",
                    String::from_utf8(output.stdout).unwrap().trim()
                )
            }
            _ => unreachable!(),
        };
        fs::write(admin.join("FETCH_HEAD"), contents).unwrap();
        age(&admin);
        let out = r.run(&["-d", "5", "--execute"]);
        assert_eq!(
            out.status.success(),
            kind == "empty",
            "{kind}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(r.exists("keep"), kind != "empty");
    }
}

#[test]
fn codex_thread_metadata_remains_protected() {
    let r = Repo::new();
    r.stale("keep", false);
    let admin = r.0.join(".git/worktrees/keep");
    fs::write(admin.join("codex-thread.json"), "{}").unwrap();
    age(&admin);
    assert!(!r.run(&["-d", "5", "--execute"]).status.success());
    assert!(r.exists("keep"));
}
