#[cfg(unix)]
fn interactive(command: &Command, args: &[&str], mutation: &str, selection: &str) -> Output {
    let script = r#"
import os, pty, select, subprocess, sys, time
binary, mutation, selection, *args = sys.argv[1:]
master, slave = pty.openpty()
child = subprocess.Popen([binary, *args], stdin=slave, stdout=slave, stderr=slave)
os.close(slave)
data = b""
stage = 0
deadline = time.monotonic() + 30
try:
    while time.monotonic() < deadline:
        if select.select([master], [], [], .1)[0]:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            data += chunk
        if stage == 0 and b"(Enter/q cancels): " in data:
            if mutation:
                subprocess.run(["/bin/sh", "-c", mutation], check=True)
            os.write(master, (selection + "\n").encode())
            stage = 1
        if stage == 1 and b"Type yes: " in data:
            os.write(master, b"yes\n")
            stage = 2
        if child.poll() is not None:
            break
    else:
        child.kill()
        raise RuntimeError("interactive prompt timed out: " + repr(data))
    code = child.wait(timeout=5)
    sys.stdout.buffer.write(data)
    sys.exit(code)
finally:
    if child.poll() is None:
        child.kill()
        child.wait()
    os.close(master)
"#;
    let mut python = Command::new("python3");
    python.arg("-c").arg(script).arg(command.get_program()).arg(mutation).arg(selection).args(args);
    if let Some(cwd) = command.get_current_dir() {
        python.current_dir(cwd);
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value { python.env(key, value); } else { python.env_remove(key); }
    }
    python.output().unwrap()
}

#[test]
fn interactive_non_tty_fails_without_deletion() {
    let repo = Repo::new();
    repo.git(&["branch", "topic"]);
    for args in [
        vec!["br", "--interactive"],
        vec!["wt", "-d", "3", "--interactive"],
        vec!["br", "--interactive", "--dry-run"],
        vec!["br", "--interactive", "--format", "json"],
    ] {
        assert!(!repo.run(&args).status.success());
        assert!(repo.exists("topic"));
    }
}

#[cfg(unix)]
#[test]
fn interactive_br_selection_cancellation_and_changed_identity() {
    for mode in ["select", "cancel", "change"] {
        let repo = Repo::new();
        repo.git(&["branch", "a"]);
        repo.git(&["branch", "b"]);
        let mutation = if mode == "change" {
            // Still merged after the update: only the exact selected identity is authorized.
            "git commit --allow-empty -m newer >/dev/null && git branch -f a main >/dev/null && git push origin main >/dev/null"
        } else { "" };
        let output = interactive(&repo.cli(), &["br", "--interactive"], mutation, if mode == "cancel" { "" } else { "1" });
        assert_success(&output);
        assert_eq!(repo.exists("a"), mode != "select", "{mode}: {}", text(&output.stdout));
        assert!(repo.exists("b"));
    }
}
