use std::process::Command;

pub fn command() -> Command {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::OsString;
        use std::sync::OnceLock;

        static EXECUTABLE: OnceLock<OsString> = OnceLock::new();
        Command::new(EXECUTABLE.get_or_init(executable))
    }
    #[cfg(not(target_os = "macos"))]
    Command::new("git")
}

#[cfg(target_os = "macos")]
fn executable() -> std::ffi::OsString {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Stdio;

    let fallback = || "git".into();
    let Some(path) = env::var_os("PATH") else {
        return fallback();
    };
    let selected = env::split_paths(&path)
        .map(|directory| directory.join("git"))
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        });
    // Only bypass Apple's launcher. A user-selected Git or PATH wrapper must
    // remain in control, including wrappers used by credential/tool managers.
    if selected.as_deref() != Some(Path::new("/usr/bin/git")) {
        return fallback();
    }
    let Ok(output) = Command::new("/usr/bin/xcrun")
        .args(["--find", "git"])
        .stderr(Stdio::null())
        .output()
    else {
        return fallback();
    };
    let Ok(path) = std::str::from_utf8(&output.stdout) else {
        return fallback();
    };
    let path = Path::new(path.trim());
    if output.status.success()
        && path.is_absolute()
        && path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    {
        path.as_os_str().to_owned()
    } else {
        fallback()
    }
}
