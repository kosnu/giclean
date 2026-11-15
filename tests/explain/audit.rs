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
    assert!(content.contains("\"dry_run_requested\":true"));
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
