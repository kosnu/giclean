//! Capture /dev/tty prompts as well as stderr, without replacing Git's authentication.

use std::env;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Remote {
    directory: PathBuf,
    pub command: Command,
}

impl Remote {
    pub fn prepare(git: &Command) -> io::Result<Self> {
        // script is standard on the supported platforms, but optional here.
        // If it or a controlling terminal is unavailable, keep normal Git input.
        File::open("/dev/tty")?;
        fs::metadata("/usr/bin/script")?;
        let root = std::path::absolute(env::temp_dir())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..16 {
            let directory = root.join(format!(
                "giclean-terminal-{}-{timestamp}-{attempt}",
                std::process::id()
            ));
            match DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => {
                    let mut remote = Self {
                        directory,
                        command: Command::new("/bin/sh"),
                    };
                    File::create(remote.stdout())?;
                    remote.configure(git);
                    return Ok(remote);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot create private terminal directory",
        ))
    }

    fn configure(&mut self, git: &Command) {
        // Keep machine-readable stdout separate from the PTY's prompts. Git's
        // stdin remains /dev/null; /dev/tty belongs to script's child terminal.
        let mut child = vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("output=$1; shift; exec \"$@\" > \"$output\" < /dev/null"),
            OsString::from("giclean"),
            self.stdout().into_os_string(),
            git.get_program().to_owned(),
        ];
        child.extend(git.get_args().map(OsString::from));
        self.command.args([
            "-c",
            "exec \"$@\" < /dev/tty >&2",
            "giclean",
            "/usr/bin/script",
            "-q",
        ]);
        if cfg!(target_os = "macos") {
            self.command.arg("/dev/null").args(child);
        } else {
            self.command.args(["-e", "-c"]).arg(shell_command(&child));
            self.command.arg("/dev/null");
        }
        if let Some(directory) = git.get_current_dir() {
            self.command.current_dir(directory);
        }
        for (key, value) in git.get_envs() {
            match value {
                Some(value) => self.command.env(key, value),
                None => self.command.env_remove(key),
            };
        }
    }

    pub fn stdout(&self) -> PathBuf {
        self.directory.join("stdout")
    }
}

fn shell_command(args: &[OsString]) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let mut command = Vec::new();
    for arg in args {
        if !command.is_empty() {
            command.push(b' ');
        }
        command.push(b'\'');
        for &byte in arg.as_bytes() {
            if byte == b'\'' {
                command.extend_from_slice(b"'\\''");
            } else {
                command.push(byte);
            }
        }
        command.push(b'\'');
    }
    OsString::from_vec(command)
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
