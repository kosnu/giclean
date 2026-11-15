use super::*;

#[test]
fn audit_opt_in_records_outcomes_without_names_or_paths() {
    let repo = Repo::new();
    repo.git(&["branch", "private-customer-secret"]);
    let oid = repo.git(&["rev-parse", "HEAD"]);
    let log = repo.root.join("audit.jsonl");
    assert_success(
        &repo
            .cli()
            .arg("--audit-log")
            .arg(&log)
            .arg("br")
            .output()
            .unwrap(),
    );
    assert!(!repo.exists("private-customer-secret"));
    let content = fs::read_to_string(&log).unwrap();
    assert_eq!(content.lines().count(), 2);
    assert!(content.contains("\"event\":\"started\""));
    assert!(content.contains("\"success\":true"));
    assert!(content.contains("\"outcome\":\"deleted\""));
    assert!(content.contains(&oid));
    assert!(!content.contains("private-customer-secret"));
    assert!(!content.contains(repo.root.to_str().unwrap()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(repo.git(&["cat-file", "-t", &oid]), "commit");
}

#[test]
fn audit_dry_run_and_failed_commands_are_not_deletions() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let log = repo.root.join("preview.jsonl");
    assert_success(
        &repo
            .cli()
            .arg("--audit-log")
            .arg(&log)
            .args(["br", "--dry-run", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert!(repo.exists("topic"));
    let content = fs::read_to_string(log).unwrap();
    assert!(content.contains("\"dry_run_token_present\":true"));
    assert!(content.contains("\"outcome\":\"candidate\""));
    assert!(!content.contains("\"outcome\":\"deleted\""));
    let failed = repo.root.join("failed.jsonl");
    assert!(
        !repo
            .cli()
            .arg("--audit-log")
            .arg(&failed)
            .args(["br", "--invalid-private-value"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let content = fs::read_to_string(failed).unwrap();
    assert!(content.contains("\"success\":false"));
    assert!(!content.contains("invalid-private-value"));
}

#[test]
fn audit_setup_failure_and_existing_file_leave_refs_unchanged() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let existing = repo.root.join("existing");
    fs::write(&existing, "unchanged").unwrap();
    for path in [existing.clone(), repo.root.join("missing-parent/log")] {
        assert!(
            !repo
                .cli()
                .arg("--audit-log")
                .arg(path)
                .arg("br")
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(repo.exists("topic"));
    }
    assert_eq!(fs::read_to_string(existing).unwrap(), "unchanged");
}

#[cfg(unix)]
#[test]
fn audit_refuses_symlink() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let target = repo.root.join("target");
    fs::write(&target, "unchanged").unwrap();
    let link = repo.root.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(
        !repo
            .cli()
            .arg("--audit-log")
            .arg(link)
            .arg("br")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(repo.exists("topic"));
    assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
}

#[test]
fn audit_prune_reports_summary_without_claiming_target_results() {
    let repo = Repo::new();
    let log = repo.root.join("prune.jsonl");
    assert_success(
        &repo
            .cli()
            .arg("--audit-log")
            .arg(&log)
            .args(["wt-prune", "-d", "5", "--dry-run"])
            .output()
            .unwrap(),
    );
    let content = fs::read_to_string(log).unwrap();
    assert!(content.contains("\"target_results_available\":false"));
    assert!(content.contains("\"results\":[]"));
}

#[test]
fn audit_and_config_compose_in_either_prefix_order() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let config = repo.root.join("settings");
    fs::write(&config, "wt-days=5\n").unwrap();
    for audit_first in [true, false] {
        let log = repo.root.join(format!("combined-{audit_first}.jsonl"));
        let mut cmd = repo.cli();
        if audit_first {
            cmd.arg("--audit-log")
                .arg(&log)
                .arg("--config")
                .arg(&config);
        } else {
            cmd.arg("--config")
                .arg(&config)
                .arg("--audit-log")
                .arg(&log);
        }
        assert_success(&cmd.args(["br", "--dry-run"]).output().unwrap());
        assert!(repo.exists("topic"));
        let content = fs::read_to_string(log).unwrap();
        assert!(content.contains("\"outcome\":\"candidate\""));
        assert!(!content.contains("\"outcome\":\"deleted\""));
        // No CLI age: wt must receive the configured age before dispatch.
        let wt_log = repo.root.join(format!("wt-{audit_first}.jsonl"));
        assert_success(
            &repo
                .cli()
                .arg("--audit-log")
                .arg(&wt_log)
                .arg("--config")
                .arg(&config)
                .args(["wt", "--dry-run"])
                .output()
                .unwrap(),
        );
    }
}

#[test]
fn combined_prefix_errors_do_not_mutate_or_overwrite() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let config = repo.root.join("settings");
    fs::write(&config, "wt-days=5\n").unwrap();
    let log = repo.root.join("existing");
    fs::write(&log, "untouched").unwrap();
    assert!(
        !repo
            .cli()
            .arg("--config")
            .arg(&config)
            .arg("--audit-log")
            .arg(&log)
            .arg("br")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "untouched");
    assert!(repo.exists("topic"));
    let fresh = repo.root.join("fresh");
    assert!(
        !repo
            .cli()
            .arg("--audit-log")
            .arg(&fresh)
            .arg("--config")
            .arg(&config)
            .arg("--audit-log")
            .arg(&log)
            .arg("br")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!fresh.exists());
    assert!(repo.exists("topic"));
}

#[test]
fn combined_prefix_does_not_consume_flag_shaped_command_values() {
    let repo = Repo::new();
    let config = repo.root.join("settings");
    fs::write(&config, "wt-days=5\n").unwrap();
    // --audit-log is a literal exclusion name here, not a global option.
    assert_success(
        &repo
            .cli()
            .arg("--config")
            .arg(&config)
            .args(["repos", "--exclude", "--audit-log"])
            .output()
            .unwrap(),
    );
    assert!(!repo.work.join("--audit-log").exists());
}

#[cfg(unix)]
#[test]
fn combined_interactive_records_selected_outcome_and_cancellation() {
    for audit_first in [true, false] {
        for cancel in [true, false] {
            let repo = Repo::new();
            repo.git(&["branch", "a"]);
            repo.git(&["branch", "b"]);
            let config = repo.root.join("settings");
            fs::write(&config, "wt-days=5\n").unwrap();
            let log = repo.root.join("interactive.jsonl");
            let config = config.to_str().unwrap();
            let path = log.to_str().unwrap();
            let args = if audit_first {
                vec![
                    "--audit-log",
                    path,
                    "--config",
                    config,
                    "br",
                    "--interactive",
                ]
            } else {
                vec![
                    "--config",
                    config,
                    "--audit-log",
                    path,
                    "br",
                    "--interactive",
                ]
            };
            let output = interactive(&repo.cli(), &args, "", if cancel { "" } else { "1" });
            assert_success(&output);
            assert_eq!(repo.exists("a"), cancel);
            assert!(repo.exists("b"));
            let content = fs::read_to_string(&log).unwrap();
            assert_eq!(content.lines().count(), 2);
            assert!(content.contains("\"success\":true"));
            assert_eq!(content.contains("\"outcome\":\"deleted\""), !cancel);
            assert!(!content.contains(repo.root.to_str().unwrap()));
        }
    }
}

#[test]
fn combined_interactive_non_tty_and_log_failure_never_delete() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    let config = repo.root.join("settings");
    fs::write(&config, "wt-days=5\n").unwrap();
    let log = repo.root.join("interactive.jsonl");
    let output = repo
        .cli()
        .arg("--config")
        .arg(&config)
        .arg("--audit-log")
        .arg(&log)
        .args(["br", "--interactive"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(repo.exists("topic"));
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("\"success\":false")
    );
    let output = repo
        .cli()
        .arg("--audit-log")
        .arg(&log)
        .arg("--config")
        .arg(&config)
        .args(["br", "--interactive"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("audit"));
    assert!(repo.exists("topic"));
}
