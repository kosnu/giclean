use super::*;

fn assert_unknown(output: &Output, tree: &Path, detail: &str, outcome: &str) {
    assert_success(output);
    let document = json_support::parse(&output.stdout);
    let decision = document
        .get("decisions")
        .array()
        .iter()
        .find(|entry| entry.get("target").get("path").string() == tree.to_str().unwrap())
        .unwrap();
    assert_eq!(decision.get("reason_code").string(), "agent_unknown");
    assert_eq!(decision.get("eligibility").string(), "unknown");
    assert_eq!(decision.get("detail").string(), detail);
    let execution = document
        .get("executions")
        .array()
        .iter()
        .find(|entry| entry.get("target").get("path").string() == tree.to_str().unwrap())
        .unwrap();
    assert_eq!(execution.get("outcome").string(), outcome);
    assert!(tree.exists());
}

#[test]
fn codex_home_keeps_every_tree_without_interpreting_session_contents_or_age() {
    let repo = Repo::wt();
    let (_, _, first) = repo.topic("a", Some("merge"));
    let (_, _, second) = repo.topic("b", None);
    let home = repo.root.join(".codex");
    fs::create_dir_all(home.join("sessions")).unwrap();
    // Metadata-only synthetic subsets of the public 0.167.2 ThreadStatus
    // format, plus history/unsupported/corrupt inputs. No real sessions.
    let fixtures: &[(&str, &[u8])] = &[
        ("empty", b""),
        (
            "active.json",
            br#"{"status":{"type":"active","activeFlags":[]}}"#,
        ),
        ("idle.json", br#"{"status":{"type":"idle"}}"#),
        (
            "approval.json",
            br#"{"status":{"type":"active","activeFlags":["waitingOnApproval"]}}"#,
        ),
        (
            "input.json",
            br#"{"status":{"type":"active","activeFlags":["waitingOnUserInput"]}}"#,
        ),
        (
            "archived.jsonl",
            br#"{"type":"session_meta","payload":{"cwd":"/elsewhere","cli_version":"0.50.0"}}"#,
        ),
        (
            "future.jsonl",
            br#"{"type":"future","payload":{"cli_version":"99.0.0"}}"#,
        ),
        ("partial.jsonl", b"{\"type\":"),
        ("compressed.jsonl.zst", b"\x28\xb5\x2f\xfd\x00"),
        ("private-marker", b"SYNTHETIC_DO_NOT_OUTPUT"),
    ];
    for (name, contents) in fixtures {
        let fixture = home.join("sessions").join(name);
        fs::write(&fixture, contents).unwrap();
        age_directory(&home);
        let modified = fs::metadata(&fixture).unwrap().modified().unwrap();
        for dry in [true, false] {
            let mut command = repo.wt_cli();
            command.args(["wt", "-d", "3", "--format", "json"]);
            if dry {
                command.arg("--dry-run");
            }
            let output = command.output().unwrap();
            for tree in [&first, &second] {
                assert_unknown(&output, tree, "codex: incomplete_coverage", "not_attempted");
            }
            assert!(!text(&output.stdout).contains("SYNTHETIC_DO_NOT_OUTPUT"));
            assert!(!text(&output.stderr).contains("SYNTHETIC_DO_NOT_OUTPUT"));
            assert_eq!(fs::read(&fixture).unwrap(), *contents);
            assert_eq!(
                fs::metadata(&fixture).unwrap().modified().unwrap(),
                modified
            );
        }
    }
    // Protection is decided before PR or origin queries, including an unmerged
    // tree. A home is uncertainty, not a claim about which thread uses a tree.
    assert!(!repo.root.join("gh-calls").exists());
    let output = checked(
        repo.wt_cli()
            .args(["wt", "-d", "3", "--dry-run", "--explain"]),
    );
    assert!(text(&output.stdout).contains("agent usage could not be ruled out safely"));
    assert!(text(&output.stdout).contains("codex: incomplete_coverage"));
    assert!(!text(&output.stdout).contains("candidate:"));
    assert!(!repo.work.join(".git/FETCH_HEAD").exists());
}

#[test]
fn codex_home_configuration_and_path_failures_are_unknown() {
    let repo = Repo::wt();
    let (_, _, tree) = repo.topic("a", Some("merge"));
    let custom = repo.root.join("custom-codex");
    fs::create_dir(&custom).unwrap();
    let missing = repo.root.join("missing");
    let file = repo.root.join("file");
    fs::write(&file, "SYNTHETIC_SECRET").unwrap();
    let symlink = repo.root.join("alias");
    let dangling = repo.root.join("dangling");
    std::os::unix::fs::symlink(&custom, &symlink).unwrap();
    std::os::unix::fs::symlink(&missing, &dangling).unwrap();
    for (value, failure) in [
        (custom.as_os_str(), "incomplete_coverage"),
        (missing.as_os_str(), "unavailable"),
        (file.as_os_str(), "invalid_data"),
        (symlink.as_os_str(), "path_uncertain"),
        (dangling.as_os_str(), "path_uncertain"),
        (std::ffi::OsStr::new("relative-home"), "path_uncertain"),
        (std::ffi::OsStr::new(""), "path_uncertain"),
    ] {
        let output = repo
            .wt_cli()
            .env("CODEX_HOME", value)
            .args(["wt", "-d", "3", "--format", "json"])
            .output()
            .unwrap();
        assert_unknown(
            &output,
            &tree,
            &format!("codex: {failure}"),
            "not_attempted",
        );
        assert!(!text(&output.stdout).contains("SYNTHETIC_SECRET"));
        assert!(!text(&output.stdout).contains(custom.to_str().unwrap()));
    }
    for (home, failure) in [
        (None, "unavailable"),
        (Some(missing.as_os_str()), "read_failed"),
        (Some(file.as_os_str()), "invalid_data"),
        (Some(std::ffi::OsStr::new("")), "path_uncertain"),
        (Some(std::ffi::OsStr::new("relative")), "path_uncertain"),
    ] {
        let mut command = repo.wt_cli();
        if let Some(home) = home {
            command.env("HOME", home);
        } else {
            command.env_remove("HOME");
        }
        let output = command
            .args(["wt", "-d", "3", "--format", "json"])
            .output()
            .unwrap();
        assert_unknown(
            &output,
            &tree,
            &format!("codex: {failure}"),
            "not_attempted",
        );
    }
    // Explicit CODEX_HOME takes precedence even when HOME is unavailable.
    let output = repo
        .wt_cli()
        .env_remove("HOME")
        .env("CODEX_HOME", custom)
        .args(["wt", "-d", "3", "--format", "json"])
        .output()
        .unwrap();
    assert_unknown(
        &output,
        &tree,
        "codex: incomplete_coverage",
        "not_attempted",
    );
}

#[test]
fn codex_default_home_alias_and_inaccessible_children_cannot_clear_protection() {
    for alias in [false, true] {
        let repo = Repo::wt();
        let (_, _, tree) = repo.topic("a", Some("merge"));
        let home = repo.root.join(".codex");
        let detail = if alias {
            std::os::unix::fs::symlink(repo.root.join("missing"), &home).unwrap();
            "codex: path_uncertain"
        } else {
            fs::create_dir_all(home.join("sessions")).unwrap();
            fs::set_permissions(home.join("sessions"), fs::Permissions::from_mode(0o000)).unwrap();
            "codex: incomplete_coverage"
        };
        let output = repo
            .wt_cli()
            .args(["wt", "-d", "3", "--format", "json"])
            .output()
            .unwrap();
        assert_unknown(&output, &tree, detail, "not_attempted");
        if !alias {
            fs::set_permissions(home.join("sessions"), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

#[test]
fn codex_new_home_or_source_failure_after_initial_inspection_vetoes_deletion() {
    for action in ["mkdir \"$HOME/.codex\"", "mv \"$HOME\" \"$HOME.before\""] {
        for dry in [true, false] {
            let repo = Repo::wt();
            let (tip, merged, tree) = repo.topic("a", Some("merge"));
            repo.pr(&tip, "a", &merged, old_time());
            let home = repo.root.join("user-home");
            fs::create_dir(&home).unwrap();
            fs::write(repo.root.join("gh-action"), action).unwrap();
            let mut command = repo.wt_cli();
            command
                .env("HOME", home)
                .args(["wt", "-d", "3", "--format", "json"]);
            if dry {
                command.arg("--dry-run");
            }
            let output = command.output().unwrap();
            let detail = if action.starts_with("mkdir") {
                "codex: incomplete_coverage"
            } else {
                "codex: read_failed"
            };
            assert_unknown(&output, &tree, detail, "skipped");
            assert!(repo.root.join("gh-calls").exists());
            assert!(repo.exists("a"));
        }
    }
}

#[test]
fn codex_observation_is_refreshed_after_the_final_git_snapshot() {
    for dry in [true, false] {
        let repo = Repo::wt();
        let (tip, merged, tree) = repo.topic("a", Some("merge"));
        repo.pr(&tip, "a", &merged, old_time());
        repo.git_wrapper(
            r#"case "$*" in
 *'worktree list --porcelain -z'*)
   if [ -f "$TEST_GH_ROOT/initial-snapshot" ]; then mkdir -p "$HOME/.codex";
   else : > "$TEST_GH_ROOT/initial-snapshot"; fi;;
esac"#,
        );
        let mut command = repo.wt_cli();
        command.args(["wt", "-d", "3", "--format", "json"]);
        if dry {
            command.arg("--dry-run");
        }
        let output = command.output().unwrap();
        assert_unknown(&output, &tree, "codex: incomplete_coverage", "skipped");
    }
}

#[test]
fn codex_home_has_no_effect_on_br() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    fs::create_dir(repo.root.join(".codex")).unwrap();
    let output = checked(repo.cli().args(["br", "--dry-run"]));
    assert!(text(&output.stdout).contains("candidate: topic"));
    assert!(repo.exists("topic"));
}

#[test]
fn codex_home_protection_is_inherited_by_batch_wt() {
    let repo = Repo::wt();
    let (tip, merged, tree) = repo.topic("topic", Some("merge"));
    repo.pr(&tip, "topic", &merged, old_time());
    let home = repo.root.join("custom-codex");
    fs::create_dir(&home).unwrap();
    for mode in ["--dry-run", "--execute"] {
        let output = checked(
            repo.wt_cli()
                .env("CODEX_HOME", &home)
                .args(["batch", "wt", "-d", "3", mode, "--explain"])
                .arg(&repo.work),
        );
        assert!(text(&output.stdout).contains("codex: incomplete_coverage"));
        assert!(!text(&output.stdout).contains("candidate:"));
        assert!(!text(&output.stdout).contains(": deleted"));
        assert!(tree.exists());
    }
    assert!(!repo.root.join("gh-calls").exists());
}
