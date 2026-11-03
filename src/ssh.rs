use std::process::Command;

#[cfg(unix)]
pub fn configure(command: &mut Command, remote: &str) {
    use std::collections::hash_map::DefaultHasher;
    use std::env;
    use std::hash::{Hash, Hasher};
    use std::path::PathBuf;
    use std::process::Stdio;

    if !is_ssh_remote(remote)
        || ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_SSH_VARIANT"]
            .iter()
            .any(|key| env::var_os(key).is_some())
    {
        return;
    }
    // An explicitly selected SSH command or variant belongs to the user.
    let Ok(config) = crate::git::command()
        .args([
            "config",
            "--get-regexp",
            r"^(core\.sshcommand|ssh\.variant|protocol\.version)$",
        ])
        .output()
    else {
        return;
    };
    if !matches!(config.status.code(), Some(0 | 1)) {
        return;
    }
    let Ok(config) = std::str::from_utf8(&config.stdout) else {
        return;
    };
    if config
        .lines()
        .any(|line| !line.starts_with("protocol.version "))
    {
        return;
    }
    let Ok(version) = Command::new("ssh").arg("-V").output() else {
        return;
    };
    if !version.status.success() || !supports_multiplexing(&version.stderr) {
        return;
    }
    let Ok(user) = Command::new("id").arg("-u").stderr(Stdio::null()).output() else {
        return;
    };
    if !user.status.success() {
        return;
    }
    let Some(uid) = std::str::from_utf8(&user.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
    else {
        return;
    };
    // A short, private directory avoids Unix socket path limits, including macOS.
    let directory = PathBuf::from(format!("/tmp/giclean-ssh-{uid}"));
    if !private_directory(&directory, uid) {
        return;
    }
    let mut hash = DefaultHasher::new();
    remote.hash(&mut hash);
    // The URL hash also separates host aliases that use different identities.
    // Only numeric/hex values and fixed text enter this shell-interpreted value.
    let ssh = format!(
        "ssh -o ControlMaster=auto -o ControlPersist=60 -o SendEnv=GIT_PROTOCOL -o ControlPath={}/cm-{:016x}-%C",
        directory.display(),
        hash.finish()
    );
    command
        .env("GIT_SSH_COMMAND", ssh)
        .env("GIT_SSH_VARIANT", "ssh");
    // v0 advertises HEAD immediately; v2 needs an additional ls-refs round trip.
    // Keep an explicitly selected protocol, including command-scope settings.
    if config.is_empty() {
        command.args(["-c", "protocol.version=0"]);
    }
}

#[cfg(not(unix))]
pub fn configure(_command: &mut Command, _remote: &str) {}

#[cfg(unix)]
fn is_ssh_remote(remote: &str) -> bool {
    remote.starts_with("ssh://")
        || remote.starts_with("git+ssh://")
        || (!remote.contains("://")
            && remote.split_once(':').is_some_and(|(host, path)| {
                !host.is_empty()
                    && !host.contains('/')
                    && (!path.starts_with(':') || host.contains('['))
            }))
}

#[cfg(unix)]
fn supports_multiplexing(version: &[u8]) -> bool {
    let Some(version) = std::str::from_utf8(version)
        .ok()
        .and_then(|value| value.strip_prefix("OpenSSH_"))
    else {
        return false;
    };
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };
    let major = major.parse::<u32>();
    let minor = minor
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap_or_default()
        .parse::<u32>();
    matches!((major, minor), (Ok(major), Ok(minor)) if (major, minor) >= (6, 7))
}

#[cfg(unix)]
fn private_directory(path: &std::path::Path, uid: u32) -> bool {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o777 == 0o700
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn only_ssh_transports_are_eligible() {
        for remote in [
            "git@github.com:kosnu/savings.git",
            "host:repo.git",
            "ssh://git@host:2222/repo.git",
            "git+ssh://git@host/repo.git",
            "git@[::1]:repo.git",
        ] {
            assert!(is_ssh_remote(remote), "{remote}");
        }
        for remote in [
            "https://github.com/kosnu/savings.git",
            "file:///tmp/repo.git",
            "git://host/repo.git",
            "/tmp/repo.git",
            "./host:repo.git",
            "../host:repo.git",
            "ext::custom-helper",
            "repo.git",
            ":repo.git",
        ] {
            assert!(!is_ssh_remote(remote), "{remote}");
        }
    }

    #[test]
    fn requires_openssh_with_control_path_hash_support() {
        for version in [
            "OpenSSH_6.7p1\n",
            "OpenSSH_9.9p2, LibreSSL\n",
            "OpenSSH_10.0p1\n",
        ] {
            assert!(supports_multiplexing(version.as_bytes()), "{version}");
        }
        for version in [
            "OpenSSH_6.6p1\n",
            "OpenSSH_5.9\n",
            "other-ssh\n",
            "OpenSSH_invalid\n",
        ] {
            assert!(!supports_multiplexing(version.as_bytes()), "{version}");
        }
    }

    #[test]
    fn control_directory_must_be_private_owned_and_not_a_symlink() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "giclean-ssh-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let uid = fs::metadata(&root).unwrap().uid();
        let directory = root.join("control");
        assert!(private_directory(&directory, uid));
        assert!(private_directory(&directory, uid));
        assert!(!private_directory(&directory, uid.wrapping_add(1)));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!private_directory(&directory, uid));
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o755);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.join("link");
        symlink(&directory, &link).unwrap();
        assert!(!private_directory(&link, uid));
        let file = root.join("file");
        fs::write(&file, "not a directory").unwrap();
        assert!(!private_directory(&file, uid));
        fs::remove_dir_all(root).unwrap();
    }
}
